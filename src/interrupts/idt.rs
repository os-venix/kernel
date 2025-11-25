use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use lazy_static::lazy_static;
use alloc::collections::btree_map::BTreeMap;
use alloc::vec::Vec;
use alloc::vec;
use spin::{Once, RwLock};
use alloc::boxed::Box;

use crate::interrupts::local_apic;
use crate::gdt;
use crate::scheduler;
use crate::process;

#[repr(C)]
#[derive(Debug)]
struct StackFrame {
    registers: process::GeneralPurposeRegisters,
    stack_frame: InterruptStackFrame,
}

macro_rules! irq_handler_def {
    ($irq:literal) => {
	paste::item! {
	    unsafe fn [<install_irq_ $irq>](idt: &mut x86_64::structures::idt::InterruptDescriptorTable) {
		let f: extern "x86-interrupt" fn(InterruptStackFrame) = core::mem::transmute([<irq_ $irq>] as usize);
		idt[$irq].set_handler_fn(f);
	    }

	    #[naked]
	    #[allow(named_asm_labels)]
	    extern "C" fn [<irq_ $irq >] () {
		extern "C" fn inner(stack_frame: &StackFrame) -> ! {
		    let process = scheduler::get_current_process();
		    process.set_registers(
			stack_frame.stack_frame.stack_pointer.as_u64(),
			stack_frame.stack_frame.instruction_pointer.as_u64(),
			stack_frame.stack_frame.cpu_flags.bits(),
			&stack_frame.registers);

		    if stack_frame.stack_frame.stack_pointer.as_u64() >= *gdt::IST_FRAME.get().expect(":(") &&
			stack_frame.stack_frame.stack_pointer.as_u64() <= *gdt::IST_FRAME.get().expect(":(") + (1024 * 1024 * 8) {
			    panic!("Re-entrant IRQ");
			}

		    local_apic::ack_apic($irq);

		    {
			let handler_funcs = HANDLER_FUNCS.get().expect("Handler funcs not initialised").read();
			match handler_funcs.get(&$irq) {
			    Some(h) => for func in h.iter() {
				let func_ptr = &*func as *const _ as *const () as usize;

				if func_ptr < 0x1000 {
				    panic!("Bad IRQ handler");
				}

				(func)();
			    },
			    None => (),
			}
		    }

		    scheduler::schedule_next();
		}

		unsafe {
		    core::arch::asm!(
			"cli",

			"test qword ptr [rsp + 0x08], 0x03",
			"je 2f",
			"swapgs",
			"2:",

			"push rax",
			"push rbx",
			"push rcx",
			"push rdx",
			"push rsi",
			"push rdi",
			"push rbp",
			"push r8",
			"push r9",
			"push r10",
			"push r11",
			"push r12",
			"push r13",
			"push r14",
			"push r15",

			"mov rdi, rsp",
			"call {inner}",

			inner = sym inner,
			options(noreturn),
		    );
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
	idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
	idt.segment_not_present.set_handler_fn(segment_not_present_handler);
	idt.stack_segment_fault.set_handler_fn(stack_segment_handler);

	unsafe {
	    idt.general_protection_fault.set_handler_fn(gpf_handler)
		.set_stack_index(gdt::KERNEL_IST_INDEX);
	    idt.double_fault.set_handler_fn(double_fault_handler)
		.set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
	    idt.page_fault.set_handler_fn(page_fault_handler)
		.set_stack_index(gdt::KERNEL_IST_INDEX);

	    // IDT interrutps
	    install_irq_32(&mut idt);
	    install_irq_33(&mut idt);
	    install_irq_34(&mut idt);
	    install_irq_35(&mut idt);
	    install_irq_36(&mut idt);
	    install_irq_37(&mut idt);
	    install_irq_38(&mut idt);
	    install_irq_39(&mut idt);
	    install_irq_40(&mut idt);
	    install_irq_41(&mut idt);
	    install_irq_42(&mut idt);
	    install_irq_43(&mut idt);
	    install_irq_44(&mut idt);
	    install_irq_45(&mut idt);
	    install_irq_46(&mut idt);
	    install_irq_47(&mut idt);
	}

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
	let mut v = vec![handler];
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
    log::warn!("EXCEPTION: #NMI\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn overflow_handler(stack_frame: InterruptStackFrame) {
    log::warn!("EXCEPTION: #OF\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    log::warn!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn bound_range_exceeded_handler(stack_frame: InterruptStackFrame) {
    log::warn!("EXCEPTION: #BR\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn device_not_available_handler(stack_frame: InterruptStackFrame) {
    log::warn!("EXCEPTION: DEVICE NOT AVAILABLE\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _error_code: u64) -> ! {
    x86_64::instructions::interrupts::disable();
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn page_fault_handler(stack_frame: InterruptStackFrame, error_code: PageFaultErrorCode) {
    x86_64::instructions::interrupts::disable();
    let target_addr = x86_64::registers::control::Cr2::read_raw();
    panic!("EXCEPTION: PAGE FAULT\nADDR 0x{:x}\n{:#?}\n{:#?}", target_addr, error_code, stack_frame);
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
    panic!("EXCEPTION: #UD\n{:#?}", stack_frame);
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
irq_handler_def!(42);
irq_handler_def!(43);
irq_handler_def!(44);
irq_handler_def!(45);
irq_handler_def!(46);
irq_handler_def!(47);
