use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use x86_64::registers::model_specific::Msr;
use x86_64::PhysAddr;
use lazy_static::lazy_static;
use raw_cpuid::CpuId;
use pic8259::ChainedPics;
use spin::{Once, RwLock};
use core::ptr::{read_volatile, write_volatile};
use ::acpi::platform::interrupt::{TriggerMode, Polarity};
use alloc::vec::Vec;
use alloc::format;

use crate::gdt;
use crate::memory;
use crate::sys::acpi;

const IA32_APIC_BASE_MSR: u32 = 0x1B;
const IA32_APIC_BASE_MSR_IS_BSP: u64 = 1 << 8;
const IA32_APIC_BASE_MSR_EXTD: u64 = 1 << 10;
const IA32_APIC_BASE_MSR_ENABLE: u64 = 1 << 11;

const IA32_X2APIC_SIVR: u32 = 0x80F;
const IA32_X2APIC_SIVR_VECTOR: u64 = 0xFF;
const IA32_X2APIC_SIVR_EN: u64 = 1 << 8;

const IA32_X2APIC_IDR: u32 = 0x802;
const IA32_X2APIC_EOI: u32 = 0x80B;

const PIC_1_OFFSET: u8 = 32;
const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

const IOAPICVER: u8 = 0x01;
const IOAPIC_MAX_REDIRECITON_ENTRY_MASK: u32 = 0x00FF_0000;
const IOAPIC_MAX_REDIRECTION_ENTRY_SHIFT: u32 = 16;

const IOAPIC_INTMASK_ENABLED: u32 = 0 << 16;
const IOAPIC_INTMASK_DISABLED: u32 = 1 << 16;

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
}

unsafe impl Send for IoApic {}
unsafe impl Sync for IoApic {}

impl IoApic {
    pub fn new(id: u8, base_addr: u32, global_system_interrupt_base: u32) -> IoApic {
	let size = 8;
	let start_phys_addr = base_addr - (base_addr % 4096);  // Page align
	let total_size = size + (base_addr % 4096) + (4096 - (size % 4096));  // Total amount, aligned to page boundaries

	let allocated_region = match memory::allocate_contiguous_region_kernel(
	    total_size as u64, PhysAddr::new(start_phys_addr as u64), memory::MemoryAllocationType::MMIO) {
	    Ok(v) => v,
	    Err(e) => panic!("{:#?}", e),
	};

	let offset_from_start = base_addr - start_phys_addr;
	let virt_addr = allocated_region + offset_from_start as u64;

	IoApic {
	    ioregsel: virt_addr.as_mut_ptr(),
	    iowin: (virt_addr + 0x10).as_mut_ptr(),
	    id: id,
	    global_system_interrupt_base: global_system_interrupt_base,
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

    pub fn map_interrupt(&self, gsi: u32, dest_apic: u32, trigger_mode: TriggerMode, polarity: Polarity, vector: u8) {
	let ioredtbl_hi = dest_apic << 24;
	let ioredtbl_lo = IOAPIC_INTMASK_ENABLED | match trigger_mode {
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
}

static PICS: RwLock<ChainedPics> = RwLock::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });
static IOAPICS: Once<RwLock<Vec<IoApic>>> = Once::new();

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
	let mut idt = InterruptDescriptorTable::new();
	idt.breakpoint.set_handler_fn(breakpoint_handler);
	idt.page_fault.set_handler_fn(page_fault_handler);
	idt.general_protection_fault.set_handler_fn(gpf_handler);
	unsafe {
	    idt.double_fault.set_handler_fn(double_fault_handler)
		.set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
	}

	// IDT interrutps
	idt[0x20].set_handler_fn(timer_handler);
	idt[0x21].set_handler_fn(keyboard_handler);
	idt[0x22].set_handler_fn(unknown_handler);
	idt[0x23].set_handler_fn(com2_handler);
	idt[0x24].set_handler_fn(com1_handler);
	idt[0x25].set_handler_fn(lpt2_handler);
	idt[0x26].set_handler_fn(fdd_handler);
	idt[0x27].set_handler_fn(spurious_interrupt_handler);
	idt[0x28].set_handler_fn(rtc_handler);
	idt[0x29].set_handler_fn(unknown_handler);
	idt[0x2A].set_handler_fn(unknown_handler);
	idt[0x2B].set_handler_fn(unknown_handler);
	idt[0x2C].set_handler_fn(mouse_handler);
	idt[0x2D].set_handler_fn(fpu_handler);
	idt[0x2E].set_handler_fn(primary_hdd_handler);
	idt[0x2F].set_handler_fn(secondary_hdd_handler);

	// APIC Spurious Interrupts
	idt[0xFF].set_handler_fn(spurious_interrupt_handler);

	idt
    };
}

pub fn init_idt() {
    IDT.load();
}

fn remap_pics() {
    let mut pics = PICS.write();

    unsafe {
	pics.initialize();
	pics.disable();
    }
}

fn find_ioapic(gsi: u32, ioapics: &Vec<IoApic>) -> Option<&IoApic> {
    ioapics.iter()
	.find(|apic| apic.contains_gsi(gsi))
}

pub fn init_bsp_apic() {
    let cpu_id = CpuId::new();
    let features = cpu_id.get_feature_info().expect("CPUID get features info failed.");

    // Check that we have an APIC
    if !features.has_apic() {
	panic!("System does not have a Local APIC. CPU not supported.");
    }

    if !features.has_x2apic() {
	panic!("System APIC does not support X2 mode. CPU not supported.");
    }

    {
	let acpi = acpi::ACPI.get().expect("Attempted to access ACPI tables before ACPI is initialised").read();
	let platform_info = match acpi.platform_info() {
	    Ok(pi) => pi,
	    Err(e) => panic!("{:#?}", e),
	};

	let interrupt_model = match platform_info.interrupt_model {
	    acpi::InterruptModel::Unknown => panic!("ACPI reports no APIC presence. CPU not supported."),
	    acpi::InterruptModel::Apic(a) => a,
	    _ => panic!("Unrecognised interrupt model."),
	};

	if interrupt_model.also_has_legacy_pics {
	    log::info!("Legacy PIC is present. Remapping.");
	    remap_pics();
	}
    }

    // Get the base address of the APIC
    let mut ia32_apic_base_msr = Msr::new(IA32_APIC_BASE_MSR);
    let base_msr_val = unsafe {
	ia32_apic_base_msr.read()
    };

    if base_msr_val & IA32_APIC_BASE_MSR_IS_BSP == 0 {
	panic!("Attempted to initialise BSP APIC on an AP");
    }

    // Enable the APIC in X2 mode
    unsafe {
	ia32_apic_base_msr.write(base_msr_val | IA32_APIC_BASE_MSR_ENABLE | IA32_APIC_BASE_MSR_EXTD);
    }

    // Enable the APIC using the Spurious Interrupt Vector Register
    let mut ia32_x2apic_sivr = Msr::new(IA32_X2APIC_SIVR);
    unsafe {
	ia32_x2apic_sivr.write(IA32_X2APIC_SIVR_VECTOR | IA32_X2APIC_SIVR_EN);
    }

    let ia32_x2apic_idr = Msr::new(IA32_X2APIC_IDR);
    let bsp_apic_id = unsafe {
	ia32_x2apic_idr.read()
    };

    {
	let acpi = acpi::ACPI.get().expect("Attempted to access ACPI tables before ACPI is initialised").read();
	let platform_info = match acpi.platform_info() {
	    Ok(pi) => pi,
	    Err(e) => panic!("{:#?}", e),
	};

	let interrupt_model = match platform_info.interrupt_model {
	    acpi::InterruptModel::Unknown => panic!("ACPI reports no APIC presence. CPU not supported."),
	    acpi::InterruptModel::Apic(a) => a,
	    _ => panic!("Unrecognised interrupt model."),
	};

	let mut ioapics = IOAPICS.call_once(|| RwLock::new(Vec::<IoApic>::new())).write();
	for io_apic in interrupt_model.io_apics.iter() {
	    log::info!("Found I/O APIC, ID=0x{:x}, GSI base=0x{:x}", io_apic.id, io_apic.global_system_interrupt_base);
	    ioapics.push(IoApic::new(io_apic.id, io_apic.address, io_apic.global_system_interrupt_base));
	}

	let interrupt_source_overrides = interrupt_model.interrupt_source_overrides;
	for over in interrupt_source_overrides.iter() {
	    find_ioapic(over.global_system_interrupt, &ioapics)
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
		    over.isa_source + PIC_1_OFFSET);
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

	    find_ioapic(legacy_irq as u32, &ioapics)
		.expect(
		    format!("Unable to find an I/O APIC for legacy IRQ {}", legacy_irq)
			.as_str())
		.map_interrupt(
		    legacy_irq as u32,
		    bsp_apic_id as u32,
		    TriggerMode::SameAsBus,
		    Polarity::SameAsBus,
		    legacy_irq + PIC_1_OFFSET);
	}
    }

    x86_64::instructions::interrupts::enable();
}

fn ack_apic() {
    let mut ia32_x2apic_eoi = Msr::new(IA32_X2APIC_EOI);
    unsafe {
	ia32_x2apic_eoi.write(0);
    }
}

// Faults
extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    log::warn!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _error_code: u64) -> ! {
    x86_64::instructions::interrupts::disable();
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn page_fault_handler(stack_frame: InterruptStackFrame, error_code: PageFaultErrorCode) {
    x86_64::instructions::interrupts::disable();
    panic!("EXCEPTION: PAGE FAULT {:#?}\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn gpf_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    x86_64::instructions::interrupts::disable();
    panic!("EXCEPTION: GPF error code 0x{:x}\n{:#?}", error_code, stack_frame);
}    

// IRQs
extern "x86-interrupt" fn spurious_interrupt_handler(_stack_frame: InterruptStackFrame) {
    log::info!("Spurious interrupt happened :-)");
}

extern "x86-interrupt" fn timer_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("Timer interrupt happened");
	ack_apic();
    });
}

extern "x86-interrupt" fn keyboard_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("Keyboard interrupt happened");
	ack_apic();
    });
}

extern "x86-interrupt" fn unknown_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("Unknown IRQ happened");
	ack_apic();
    });
}

extern "x86-interrupt" fn com2_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("COM2 IRQ happened");
	ack_apic();
    });
}

extern "x86-interrupt" fn com1_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("COM1 IRQ happened");
	ack_apic();
    });
}

extern "x86-interrupt" fn lpt2_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("LPT2 IRQ happened");
	ack_apic();
    });
}

extern "x86-interrupt" fn fdd_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("FDD IRQ happened");
	ack_apic();
    });
}

extern "x86-interrupt" fn rtc_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("RTC IRQ happened");
	ack_apic();
    });
}

extern "x86-interrupt" fn mouse_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("Mouse IRQ happened");
	ack_apic();
    });
}

extern "x86-interrupt" fn fpu_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("FPU IRQ happened");
	ack_apic();
    });
}

extern "x86-interrupt" fn primary_hdd_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("IDE1 IRQ happened");
	ack_apic();
    });
}

extern "x86-interrupt" fn secondary_hdd_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("IDE2 IRQ happened");
	ack_apic();
    });
}
