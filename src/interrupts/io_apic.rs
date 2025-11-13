use spin::{Once, RwLock};
use core::ptr::{read_volatile, write_volatile};
use ::acpi::platform::interrupt::{TriggerMode, Polarity};
use ::acpi::InterruptModel;
use alloc::vec::Vec;
use alloc::format;
use alloc::collections::btree_map::BTreeMap;

use crate::interrupts::IRQ_BASE;
use crate::memory;
use crate::sys::acpi;

const IOAPICVER: u8 = 0x01;
const IOAPIC_MAX_REDIRECITON_ENTRY_MASK: u32 = 0x00FF_0000;
const IOAPIC_MAX_REDIRECTION_ENTRY_SHIFT: u32 = 16;

const IOAPIC_INTMASK_DISABLED: u32 = 1 << 16;
const IOAPIC_INTMASK_ENABLED: u32 = !IOAPIC_INTMASK_DISABLED;

const IOAPIC_TRIGGER_EDGE: u32 = 0 << 15;
const IOAPIC_TRIGGER_LEVEL: u32 = 1 << 15;

const IOAPIC_POLARITY_HIGH: u32 = 0 << 13;
const IOAPIC_POLARITY_LOW: u32 = 1 << 13;

const IOAPIC_DESTINATION_PHYSICAL: u32 = 0 << 11;
const IOAPIC_DESTINATION_LOGICAL: u32 = 1 << 11;

const IOAPIC_DELIVERY_FIXED: u32 = 0 << 8;

struct IoApic {
    ioregsel: *mut u32,
    iowin: *mut u32,
    id: u8,
    global_system_interrupt_base: u32,

    gsi_to_irq: BTreeMap<u32, u8>,
}

unsafe impl Send for IoApic {}
unsafe impl Sync for IoApic {}

impl IoApic {
    pub fn new(id: u8, base_addr: u32, global_system_interrupt_base: u32) -> IoApic {
	let size = 0x20;

	let (virt_addr, _) = match memory::allocate_arbitrary_contiguous_region_kernel(
	    base_addr as usize, size as usize, memory::MemoryAllocationType::MMIO(base_addr as u64)) {
	    Ok(v) => v,
	    Err(e) => panic!("{:#?}", e),
	};

	IoApic {
	    ioregsel: virt_addr.as_mut_ptr(),
	    iowin: (virt_addr + 0x10).as_mut_ptr(),
	    id: id,
	    global_system_interrupt_base: global_system_interrupt_base,
	    gsi_to_irq: BTreeMap::new(),
	}
    }

    fn read_reg(&self, reg_num: u8) -> u32 {
	unsafe {
	    write_volatile(self.ioregsel, reg_num as u32);
	    read_volatile(self.iowin)
	}
    }

    fn write_reg(&self, reg_num: u8, val: u32) {
	unsafe {
	    write_volatile(self.ioregsel, reg_num as u32);
	    write_volatile(self.iowin, val);
	}
    }

    pub fn get_max_redirection_table_entries(&self) -> u8 {
	let ver = self.read_reg(IOAPICVER);
	((ver & IOAPIC_MAX_REDIRECITON_ENTRY_MASK) >> IOAPIC_MAX_REDIRECTION_ENTRY_SHIFT) as u8
    }

    pub fn contains_gsi(&self, gsi: u32) -> bool {
	self.global_system_interrupt_base <= gsi &&
	    gsi <= self.global_system_interrupt_base + self.get_max_redirection_table_entries() as u32
    }

    pub fn map_interrupt(&mut self, gsi: u32, dest_apic: u32, trigger_mode: TriggerMode, polarity: Polarity, vector: u8) {
	self.gsi_to_irq.entry(gsi).and_modify(|irq| *irq = vector).or_insert(vector);

	let ioredtbl_hi = dest_apic << 24;
	let ioredtbl_lo = IOAPIC_INTMASK_DISABLED | match trigger_mode {
	    TriggerMode::Edge => IOAPIC_TRIGGER_EDGE,
	    TriggerMode::Level => IOAPIC_TRIGGER_LEVEL,
	    TriggerMode::SameAsBus => IOAPIC_TRIGGER_EDGE,
	} | match polarity {
	    Polarity::ActiveHigh => IOAPIC_POLARITY_HIGH,
	    Polarity::ActiveLow => IOAPIC_POLARITY_LOW,
	    Polarity::SameAsBus => IOAPIC_POLARITY_HIGH,
	} | IOAPIC_DESTINATION_PHYSICAL | IOAPIC_DELIVERY_FIXED | vector as u32;

	let ioredtbl_idx = gsi - self.global_system_interrupt_base;
	let ioredtbl_lo_idx: u8 = (ioredtbl_idx as u8 * 2) + 0x10;
	let ioredtbl_hi_idx: u8 = (ioredtbl_idx as u8 * 2) + 0x11;

	self.write_reg(ioredtbl_lo_idx, ioredtbl_lo);
	self.write_reg(ioredtbl_hi_idx, ioredtbl_hi);
    }

    pub fn get_irq_for_gsi(&self, gsi: u32) -> Option<u8> {
	match self.gsi_to_irq.get(&gsi) {
	    Some(&i) => Some(i),
	    None => None,
	}
    }

    pub fn enable_gsi(&self, gsi: u32) {
	let ioredtbl_idx = gsi - self.global_system_interrupt_base;
	let ioredtbl_lo_idx: u8 = (ioredtbl_idx as u8 * 2) + 0x10;
	let ioredtbl_lo = self.read_reg(ioredtbl_lo_idx) & IOAPIC_INTMASK_ENABLED;
	let ioredtbl_hi_idx: u8 = (ioredtbl_idx as u8 * 2) + 0x11;
	let ioredtbl_hi = self.read_reg(ioredtbl_hi_idx);

	self.write_reg(ioredtbl_lo_idx, ioredtbl_lo);
	self.write_reg(ioredtbl_hi_idx, ioredtbl_hi);
    }
}

static IOAPICS: Once<RwLock<Vec<IoApic>>> = Once::new();
static IRQ_TO_GSI: Once<RwLock<BTreeMap<u8, u32>>> = Once::new();

pub fn init_io_apics(bsp_apic_id: u64) {
    let acpi = acpi::ACPI.get().expect("Attempted to access ACPI tables before ACPI is initialised").read();
    let platform_info = match acpi.platform_info() {
	Ok(pi) => pi,
	Err(e) => panic!("{:#?}", e),
    };

    let interrupt_model = match platform_info.interrupt_model {
	InterruptModel::Unknown => panic!("ACPI reports no APIC presence. CPU not supported."),
	InterruptModel::Apic(a) => a,
	_ => panic!("Unrecognised interrupt model."),
    };

    let mut ioapics = IOAPICS.call_once(|| RwLock::new(Vec::<IoApic>::new())).write();
    for io_apic in interrupt_model.io_apics.iter() {
	log::info!("Found I/O APIC, ID=0x{:x}, GSI base=0x{:x}", io_apic.id, io_apic.global_system_interrupt_base);
	ioapics.push(IoApic::new(io_apic.id, io_apic.address, io_apic.global_system_interrupt_base));
    }

    let mut irq_to_gsi = IRQ_TO_GSI.call_once(|| RwLock::new(BTreeMap::<u8, u32>::new())).write();
    let interrupt_source_overrides = interrupt_model.interrupt_source_overrides;
    for over in interrupt_source_overrides.iter() {
	irq_to_gsi.insert(over.isa_source, over.global_system_interrupt);

	find_ioapic(over.global_system_interrupt, &mut ioapics)
	    .expect(
		format!("Unable to find an I/O APIC for legacy IRQ {}/GSI {}",
			over.isa_source,
			over.global_system_interrupt)
		    .as_str())
	    .map_interrupt(
		over.global_system_interrupt,
		bsp_apic_id as u32,
		over.trigger_mode,
		over.polarity,
		over.isa_source + IRQ_BASE);
    }

    for legacy_irq in 0..=15 {
	// If something is already mapped here, don't map anything
	if interrupt_source_overrides
	    .iter()
	    .any(|over| over.isa_source == legacy_irq) {
		continue;
	    }

	// If this IRQ is already mapped, don't remap
	if interrupt_source_overrides
	    .iter()
	    .any(|over| over.global_system_interrupt == legacy_irq as u32) {
		continue;
	    }

	irq_to_gsi.insert(legacy_irq, legacy_irq as u32);

	find_ioapic(legacy_irq as u32, &mut ioapics)
	    .expect(
		format!("Unable to find an I/O APIC for legacy IRQ {}", legacy_irq)
		    .as_str())
	    .map_interrupt(
		legacy_irq as u32,
		bsp_apic_id as u32,
		TriggerMode::SameAsBus,
		Polarity::SameAsBus,
		legacy_irq + IRQ_BASE);
    }
}

fn find_ioapic(gsi: u32, ioapics: &mut Vec<IoApic>) -> Option<&mut IoApic> {
    for apic in ioapics {
	if (*apic).contains_gsi(gsi) {
	    return Some(apic);
	}
    }

    None
}

pub fn get_irq_for_gsi(gsi: u32) -> u8 {
    let mut ioapics = IOAPICS.call_once(|| RwLock::new(Vec::<IoApic>::new())).write();
    let io_apic = find_ioapic(gsi, &mut ioapics).expect(&format!("GSI {} not found", gsi));

    match io_apic.get_irq_for_gsi(gsi) {
	Some(irq) => irq,
	None => unimplemented!(),
    }
}

pub fn enable_gsi(gsi: u32) {
    let mut ioapics = IOAPICS.call_once(|| RwLock::new(Vec::<IoApic>::new())).write();
    let io_apic = find_ioapic(gsi, &mut ioapics).expect(&format!("GSI {} not found", gsi));

    io_apic.enable_gsi(gsi);
}

pub fn enable_irq(irq: u8) {
    let irq_to_gsi = IRQ_TO_GSI.call_once(|| RwLock::new(BTreeMap::<u8, u32>::new())).read();
    let gsi = irq_to_gsi.get(&irq).unwrap();

    let mut ioapics = IOAPICS.call_once(|| RwLock::new(Vec::<IoApic>::new())).write();
    let io_apic = find_ioapic(*gsi, &mut ioapics).expect(&format!("GSI {} not found", gsi));

    io_apic.enable_gsi(*gsi);
}
