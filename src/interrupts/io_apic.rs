use spin::{Once, RwLock};
use core::ptr::{read_volatile, write_volatile};
use alloc::vec::Vec;
use alloc::collections::btree_map::BTreeMap;

use crate::interrupts::IRQ_BASE;
use crate::memory;
use crate::sys::acpi;
use crate::sys::acpi::interrupts::{Polarity, TriggerMode};

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

const IOAPIC_DELIVERY_FIXED: u32 = 0 << 8;

struct IoApic {
    ioregsel: *mut u32,
    iowin: *mut u32,
    global_system_interrupt_base: u32,

    gsi_to_irq: BTreeMap<u32, u8>,
}

unsafe impl Send for IoApic {}
unsafe impl Sync for IoApic {}

impl IoApic {
    pub fn new(base_addr: u32, global_system_interrupt_base: u32) -> IoApic {
	let size = 0x20;

	let virt_addr = match memory::allocate_mmio(base_addr as usize, size as usize) {
	    Ok(v) => v,
	    Err(e) => panic!("{:#?}", e),
	};

	IoApic {
	    ioregsel: virt_addr.as_mut_ptr(),
	    iowin: (virt_addr + 0x10).as_mut_ptr(),
	    global_system_interrupt_base,
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

    pub fn map_interrupt(&mut self, gsi: u32, dest_apic: u32, trigger_mode: acpi::interrupts::TriggerMode, polarity: acpi::interrupts::Polarity, vector: u8) {
	self.gsi_to_irq.entry(gsi).and_modify(|irq| *irq = vector).or_insert(vector);

	let ioredtbl_hi = dest_apic << 24;
	let ioredtbl_lo = IOAPIC_INTMASK_DISABLED | match trigger_mode {
	    TriggerMode::Edge => IOAPIC_TRIGGER_EDGE,
	    TriggerMode::Level => IOAPIC_TRIGGER_LEVEL,
	    TriggerMode::Conforming => IOAPIC_TRIGGER_EDGE,
	} | match polarity {
	    Polarity::ActiveHigh => IOAPIC_POLARITY_HIGH,
	    Polarity::ActiveLow => IOAPIC_POLARITY_LOW,
	    Polarity::Conforming => IOAPIC_POLARITY_HIGH,
	} | IOAPIC_DESTINATION_PHYSICAL | IOAPIC_DELIVERY_FIXED | vector as u32;

	let ioredtbl_idx = gsi - self.global_system_interrupt_base;
	let ioredtbl_lo_idx: u8 = (ioredtbl_idx as u8 * 2) + 0x10;
	let ioredtbl_hi_idx: u8 = (ioredtbl_idx as u8 * 2) + 0x11;

	self.write_reg(ioredtbl_lo_idx, ioredtbl_lo);
	self.write_reg(ioredtbl_hi_idx, ioredtbl_hi);
    }

    pub fn get_irq_for_gsi(&self, gsi: u32) -> Option<u8> {
	self.gsi_to_irq.get(&gsi).copied()
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
    let io_apic_data = acpi::interrupts::iterate_madt_ioapics().unwrap();

    let mut ioapics = IOAPICS.call_once(|| RwLock::new(Vec::<IoApic>::new())).write();
    for io_apic in io_apic_data.io_apics.iter() {
	let gsi_base = io_apic.gsi_base;
	log::info!("Found I/O APIC, ID=0x{:x}, GSI base=0x{:x}", io_apic.id, gsi_base);
	ioapics.push(IoApic::new(io_apic.address, gsi_base));
    }

    let mut irq_to_gsi = IRQ_TO_GSI.call_once(|| RwLock::new(BTreeMap::<u8, u32>::new())).write();
    for over in io_apic_data.isos.iter() {
	let gsi = over.gsi;
	irq_to_gsi.insert(over.source, gsi);

	ioapics.iter_mut().find(|apic| (*apic).contains_gsi(over.gsi))
	    .unwrap_or_else(|| panic!("Unable to find an I/O APIC for legacy IRQ {}/GSI {}",
			over.source,
			gsi))
	    .map_interrupt(
		gsi,
		bsp_apic_id as u32,
		over.trigger_mode().unwrap(),
		over.polarity().unwrap(),
		over.source + IRQ_BASE);
    }

    for legacy_irq in 0..=15 {
	// If something is already mapped here, don't map anything
	if io_apic_data.isos.iter()
	    .any(|over| over.source == legacy_irq) {
		continue;
	    }

	// If this IRQ is already mapped, don't remap
	if io_apic_data.isos.iter()
	    .any(|over| over.gsi == legacy_irq as u32) {
		continue;
	    }

	irq_to_gsi.insert(legacy_irq, legacy_irq as u32);

	ioapics.iter_mut().find(|apic| (*apic).contains_gsi(legacy_irq as u32))
	    .unwrap_or_else(|| panic!("Unable to find an I/O APIC for legacy IRQ {}", legacy_irq))
	    .map_interrupt(
		legacy_irq as u32,
		bsp_apic_id as u32,
		TriggerMode::Conforming,
		Polarity::Conforming,
		legacy_irq + IRQ_BASE);
    }
}

pub fn get_irq_for_gsi(gsi: u32) -> u8 {
    let mut ioapics = IOAPICS.call_once(|| RwLock::new(Vec::<IoApic>::new())).write();
    let io_apic = ioapics.iter_mut().find(|apic| (*apic).contains_gsi(gsi))
	.unwrap_or_else(|| panic!("GSI {} not found", gsi));

    match io_apic.get_irq_for_gsi(gsi) {
	Some(irq) => irq,
	None => unimplemented!(),
    }
}

pub fn enable_gsi(gsi: u32) {
    let mut ioapics = IOAPICS.call_once(|| RwLock::new(Vec::<IoApic>::new())).write();
    let io_apic = ioapics.iter_mut().find(|apic| (*apic).contains_gsi(gsi))
	.unwrap_or_else(|| panic!("GSI {} not found", gsi));

    io_apic.enable_gsi(gsi);
}

pub fn enable_irq(irq: u8) {
    let irq_to_gsi = IRQ_TO_GSI.call_once(|| RwLock::new(BTreeMap::<u8, u32>::new())).read();
    let gsi = irq_to_gsi.get(&irq).unwrap();

    let mut ioapics = IOAPICS.call_once(|| RwLock::new(Vec::<IoApic>::new())).write();
    let io_apic = ioapics.iter_mut().find(|apic| (*apic).contains_gsi(*gsi))
	.unwrap_or_else(|| panic!("GSI {} not found", gsi));

    io_apic.enable_gsi(*gsi);
}
