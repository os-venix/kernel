use core::mem::offset_of;
use x86_64::structures::tss::TaskStateSegment;
use core::ffi::CStr;
use alloc::string::String;
use x86_64::VirtAddr;
use x86_64::registers::model_specific::{FsBase, Efer, EferFlags, SFMask, Star, LStar};
use x86_64::registers::rflags::RFlags;
use alloc::vec::Vec;

use crate::gdt;
use crate::scheduler;
use crate::sys::vfs;
use crate::memory;

pub fn init() {
    let (kernel_code, kernel_data, user_code, user_data) = gdt::get_code_selectors();

    Star::write(user_code, user_data, kernel_code, kernel_data).expect("Unable to set STAR");
    LStar::write(VirtAddr::new(syscall_enter as u64));

    unsafe {
	Efer::update(|old_flags| *old_flags |= EferFlags::SYSTEM_CALL_EXTENSIONS);
    }

    SFMask::write(RFlags::INTERRUPT_FLAG |
		  RFlags::DIRECTION_FLAG |
		  RFlags::TRAP_FLAG |
		  RFlags::ALIGNMENT_CHECK);
}

fn do_syscall(rax: u64, rdi: u64, rsi: u64, rdx: u64, _r10: u64, r8: u64, _r9: u64, rcx: u64) -> (u64, u64) {
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

	    if let Err(_) = vfs::stat(path.clone()) {
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
	0x0c => {  // exit
	    scheduler::exit()	    
	},
	0x39 => {  // fork
	    let pid = scheduler::fork_current_process(rcx);
	    (pid, 0)
	},
	0x3b => {  // execve
	    let path = unsafe {
		match CStr::from_ptr(rdi as *const i8).to_str() {
		    Ok(path) => String::from(path),
		    Err(_) => {
			return (0xFFFF_FFFF_FFFF_FFFF, 22)  // -1 error, EINVAL
		    },
		}
	    };
	    let args = unsafe {
		let mut args: Vec<String> = Vec::new();
		let mut argc_ptr = rsi as *const u64;
		while *argc_ptr != 0 {
		    let arg = CStr::from_ptr(*argc_ptr as u64 as *const i8);

		    match arg.to_str() {
			Ok(a) => args.push(String::from(a)),
			Err(_) => {
			    return (0xFFFF_FFFF_FFFF_FFFF, 22)  // -1 error, EINVAL
			},
		    }

		    argc_ptr = (rsi + 8) as *const u64;
		}

		args
	    };
	    let envvars = unsafe {
		let mut envvars: Vec<String> = Vec::new();

		let mut envvar_ptr = rdx as *const u64;
		while *envvar_ptr != 0 {
		    let envvar = CStr::from_ptr(*envvar_ptr as u64 as *const i8);

		    match envvar.to_str() {
			Ok(a) => envvars.push(String::from(a)),
			Err(_) => {
			    return (0xFFFF_FFFF_FFFF_FFFF, 22)  // -1 error, EINVAL
			},
		    }

		    envvar_ptr = (rdx + 8) as *const u64;
		}

		envvars
	    };
	    scheduler::execve(path, args, envvars);

	    (0, 0)
	},
	0x12c => {  // tcb_set
	    FsBase::write(VirtAddr::new(rdi));
	    (0, 0)
	},
	_ => panic!("Invalid syscall 0x{:X}", rax),
    }
}

#[no_mangle]
unsafe extern "C" fn syscall_inner(mut stack_frame: scheduler::GeneralPurposeRegisters) {
    let rsp: u64;
    core::arch::asm!(
	"mov {rsp}, gs:[{sp}]",
	rsp = out(reg) rsp,
	sp = const(offset_of!(gdt::ProcessorControlBlock, tmp_user_stack_ptr)),
    );

    let rax = stack_frame.rax;
    let rdx = stack_frame.rdx;
    stack_frame.rax = 0;
    stack_frame.rdx = 0;

    scheduler::set_registers_for_current_process(rsp, stack_frame.rcx, &mut stack_frame);

    let (rax, rdx) = do_syscall(
	rax,
	stack_frame.rdi,
	stack_frame.rsi,
	rdx,
	stack_frame.r10,
	stack_frame.r8,
	stack_frame.r9,
	stack_frame.rcx);
    
    let (rsp, rip) = scheduler::get_registers_for_current_process(&mut stack_frame);
    core::arch::asm!(
	"mov gs:[{sp}], {rsp}",
	sp = const(offset_of!(gdt::ProcessorControlBlock, tmp_user_stack_ptr)),
	rsp = in(reg) rsp,
    );
    
    stack_frame.rax = rax;
    stack_frame.rcx = rip;
    stack_frame.rdx = rdx;
}

// TODO - load kernel stack; may need to use swapgs for that
#[naked]
#[allow(named_asm_labels)]
unsafe extern "C" fn syscall_enter () -> ! {
    core::arch::asm!(
	"swapgs",
	"mov gs:[{sp}], rsp",
	"mov rsp, gs:[{ksp}]",

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
	"call syscall_inner",

	"pop r15",
	"pop r14",
	"pop r13",
	"pop r12",
	"pop r11",
	"pop r10",
	"pop r9",
	"pop r8",
	"pop rbp",
	"pop rdi",
	"pop rsi",
	"pop rdx",
	"pop rcx",
	"pop rbx",
	"pop rax",

	"mov rsp, gs:[{sp}]",
	"swapgs",
	"sysretq",

	options(noreturn),
	sp = const(offset_of!(gdt::ProcessorControlBlock, tmp_user_stack_ptr)),
	ksp = const(offset_of!(gdt::ProcessorControlBlock, tss) + offset_of!(TaskStateSegment, privilege_stack_table)),
    );
}
