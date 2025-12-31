use core::mem::offset_of;
use x86_64::structures::tss::TaskStateSegment;
use alloc::boxed::Box;
use core::future::Future;
use core::pin::Pin;
use alloc::sync::Arc;
use spin::Mutex;

use crate::gdt;
use crate::scheduler;
use crate::process;
use crate::sys::syscall::SyscallResult;

#[no_mangle]
unsafe extern "C" fn kasync_inner(stack_frame: process::GeneralPurposeRegisters) -> ! {
    let rsp: u64;
    core::arch::asm!(
	"mov {rsp}, gs:[{sp}]",
	rsp = out(reg) rsp,
	sp = const(offset_of!(gdt::ProcessorControlBlock, tmp_user_stack_ptr)),
    );

    let rip = stack_frame.rcx;

    let process = scheduler::get_current_process();
    process.clone().set_registers(rsp, rip, stack_frame.r11, &stack_frame);

    let raw = stack_frame.rdi as *mut Pin<Box<dyn Future<Output = SyscallResult> + Send>>;

    let fut: Pin<Box<dyn Future<Output = SyscallResult> + Send>> =
        *Box::from_raw(raw);

    process.set_state(process::TaskState::AsyncSyscall {
	future: Arc::new(Mutex::new(fut)),
    });

    scheduler::schedule_next();
}

#[allow(named_asm_labels)]
pub unsafe fn do_kasync(
    fut: Pin<Box<dyn Future<Output = SyscallResult> + Send + 'static>>) -> SyscallResult {
    let ret: u64;
    let err: u64;
    
    // Decompose the fat pointer explicitly
    let raw = Box::into_raw(Box::new(fut));

    core::arch::asm!(
	// Set up state: clear interrupts, save the return address and flags, set syscall stack
	"cli",
	"lea rcx, [rip + 2f]",
	"pushfq",
	"pop r11",

	// Save the stack pointer (note that, because this is a kthread, swapping gs is unnecessary
	"mov gs:[{sp}], rsp",
	"mov rsp, gs:[{ksp}]",

	// Actually syscall
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
	"call kasync_inner",

	"2:",
	"nop",

        in("rdi") raw,

        lateout("rax") ret,
	lateout("rdx") err,
        lateout("rcx") _,
        lateout("r11") _,

	sp = const(offset_of!(gdt::ProcessorControlBlock, tmp_user_stack_ptr)),
	ksp = const(offset_of!(gdt::ProcessorControlBlock, tss) + offset_of!(TaskStateSegment, privilege_stack_table)),
    );

    SyscallResult {
        return_value: ret,
        err_num: err,
    }
}
