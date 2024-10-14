use crate::driver;
use aml::{AmlName, AmlValue, value::Args, resource::{resource_descriptor_list, Resource, MemoryRangeDescriptor}};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::format;
use spin::{Once, RwLock};
use core::ptr::{read_volatile, write_volatile};
use x86_64::PhysAddr;

use crate::sys::acpi;
use crate::memory;

const LEG_RT_CNF: u64 = 0x02;
const ENABLE_CNF: u64 = 0x01;

struct HpetCounter {
    pub configuration_capability_register: *mut u64,
    pub comparator_value_register: *mut u64,
    pub fsb_interrupt_route_register: *mut u64,
}

struct Hpet {
    device_id: u64,
    general_capabilities_register: *const u64,
    general_configuration_register: *mut u64,
    general_interrupt_status_register: *mut u64,
    main_counter_value_register: *mut u64,
    counters: Vec<HpetCounter>,
}

unsafe impl Send for Hpet {}
unsafe impl Sync for Hpet {}

impl Hpet {
    pub fn new(device_id: u64, base_addr: u32, size: u32) -> Hpet {
	let start_phys_addr = base_addr - (base_addr % 4096);  // Page align
	let total_size = size + (base_addr % 4096) + (4096 - (size % 4096));  // Total amount, aligned to page boundaries

	let allocated_region = match memory::allocate_contiguous_region_kernel(
	    total_size as u64, PhysAddr::new(start_phys_addr as u64), memory::MemoryAllocationType::MMIO) {
	    Ok(v) => v,
	    Err(e) => panic!("{:#?}", e),
	};

	let offset_from_start = base_addr - start_phys_addr;
	let virt_addr = allocated_region + offset_from_start as u64;

	let mut hpet = Hpet {
	    device_id: device_id,
	    general_capabilities_register: virt_addr.as_ptr(),
	    general_configuration_register: (virt_addr + 0x10).as_mut_ptr(),
	    general_interrupt_status_register: (virt_addr + 0x20).as_mut_ptr(),
	    main_counter_value_register: (virt_addr + 0xF0).as_mut_ptr(),
	    counters: Vec::new(),
	};

	let num_counters = unsafe {
	    (read_volatile::<u64>(hpet.general_capabilities_register) & 0xF00) >> 8
	};

	for counter in 0..=num_counters {
	    let counter_base = virt_addr + 0x100 + (counter * 020);
	    hpet.counters.push(HpetCounter {
		configuration_capability_register: counter_base.as_mut_ptr(),
		comparator_value_register: (counter_base + 0x08).as_mut_ptr(),
		fsb_interrupt_route_register: (counter_base + 0x10).as_mut_ptr(),
	    });
	}

	unsafe {
	    write_volatile::<u64>(hpet.general_configuration_register, ENABLE_CNF);
	}

	hpet
    }
}

static HPETS: Once<RwLock<Vec<Hpet>>> = Once::new();

pub fn init() {
    let hpet_driver = driver::DriverInfo {
	hid: String::from("PNP0103"),
	init: init_driver,
    };
    driver::register_driver(hpet_driver);
    HPETS.call_once(|| RwLock::new(Vec::<Hpet>::new()));
}

fn init_driver(driver_id: u64, acpi_device: &AmlName, uid: u32) {
    let crs_path = acpi_device.as_string() + "._CRS";
    let crs = {
	let mut aml = acpi::AML.get().expect("Attempted to access ACPI tables before ACPI is initialised").write();
	match aml.invoke_method(
	    &AmlName::from_str(&crs_path).expect(&format!("Unable to construct AmlName {}", &crs_path)),
	    Args::EMPTY,
	) {
	    Ok(AmlValue::Buffer(v)) => AmlValue::Buffer(v),
	    _ => panic!("CRS expected for HPET"),
	}
    };

    let resources = match resource_descriptor_list(&crs) {
	Ok(v) => v,
	Err(e) => panic!("Malformed CRS for HPET: {:#?}", e),
    };

    let (base_address, range_length) = resources.iter()
	.filter(|r| match r {
	    Resource::MemoryRange(_) => true,
	    _ => false,
	}).map(|r| match r {
	    Resource::MemoryRange(MemoryRangeDescriptor::FixedLocation {
		is_writable: _,
		base_address: base_address,
		range_length: range_length
	    }) => (base_address, range_length),
	    _ => panic!("This shouldn't happen"),
	}).nth(0).expect("No memory ranges returned for HPET");

    let device = driver::DeviceInfo {
	driver_id: driver_id,
	uid: uid,
	is_loaded: true
    };
    let device_id = driver::register_device(device);

    {
	let mut hpets = HPETS.get().expect("Attempted to initialise HPET device before initialising driver").write();
	hpets.push(Hpet::new(device_id, *base_address, *range_length));
    }
}
