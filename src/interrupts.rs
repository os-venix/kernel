use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};
use x86_64::registers::model_specific::Msr;
use lazy_static::lazy_static;
use raw_cpuid::CpuId;

use crate::gdt;

const IA32_APIC_BASE_MSR: u32 = 0x1B;
const IA32_APIC_BASE_MSR_IS_BSP: u64 = 1 << 8;
const IA32_APIC_BASE_MSR_EXTD: u64 = 1 << 10;
const IA32_APIC_BASE_MSR_ENABLE: u64 = 1 << 11;

const IA32_X2APIC_SIVR: u32 = 0x80F;
const IA32_X2APIC_SIVR_VECTOR: u64 = 0xFF;
const IA32_X2APIC_SIVR_EN: u64 = 1 << 8;

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
	let mut idt = InterruptDescriptorTable::new();
	idt.breakpoint.set_handler_fn(breakpoint_handler);
	unsafe {
	    idt.double_fault.set_handler_fn(double_fault_handler)
		.set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
	}
	idt[0xFF].set_handler_fn(spurious_interrupt_handler);

	idt
    };
}

pub fn init_idt() {
    IDT.load();
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
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    log::warn!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _error_code: u64) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn spurious_interrupt_handler(_stack_frame: InterruptStackFrame) {
    log::info!("Spurious interrupt happened :-)");
}
