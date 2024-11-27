use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use lazy_static::lazy_static;
use alloc::collections::btree_map::BTreeMap;
use alloc::vec::Vec;
use spin::{Once, RwLock};
use alloc::boxed::Box;

use crate::interrupts::local_apic;
use crate::gdt;

macro_rules! irq_handler_def {
    ($irq:literal) => {
	paste::item! {
	    extern "x86-interrupt" fn [<irq_ $irq >] (_stack_frame: InterruptStackFrame) {
		unsafe {
		    core::arch::asm!(concat!(
			"push rax\n",
			"push rbx\n",
			"push rcx\n",
			"push rdx\n",
			"push rsi\n",
			"push rdi\n",
			"push rbp\n",
			"push r8\n",
			"push r9\n",
			"push r10\n",
			"push r11\n",
			"push r12\n",
			"push r13\n",
			"push r14\n",
			"push r15\n",
		    ));
		}

		local_apic::ack_apic();

		{
		    let handler_funcs = HANDLER_FUNCS.get().expect("Handler funcs not initialised").read();
		    match handler_funcs.get(&$irq) {
			Some(h) => for func in h.iter() {
			    (func)();
			},
			None => (),
		    }
		}
		
		unsafe {
		    core::arch::asm!(concat!(
			"pop r15\n",
			"pop r14\n",
			"pop r13\n",
			"pop r12\n",
			"pop r11\n",
			"pop r10\n",
			"pop r9\n",
			"pop r8\n",
			"pop rbp\n",
			"pop rdi\n",
			"pop rsi\n",
			"pop rdx\n",
			"pop rcx\n",
			"pop rbx\n",
			"pop rax\n",
		    ));
		}
	    }
	}
    };
}

static HANDLER_FUNCS: Once<RwLock<BTreeMap<u8, Vec<Box<(dyn Fn() + Send + Sync)>>>>> = Once::new();

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
	let mut idt = InterruptDescriptorTable::new();
	idt.divide_error.set_handler_fn(divide_error_handler);
	idt.debug.set_handler_fn(debug_handler);
	idt.non_maskable_interrupt.set_handler_fn(nmi_handler);
	idt.overflow.set_handler_fn(overflow_handler);
	idt.bound_range_exceeded.set_handler_fn(bound_range_exceeded_handler);
	idt.device_not_available.set_handler_fn(device_not_available_handler);
	idt.invalid_tss.set_handler_fn(invalid_tss_handler);
	idt.breakpoint.set_handler_fn(breakpoint_handler);
	idt.page_fault.set_handler_fn(page_fault_handler);
	idt.general_protection_fault.set_handler_fn(gpf_handler);
	idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
	idt.segment_not_present.set_handler_fn(segment_not_present_handler);
	idt.stack_segment_fault.set_handler_fn(stack_segment_handler);
	unsafe {
	    idt.double_fault.set_handler_fn(double_fault_handler)
		.set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
	}

	// IDT interrutps
	idt[0x20].set_handler_fn(irq_32);
	idt[0x21].set_handler_fn(irq_33);
	idt[0x22].set_handler_fn(irq_34);
	idt[0x23].set_handler_fn(irq_35);
	idt[0x24].set_handler_fn(irq_36);
	idt[0x25].set_handler_fn(irq_37);
	idt[0x26].set_handler_fn(irq_38);
	idt[0x27].set_handler_fn(irq_39);
	idt[0x28].set_handler_fn(irq_40);
	idt[0x29].set_handler_fn(irq_41);
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

pub fn init_handlers() {
    HANDLER_FUNCS.call_once(|| RwLock::new(BTreeMap::<u8, Vec::<Box<(dyn Fn() + Send + Sync)>>>::new()));
}

pub fn add_handler_to_irq(irq: u8, handler: Box<(dyn Fn() + Send + Sync)>) {
    let mut handler_funcs = HANDLER_FUNCS.get().expect("Handler funcs have not been initialised").write();

    if let Some(v) = handler_funcs.get_mut(&irq) {
	v.push(handler);
    } else {
	let mut v = Vec::<Box<(dyn Fn() + Send + Sync)>>::new();
	v.push(handler);
	handler_funcs.insert(irq, v);
    }
}

// Faults
extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
    log::warn!("EXCEPTION: #DE\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn debug_handler(stack_frame: InterruptStackFrame) {
    panic!("EXCEPTION: #DB\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn nmi_handler(stack_frame: InterruptStackFrame) {
//    log::warn!("EXCEPTION: #NMI\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn overflow_handler(stack_frame: InterruptStackFrame) {
//    log::warn!("EXCEPTION: #OF\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
//    log::warn!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn bound_range_exceeded_handler(stack_frame: InterruptStackFrame) {
//    log::warn!("EXCEPTION: #BR\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn device_not_available_handler(stack_frame: InterruptStackFrame) {
//    log::warn!("EXCEPTION: #NM\n{:#?}", stack_frame);
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

extern "x86-interrupt" fn segment_not_present_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    x86_64::instructions::interrupts::disable();
    panic!("EXCEPTION: NP error code 0x{:x}\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn stack_segment_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    x86_64::instructions::interrupts::disable();
    panic!("EXCEPTION: SS error code 0x{:x}\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn invalid_tss_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    x86_64::instructions::interrupts::disable();
    panic!("EXCEPTION: #TS error code 0x{:x}\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
//    log::warn!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

// IRQs
extern "x86-interrupt" fn spurious_interrupt_handler(_stack_frame: InterruptStackFrame) {
    log::info!("Spurious interrupt happened :-)");
}

irq_handler_def!(32);
irq_handler_def!(33);
irq_handler_def!(34);
irq_handler_def!(35);
irq_handler_def!(36);
irq_handler_def!(37);
irq_handler_def!(38);
irq_handler_def!(39);
irq_handler_def!(40);
irq_handler_def!(41);

extern "x86-interrupt" fn unknown_handler(_stack_frame: InterruptStackFrame) {
    x86_64::instructions::interrupts::without_interrupts(|| {
	log::info!("Unknown IRQ happened");
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
