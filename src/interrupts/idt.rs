use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use lazy_static::lazy_static;

use crate::interrupts::local_apic;
use crate::gdt;


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

pub fn init() {
    IDT.load();
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
	local_apic::ack_apic();
    });
}

extern "x86-interrupt" fn keyboard_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("Keyboard interrupt happened");
	local_apic::ack_apic();
    });
}

extern "x86-interrupt" fn unknown_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("Unknown IRQ happened");
	local_apic::ack_apic();
    });
}

extern "x86-interrupt" fn com2_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("COM2 IRQ happened");
	local_apic::ack_apic();
    });
}

extern "x86-interrupt" fn com1_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("COM1 IRQ happened");
	local_apic::ack_apic();
    });
}

extern "x86-interrupt" fn lpt2_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("LPT2 IRQ happened");
	local_apic::ack_apic();
    });
}

extern "x86-interrupt" fn fdd_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("FDD IRQ happened");
	local_apic::ack_apic();
    });
}

extern "x86-interrupt" fn rtc_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("RTC IRQ happened");
	local_apic::ack_apic();
    });
}

extern "x86-interrupt" fn mouse_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("Mouse IRQ happened");
	local_apic::ack_apic();
    });
}

extern "x86-interrupt" fn fpu_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("FPU IRQ happened");
	local_apic::ack_apic();
    });
}

extern "x86-interrupt" fn primary_hdd_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("IDE1 IRQ happened");
	local_apic::ack_apic();
    });
}

extern "x86-interrupt" fn secondary_hdd_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("IDE2 IRQ happened");
	local_apic::ack_apic();
    });
}
