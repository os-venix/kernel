use core::ffi::{c_char, CStr};
use x86_64::registers::model_specific::Msr;

use crate::gdt;

const IA32_STAR_MSR: u32 = 0xC0000081;
const IA32_LSTAR_MSR: u32 = 0xC0000082;
const IA32_EFER_MSR: u32 = 0xC0000080;

const EFER_SCE: u64 = 1;

pub fn init() {
    let (kernel_code, user_code) = gdt::get_code_selectors();
    let star: u64 = (((user_code.0 as u64 | 3) << 16) | kernel_code.0 as u64) << 32;

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
    let mut rax: u64 = 0;
    let mut rdx: u64 = 0;
    let mut rcx: u64 = 0;
    let mut r11: u64 = 0;

    core::arch::asm!(
	// TODO: load a kernel stack here

	"push rdx",
	"push rsi",
	"push rdi",
	"push rbp",
	"push r8",
	"push r9",
	"push r10",
	"push r12",
	"push r13",
	"push r14",
	"push r15",

	out("rax") rax,
	out("rdx") rdx,
	out("rcx") rcx,
	out("r11") r11,
    );

    // TODO: do the actual syscall here
    match rax {
	0x01 => {
	    let s = CStr::from_ptr(rdx as *const c_char);
	    log::info!("{}", s.to_str().expect("Unable to decode CStr"));
	},
	_ => log::info!("Syscall! Ret = 0x{:X}, FLAGS = 0x{:X}", rcx, r11),
    }

    core::arch::asm!(
	"pop r15",
	"pop r14",
	"pop r13",
	"pop r12",
	"pop r10",
	"pop r9",
	"pop r8",
	"pop rbp",
	"pop rdi",
	"pop rsi",
	"pop rdx",
	"sysretq",

	in("r11") r11,
	in("rcx") rcx,
	in("rax") rax,
	in("rdx") rdx,
    );
    
    panic!();
}
