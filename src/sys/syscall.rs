use x86_64::registers::model_specific::Msr;

use crate::gdt;

const IA32_STAR_MSR: u32 = 0xC0000081;
const IA32_LSTAR_MSR: u32 = 0xC0000082;
const IA32_EFER_MSR: u32 = 0xC0000080;

const EFER_SCE: u64 = 1;

pub fn init() {
    let (kernel_code, user_code) = gdt::get_code_selectors();
    let star: u64 = (((user_code.0 as u64) << 16) | kernel_code.0 as u64) << 32;

    // Set the segment selectors
    let mut star_msr = Msr::new(IA32_STAR_MSR);
    unsafe {
	star_msr.write(star);
    }

    let mut lstar_msr = Msr::new(IA32_LSTAR_MSR);
    unsafe {
	lstar_msr.write(syscall_enter as u64);
    }

    let mut efer_msr = Msr::new(IA32_EFER_MSR);
    unsafe {
	let efer = efer_msr.read();
	efer_msr.write(efer | EFER_SCE);
    }
}

unsafe extern "C" fn syscall_enter() {
    let mut ecx: u64 = 0;
    core::arch::asm!(
	// TODO: load a kernel stack here

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

	out("ecx") ecx,
    );

    // TODO: do the actual syscall here

    log::info!("Syscall! Ret = 0x{:X}", ecx);

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
	"sysretq\n",
    ));
    
    panic!();
}
