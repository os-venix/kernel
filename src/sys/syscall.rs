use core::mem::offset_of;
use x86_64::registers::model_specific::Msr;
use x86_64::structures::tss::TaskStateSegment;
use core::ffi::CStr;
use alloc::string::String;
use x86_64::VirtAddr;

use crate::gdt;
use crate::scheduler;
use crate::sys::vfs;
use crate::memory;

const IA32_STAR_MSR: u32 = 0xC0000081;
const IA32_LSTAR_MSR: u32 = 0xC0000082;
const IA32_EFER_MSR: u32 = 0xC0000080;
const IA32_FSBASE_MSR: u32 = 0xC0000100;

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

fn do_syscall(rax: u64, rdi: u64, rsi: u64, rdx: u64, r10: u64, r8: u64, r9: u64) -> (u64, u64) {
    match rax {
	0x00 => {  // write
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
	0x01 => {  // read
	    let actual_fd = match scheduler::get_actual_fd(rdi) {
		Ok(fd) => fd,
		Err(_) => {
		    return (0xFFFF_FFFF_FFFF_FFFF, 9);  // -1 error, EBADF
		},
	    };

	    match vfs::read_by_fd(/* file descriptor= */ actual_fd, /* buf= */ rsi, /* count= */ rdx) {
		Ok(len) => (len, 0),
		Err(_) => (0xFFFF_FFFF_FFFF_FFFF, 5),  // -1 error, EIO
	    }
	},
	0x02 => {  // open
	    // TODO - this does not check that the file actually exists
	    let path = unsafe {
		match CStr::from_ptr(rdi as *const i8).to_str() {
		    Ok(path) => String::from(path),
		    Err(_) => {
			return (0xFFFF_FFFF_FFFF_FFFF, 22)  // -1 error, EINVAL
		    },
		}
	    };

	    // Bottom 3 bits are mode. We don't currently enforce mode, but in order to progress, let's strip it out
	    // TODO - support read/write/exec/etc modes
	    if rsi & 0xFFFF_FFFF_FFFF_FFF8 != 0 {
		log::info!("Open flags are 0x{:x}", rsi);
		unimplemented!();
	    }

	    if let Err(e) = vfs::stat(path.clone()) {
		// File does not exist
		log::info!("Could not stat {}", path);
		return (0xFFFF_FFFF_FFFF_FFFF, 13)  // -1 error, EACCESS
	    }

	    let fd = scheduler::open_fd(path);
	    (fd, 0)
	},
	0x03 => {  // close
	    return match scheduler::close_fd(rdi) {
		Ok(_) => (0, 0),
		Err(_) => (0xFFFF_FFFF_FFFF_FFFF, 9),  // -1 error, EBADF
	    }
	},
	0x08 => {  // seek
	    let actual_fd = match scheduler::get_actual_fd(rdi) {
		Ok(fd) => fd,
		Err(_) => {
		    return (0xFFFF_FFFF_FFFF_FFFF, 9);  // -1 error, EBADF
		},
	    };

	    // Valid values are SEEK_SET, SEEK_CUR, or SEEK_END
	    if rdx > 3 || rdx == 0 {
		return (0xFFFF_FFFF_FFFF_FFFF, 22);  // -1 error, EINVAL
	    }

	    match vfs::seek_fd(/* file descriptor= */ actual_fd, /* offset= */ rsi, /* whence= */ rdx) {
		Ok(offs) => (offs, 0),
		Err(_) => (0xFFFF_FFFF_FFFF_FFFF, 22)  // -1 error, EINVAL
	    }
	},
	0x09 => {  // mmap
	    if r8 != 0xFFFF_FFFF_FFFF_FFFF {
		unimplemented!();
	    }

	    let (start, _) = if rdi == 0 {
		match memory::kernel_allocate(
		    rsi,
		    memory::MemoryAllocationType::RAM,
		    memory::MemoryAllocationOptions::Arbitrary,
		    memory::MemoryAccessRestriction::User) {
		    Ok(i) => i,
		    Err(e) => panic!("Could not allocate memory for mmap: {:?}", e),
		}
	    } else {
		match memory::kernel_allocate(
		    rsi,
		    memory::MemoryAllocationType::RAM,
		    memory::MemoryAllocationOptions::Arbitrary,
		    memory::MemoryAccessRestriction::UserByStart(VirtAddr::new(rdi))) {
		    Ok(i) => i,
		    Err(e) => panic!("Could not allocate memory for mmap: {:?}", e),
		}		
	    };

	    (start.as_u64(), 0)
	},
	0x12c => {  // tcb_set	    
	    let mut fsbase_msr = Msr::new(IA32_FSBASE_MSR);
	    unsafe {
		fsbase_msr.write(rdi);
	    }

	    (0, 0)
	},
	_ => panic!("Invalid syscall 0x{:X}", rax),
    }
}

#[no_mangle]
unsafe extern "C" fn syscall_inner() {
    let mut rax: u64;
    let mut rdx: u64;
    let mut rcx: u64;
    let mut r11: u64;
    let mut rsi: u64;
    let mut rdi: u64;
    let mut r10: u64;
    let mut r8: u64;
    let mut r9: u64;

    core::arch::asm!(
	// TODO: load a kernel stack here

	"nop",

	out("rax") rax,
	out("rdx") rdx,
	out("rcx") rcx,
	out("r11") r11,
	out("rsi") rsi,
	out("rdi") rdi,
	out("r10") r10,
	out("r8") r8,
	out("r9") r9,
    );

    (rax, rdx) = do_syscall(rax, rdi, rsi, rdx, r10,  r8, r9);

    core::arch::asm!(
	"nop",

	in("r11") r11,
	in("rcx") rcx,
	in("rax") rax,
	in("rdx") rdx,
	in("rsi") rsi,
	in("rdi") rdi,
	in("r10") r10,
	in("r8") r8,
	in("r9") r9,
    );
}

// TODO - load kernel stack; may need to use swapgs for that
#[naked]
#[allow(named_asm_labels)]
unsafe extern "C" fn syscall_enter () -> ! {
    core::arch::asm!(
	"swapgs",
	"mov gs:[{sp}], rsp",
	"mov rsp, gs:[{ksp}]",
	
	"push r12",
	"push r13",
	"push r14",
	"push r15",
	"push rbx",
	"call syscall_inner",
	"pop rbx",
	"pop r15",
	"pop r14",
	"pop r13",
	"pop r12",

	"mov rsp, gs:[{sp}]",
	"swapgs",
	"sysretq",

	options(noreturn),
	sp = const(offset_of!(gdt::ProcessorControlBlock, tmp_user_stack_ptr)),
	ksp = const(offset_of!(gdt::ProcessorControlBlock, tss) + offset_of!(TaskStateSegment, privilege_stack_table)),
    );
}
