use x86_64::registers::model_specific::Msr;

use crate::gdt;
use crate::scheduler;
use crate::sys::vfs;

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

fn do_syscall(rax: u64, rdi: u64, rsi: u64, rdx: u64) -> (u64, u64) {
    match rax {
	0x00 => {
	    let actual_fd = match scheduler::get_actual_fd(rdi) {
		Ok(fd) => fd,
		Err(_) => {
		    return (0xFFFF_FFFF_FFFF_FFFF, 9);  // -1 error, EBADF
		},
	    };

	    match vfs::write_by_fd(/* file descriptor= */ actual_fd, /* buf= */ rsi, /* count= */ rdx) {
		Ok(len) => (len, 0),
		Err(_) => (0xFFFF_FFFF_FFFF_FFFF, 5),  // -1 error, EIO
	    }
	},
	_ => panic!("Invalid syscall 0x{:X}", rax),
    }
}

unsafe extern "C" fn syscall_enter() -> ! {
    let mut rax: u64 = 0;
    let mut rdx: u64 = 0;
    let mut rcx: u64 = 0;
    let mut r11: u64 = 0;
    let mut rsi: u64 = 0;
    let mut rdi: u64 = 0;

    core::arch::asm!(
	// TODO: load a kernel stack here

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
	out("rsi") rsi,
	out("rdi") rdi,
    );

    (rax, rdx) = do_syscall(rax, rdi, rsi, rdx);

    core::arch::asm!(
	"pop r15",
	"pop r14",
	"pop r13",
	"pop r12",
	"pop r10",
	"pop r9",
	"pop r8",
	"pop rbp",
	"sysretq",

	in("r11") r11,
	in("rcx") rcx,
	in("rax") rax,
	in("rdx") rdx,
	in("rsi") rsi,
	in("rdi") rdi,
    );
    
    panic!();
}
