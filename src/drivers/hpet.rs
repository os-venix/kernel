use crate::driver;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::sync::Arc;
use alloc::boxed::Box;
use spin::{Mutex, Once, RwLock};
use core::ptr::{read_volatile, write_volatile};

use crate::sys::acpi::{namespace, resources};
use crate::memory;
use crate::interrupts;

const LEG_RT_CNF: u64 = 0x02;
const ENABLE_CNF: u64 = 0x01;

const HPET_COUNTER_SET_ACCUMULATOR: u64 = 1 << 6;
const HPET_COUNTER_PERIODIC: u64 = 1 << 3;
const HPET_COUNTER_NON_PERIODIC: u64 = !HPET_COUNTER_PERIODIC;
const HPET_COUNTER_ENABLED: u64 = 1 << 2;
const HPET_COUNTER_LEVEL_TRIGGERED: u64 = 1 << 1;

struct HpetCounter {
    pub configuration_capability_register: *mut u64,
    pub comparator_value_register: *mut u64,
    pub fsb_interrupt_route_register: *mut u64,
}

struct TimerCallback(u8, Box<dyn Fn() + Send + Sync>);

struct Hpet {
    counter_64: bool,
    general_capabilities_register: *const u64,
    general_configuration_register: *mut u64,
    general_interrupt_status_register: *mut u64,
    main_counter_value_register: *mut u64,
    counters: Vec<HpetCounter>,
    callbacks: Vec<TimerCallback>,
    free_counters: Vec<u8>,
    periodic_counters: Vec<u8>,
}

unsafe impl Send for Hpet {}
unsafe impl Sync for Hpet {}

impl Hpet {
    pub fn new(base_addr: u32, size: u32) -> Hpet {	
	let (virt_addr, _) = match memory::allocate_arbitrary_contiguous_region_kernel(
	    base_addr as usize, size as usize, memory::MemoryAllocationType::MMIO) {
	    Ok(v) => v,
	    Err(e) => panic!("{:#?}", e),
	};

	let mut hpet = Hpet {
	    counter_64: false,
	    general_capabilities_register: virt_addr.as_ptr(),
	    general_configuration_register: (virt_addr + 0x10).as_mut_ptr(),
	    general_interrupt_status_register: (virt_addr + 0x20).as_mut_ptr(),
	    main_counter_value_register: (virt_addr + 0xF0).as_mut_ptr(),
	    counters: Vec::new(),
	    callbacks: Vec::new(),
	    free_counters: Vec::new(),
	    periodic_counters: Vec::new(),
	};

	let num_counters = unsafe {
	    (read_volatile::<u64>(hpet.general_capabilities_register) & 0xF00) >> 8
	};

	for counter in 0..=num_counters {
	    let counter_base = virt_addr + 0x100 + (counter * 0x20);
	    hpet.counters.push(HpetCounter {
		configuration_capability_register: counter_base.as_mut_ptr(),
		comparator_value_register: (counter_base + 0x08).as_mut_ptr(),
		fsb_interrupt_route_register: (counter_base + 0x10).as_mut_ptr(),
	    });
	    hpet.free_counters.push(counter as u8);
	}

	let counter_64 = unsafe {
	    (read_volatile::<u64>(hpet.general_capabilities_register) & 0x2000) >> 13
	};
	hpet.counter_64 = counter_64 == 1;

	unsafe {
	    write_volatile::<u64>(hpet.general_configuration_register, ENABLE_CNF);
	}

	let mut possible_routings: u32 = 0xFFFF_FFFF;
	// Disable all counters
	for counter in 0..=num_counters {
	    let routing = unsafe {
		let capabilities = read_volatile::<u64>(hpet.counters[counter as usize].configuration_capability_register);
		capabilities >> 32
	    };

	    possible_routings &= routing as u32;
	    unsafe {
		write_volatile::<u64>(hpet.counters[counter as usize].configuration_capability_register, 0);
	    }
	}

	if possible_routings == 0 {
	    panic!("No routings found in common for HPET. This is not yet a supported mode. All counters must use the same IRQ");
	}

	// Find the lowest possible IRQ
	let mut gsi = 0;
	for _ in 0 .. 32 {
	    if (possible_routings & 1) == 1 {
		break;
	    } else {
		possible_routings >>= 1;
		gsi += 1;
	    }
	}

	interrupts::enable_gsi(gsi, &hpet_handler);

	hpet
    }

    pub fn find_timer_interrupting(&mut self) -> Option<u64> {
	let mut gisr = unsafe {
	    read_volatile::<u64>(self.general_interrupt_status_register)
	};

	if gisr != 0 {
	    let mut interrupting_counter = 0;
	    for _ in 0 .. 64 {
		if (gisr & 1) == 1 {
		    break;
		} else {
		    gisr >>= 1;
		    interrupting_counter += 1;
		}
	    }

	    let interrupting_timer_ack = 1 << interrupting_counter;

	    unsafe {
		write_volatile::<u64>(self.general_interrupt_status_register, interrupting_timer_ack);
	    }
	    Some(interrupting_counter)
	} else {
	    None
	}
    }

    pub fn num_timers(&self) -> u8 {
	unsafe {
	    ((read_volatile::<u64>(self.general_capabilities_register) & 0xF00) >> 8) as u8
	}
    }
    
    pub fn set_timer(&self, timer: u8, time_ms: u64, is_periodic: bool) {
	let counter = &self.counters[timer as usize];
	let mut config = unsafe {
	    read_volatile::<u64>(counter.configuration_capability_register)
	};

	if is_periodic {
	    config |= HPET_COUNTER_PERIODIC | HPET_COUNTER_SET_ACCUMULATOR;
	} else {
	    config &= HPET_COUNTER_NON_PERIODIC;
	}
	config |= HPET_COUNTER_ENABLED | HPET_COUNTER_LEVEL_TRIGGERED;

	let counter_divider = unsafe {
	    (read_volatile::<u64>(self.general_capabilities_register) & 0xFFFF_FFFF_0000_0000) >> 32
	};

	let time_in_ticks = (time_ms * 10_u64.pow(12)) / counter_divider;
	let main_counter_val = unsafe {
	    read_volatile::<u64>(self.main_counter_value_register)
	};

	let mut counter_comparator_val = main_counter_val.wrapping_add(time_in_ticks);
	if !self.counter_64 {
	    counter_comparator_val &= 0xFFFF_FFFF;
	}

	unsafe {
	    write_volatile::<u64>(counter.comparator_value_register, counter_comparator_val);
	    write_volatile::<u64>(counter.configuration_capability_register, config);
	    if is_periodic {
		write_volatile::<u64>(counter.comparator_value_register, time_in_ticks);		
	    }
	}
    }

    pub fn add_oneshot_ms(&mut self, time_ms: u64, callback: Box<dyn Fn() + Send + Sync>) {
	if let Some(counter) = self.free_counters.pop() {
	    self.callbacks.push(TimerCallback(counter, callback));
	    self.set_timer(counter, time_ms, false);
	}
    }

    pub fn add_recurring_ms(&mut self, time_ms: u64, callback: Box<dyn Fn() + Send + Sync>) {
	if let Some(counter) = self.free_counters.pop() {
	    self.callbacks.push(TimerCallback(counter, callback));
	    self.periodic_counters.push(counter);
	    self.set_timer(counter, time_ms, true);
	}
    }

    pub fn handle_triggered_callbacks(&mut self) {
	if let Some(counter) = self.find_timer_interrupting() {
	    if self.periodic_counters.contains(&(counter as u8)) {
		self.callbacks
		    .iter()
		    .filter(|callback| callback.0 == counter as u8)
		    .for_each(|callback| callback.1());
	    } else {
		self.callbacks
		    .extract_if(|callback| callback.0 == counter as u8)
		    .map(|callback| callback.1())
		    .for_each(drop);

		self.free_counters.push(counter as u8);
	    }
	}
    }
}

fn hpet_handler() {
    let mut hpet = HPET.get().expect("Attempted to initialise HPET device before initialising driver").write();
    hpet.handle_triggered_callbacks();
}

pub fn add_oneshot(time_ms: u64, callback: Box<dyn Fn() + Send + Sync>) {
    let mut hpet = HPET.get().expect("Attempted to initialise HPET device before initialising driver").write();
    hpet.add_oneshot_ms(time_ms, callback);
}

pub fn add_periodic(time_ms: u64, callback: Box<dyn Fn() + Send + Sync>) {
    let mut hpet = HPET.get().expect("Attempted to initialise HPET device before initialising driver").write();
    hpet.add_recurring_ms(time_ms, callback);
}

static HPET: Once<RwLock<Hpet>> = Once::new();

pub fn init() {
    let hpet_driver = HpetDriver {};
    driver::register_driver(Box::new(hpet_driver));
}

pub struct HpetDevice {}
unsafe impl Send for HpetDevice { }
unsafe impl Sync for HpetDevice { }
impl driver::Device for HpetDevice {
    fn read(&self, offset: u64, size: u64, access_restriction: memory::MemoryAccessRestriction) -> Result<*const u8, ()> {
	panic!("Shouldn't have attempted to read from the HPET. That makes no sense.");
    }
    fn write(&self, buf: *const u8, size: u64) -> Result<u64, ()> {
	panic!("Shouldn't have attempted to write to the HPET. That makes no sense.");
    }
}

pub struct HpetDriver {}
impl driver::Driver for HpetDriver {
    fn init(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) {
	let system_bus_identifier = if let Some(sb_info) = info.as_any().downcast_ref::<namespace::SystemBusDeviceIdentifier>() {
	    sb_info
	} else {
	    panic!("Attempted to get SB identifier from a not SB device");
	};

	let resources = resources::get_resources(system_bus_identifier.namespace).unwrap();
	let (base_address, range_length) = resources.iter()
	    .filter(|r| match r {
		resources::Resource::FixedMemory32 { .. } => true,
		_ => false,
	    }).map(|r| match r {
		resources::Resource::FixedMemory32 {
		    write_status: _,
		    address,
		    length
		} => (address, length),
		_ => panic!("This shouldn't happen"),
	    }).nth(0).expect("No memory ranges returned for HPET");

	let device = Arc::new(Mutex::new(HpetDevice {}));
	driver::register_device(device);

	HPET.call_once(|| RwLock::new(Hpet::new(*base_address, *range_length)));

	// Disable PIT, we don't use it
	log::info!("Disabling PIT");
	unsafe {
	    x86_64::instructions::port::PortWrite::write_to_port(0x43, 0x3A as u8);
	    x86_64::instructions::port::PortWrite::write_to_port(0x43, 0x7A as u8);
	    x86_64::instructions::port::PortWrite::write_to_port(0x43, 0xBA as u8);
	}
    }

    fn check_device(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	if let Some(sb_info) = info.as_any().downcast_ref::<namespace::SystemBusDeviceIdentifier>() {
	    if let Some(hid) = &sb_info.hid {
		*hid == String::from("PNP0103")
	    } else {
		false
	    }
	} else {
	    false
	}
    }

    fn check_new_device(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	!HPET.is_completed()
    }
}
