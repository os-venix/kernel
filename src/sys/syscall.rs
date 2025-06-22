use core::mem::offset_of;
use x86_64::structures::tss::TaskStateSegment;
use core::ffi::CStr;
use alloc::string::String;
use x86_64::VirtAddr;
use x86_64::registers::model_specific::{FsBase, Efer, EferFlags, SFMask, Star, LStar};
use x86_64::registers::rflags::RFlags;
use alloc::vec::Vec;
use core::error::Error;
use alloc::fmt;
use alloc::boxed::Box;
use core::future::Future;
use core::pin::Pin;
use alloc::sync::Arc;
use spin::Mutex;

use crate::gdt;
use crate::scheduler;
use crate::sys::vfs;
use crate::memory;

#[repr(u64)]
#[derive(Debug)]
pub enum CanonicalError {
    EOK = 0,
    ENOENT = 2,
    EIO = 5,
    EBADF = 9,
    EAGAIN = 11,
    EACCESS = 13,
    EINVAL = 22,
}

impl fmt::Display for CanonicalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
	write!(f, "Got error {:?}", self)
    }
}

impl Error for CanonicalError { }

pub struct SyscallResult {
    pub return_value: u64,
    pub err_num: u64,
}

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

async fn sys_write(fd: u64, buf: u64, count: u64) -> SyscallResult {
    if buf == 0 {
	log::info!("NULL");

	return SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::EIO as u64
	};
    }

    let actual_fd = match scheduler::get_actual_fd(fd) {
	Ok(fd) => fd,
	Err(_) => {
	    return SyscallResult {
		return_value: 0xFFFF_FFFF_FFFF_FFFF,
		err_num: CanonicalError::EBADF as u64
	    };
	},
    };

    match vfs::write_by_fd(actual_fd, buf, count) {
	Ok(len) => SyscallResult {
	    return_value: len,
	    err_num: CanonicalError::EOK as u64,
	},
	Err(_) => SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::EIO as u64
	},
    }
}

async fn sys_read(fd: u64, buf: u64, count: u64) -> SyscallResult {
    let actual_fd = match scheduler::get_actual_fd(fd) {
	Ok(fd) => fd,
	Err(_) => {
	    return SyscallResult {
		return_value: 0xFFFF_FFFF_FFFF_FFFF,
		err_num: CanonicalError::EBADF as u64
	    };
	},
    };

    match vfs::read_by_fd(actual_fd, buf, count).await {
	Ok(len) => SyscallResult {
	    return_value: len,
	    err_num: CanonicalError::EOK as u64,
	},
	Err(_) => SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::EIO as u64
	},
    }
}

pub async fn sys_open(path_ptr: u64, flags: u64) -> SyscallResult {
    // TODO - this does not check that the file actually exists
    let path = unsafe {
	match CStr::from_ptr(path_ptr as *const i8).to_str() {
	    Ok(path) => String::from(path),
	    Err(_) => {
		return SyscallResult {
		    return_value: 0xFFFF_FFFF_FFFF_FFFF,
		    err_num: CanonicalError::EINVAL as u64
		};
	    },
	}
    };

    // Bottom 3 bits are mode. We don't currently enforce mode, but in order to progress, let's strip it out
    // TODO - support read/write/exec/etc modes
    if flags & 0xFFFF_FFFF_FFFF_FFF8 != 0 {
	log::info!("Open flags are 0x{:x}", flags);
	unimplemented!();
    }

    if let Err(_) = vfs::stat(path.clone()).await {
	// File does not exist
	log::info!("Could not stat {}", path);
	return SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::EACCESS as u64
	};
    }

    let fd = scheduler::open_fd(path);
    SyscallResult {
	return_value: fd,
	err_num: CanonicalError::EOK as u64,
    }
}

async fn sys_close(fd: u64) -> SyscallResult {
    return match scheduler::close_fd(fd) {
	Ok(_) => SyscallResult {
	    return_value: 0,
	    err_num: 0,
	},
	Err(_) => SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::EBADF as u64
	}
    }
}

async fn sys_ioctl(fd_num: u64, ioctl: u64, buf: u64) -> SyscallResult {
    let actual_fd = match scheduler::get_actual_fd(fd_num) {
	Ok(fd) => fd,
	Err(_) => {
	    return SyscallResult {
		return_value: 0xFFFF_FFFF_FFFF_FFFF,
		err_num: CanonicalError::EBADF as u64
	    };
	},
    };

    match vfs::ioctl_by_fd(actual_fd, ioctl, buf) {
	Ok(ret) => SyscallResult {
	    return_value: ret,
	    err_num: CanonicalError::EOK as u64,
	},
	Err(_) => SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::EIO as u64
	},
    }
}

async fn sys_stat(filename: u64, buf: u64) -> SyscallResult {
    let path = unsafe {
	match CStr::from_ptr(filename as *const i8).to_str() {
	    Ok(path) => String::from(path),
	    Err(_) => {
		return SyscallResult {
		    return_value: 0xFFFF_FFFF_FFFF_FFFF,
		    err_num: CanonicalError::EINVAL as u64
		};
	    },
	}
    };

    match vfs::stat(path).await {
	Ok(ret) => SyscallResult {
	    return_value: 0,
	    err_num: CanonicalError::EOK as u64
	},
	Err(e) => SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: e as u64
	}
    }
}

async fn sys_seek(fd_num: u64, offset: u64, whence: u64) -> SyscallResult {
    let actual_fd = match scheduler::get_actual_fd(fd_num) {
	Ok(fd) => fd,
	Err(_) => {
	    return SyscallResult {
		return_value: 0xFFFF_FFFF_FFFF_FFFF,
		err_num: CanonicalError::EBADF as u64
	    };
	},
    };

    // Valid values are SEEK_SET, SEEK_CUR, or SEEK_END
    if whence > 3 || whence == 0 {
	return SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::EINVAL as u64
	};
    }

    match vfs::seek_fd(actual_fd, offset, whence).await {
	Ok(offs) => SyscallResult {
	    return_value: offs,
	    err_num: CanonicalError::EOK as u64,
	},
	Err(_) => SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::EINVAL as u64
	},
    }
}

async fn sys_mmap(start_val: u64, count: u64, r8: u64) -> SyscallResult {
    // TODO: properly pass parameters, and properly name them. There's a lot of unimplemented stuff here
    if r8 != 0xFFFF_FFFF_FFFF_FFFF {
	unimplemented!();
    }

    let (start, _) = if start_val == 0 {
	match memory::kernel_allocate(
	    count,
	    memory::MemoryAllocationType::RAM,
	    memory::MemoryAllocationOptions::Arbitrary,
	    memory::MemoryAccessRestriction::User) {
	    Ok(i) => i,
	    Err(e) => panic!("Could not allocate memory for mmap: {:?}", e),
	}
    } else {
	match memory::kernel_allocate(
	    count,
	    memory::MemoryAllocationType::RAM,
	    memory::MemoryAllocationOptions::Arbitrary,
	    memory::MemoryAccessRestriction::UserByStart(VirtAddr::new(start_val))) {
	    Ok(i) => i,
	    Err(e) => panic!("Could not allocate memory for mmap: {:?}", e),
	}		
    };

    SyscallResult {
	return_value: start.as_u64(),
	err_num: CanonicalError::EOK as u64,
    }
}

async fn sys_fork(start: u64) -> SyscallResult {
    let pid = scheduler::fork_current_process(start);
    SyscallResult {
	return_value: pid,
	err_num: CanonicalError::EOK as u64,
    }
}

pub async fn sys_execve(path_ptr: u64, args_ptr: u64, envvars_ptr: u64) -> SyscallResult {
    let path = unsafe {
	match CStr::from_ptr(path_ptr as *const i8).to_str() {
	    Ok(path) => String::from(path),
	    Err(_) => {
		return SyscallResult {
		    return_value: 0xFFFF_FFFF_FFFF_FFFF,
		    err_num: CanonicalError::EINVAL as u64
		};
	    },
	}
    };
    let args = unsafe {
	let mut args: Vec<String> = Vec::new();
	let mut argc_ptr = args_ptr as *const u64;
	while *argc_ptr != 0 {
	    let arg = CStr::from_ptr(*argc_ptr as u64 as *const i8);

	    match arg.to_str() {
		Ok(a) => args.push(String::from(a)),
		Err(_) => {
		    return SyscallResult {
			return_value: 0xFFFF_FFFF_FFFF_FFFF,
			err_num: CanonicalError::EINVAL as u64
		    };
		},
	    }

	    argc_ptr = (args_ptr + 8) as *const u64;
	}

	args
    };
    let envvars = unsafe {
	let mut envvars: Vec<String> = Vec::new();

	let mut envvar_ptr = envvars_ptr as *const u64;
	while *envvar_ptr != 0 {
	    let envvar = CStr::from_ptr(*envvar_ptr as u64 as *const i8);

	    match envvar.to_str() {
		Ok(a) => envvars.push(String::from(a)),
		Err(_) => {
		    return SyscallResult {
			return_value: 0xFFFF_FFFF_FFFF_FFFF,
			err_num: CanonicalError::EINVAL as u64
		    };
		},
	    }

	    envvar_ptr = (args_ptr + 8) as *const u64;
	}

	envvars
    };
    scheduler::execve(path, args, envvars).await;

    SyscallResult {
	return_value: 0,
	err_num: CanonicalError::EOK as u64,
    }
}

async fn sys_tcb_set(new_fs: u64) -> SyscallResult {
    FsBase::write(VirtAddr::new(new_fs));
    SyscallResult {
	return_value: 0,
	err_num: 0,
    }
}

fn do_syscall(rax: u64, rdi: u64, rsi: u64, rdx: u64, _r10: u64, r8: u64, _r9: u64, rcx: u64) -> Pin<Box<dyn Future<Output = SyscallResult> + Send + 'static>> {
    match rax {
	0x00 => Box::pin(sys_write(rdi, rsi, rdx)),
	0x01 => Box::pin(sys_read(rdi, rsi, rdx)),
	0x02 => Box::pin(sys_open(rdi, rsi)),
	0x03 => Box::pin(sys_close(rdi)),
	0x04 => Box::pin(sys_ioctl(rdi, rsi, rdx)),
	0x05 => Box::pin(sys_stat(rdi, rsi)),
	0x08 => Box::pin(sys_seek(rdi, rsi, rdx)),
	0x09 => Box::pin(sys_mmap(rdi, rsi, r8)),
	0x0c => scheduler::exit(),  // Doesn't return, so no need for async fn here
	0x39 => Box::pin(sys_fork(rcx)),
	0x3b => Box::pin(sys_execve(rdi, rsi, rdx)),
	0x12c => Box::pin(sys_tcb_set(rdi)),
	_ => panic!("Invalid syscall 0x{:X}", rax),
    }
}

#[no_mangle]
unsafe extern "C" fn syscall_inner(mut stack_frame: scheduler::GeneralPurposeRegisters) -> ! {
    let rsp: u64;
    core::arch::asm!(
	"mov {rsp}, gs:[{sp}]",
	rsp = out(reg) rsp,
	sp = const(offset_of!(gdt::ProcessorControlBlock, tmp_user_stack_ptr)),
    );

    let rax = stack_frame.rax;
    let rdx = stack_frame.rdx;
    let rip = stack_frame.rcx;

    scheduler::set_registers_for_current_process(rsp, rip, stack_frame.r11, &mut stack_frame);

    let fut = do_syscall(
	rax,
	stack_frame.rdi,
	stack_frame.rsi,
	rdx,
	stack_frame.r10,
	stack_frame.r8,
	stack_frame.r9,
	stack_frame.rcx);

    scheduler::set_task_state(scheduler::TaskState::AsyncSyscall {
	future: Arc::new(Mutex::new(fut)),
	waker: None,
    });

    scheduler::schedule_next();
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

	options(noreturn),
	sp = const(offset_of!(gdt::ProcessorControlBlock, tmp_user_stack_ptr)),
	ksp = const(offset_of!(gdt::ProcessorControlBlock, tss) + offset_of!(TaskStateSegment, privilege_stack_table)),
    );
}

#[allow(named_asm_labels)]
pub unsafe fn do_syscall6(
    nr: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
    arg6: u64,
) -> (u64, u64) {
    let ret: u64;
    let err: u64;
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
	"call syscall_inner",

	"2:",
	"nop",
	
        in("rax") nr,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        in("r10") arg4,
        in("r8")  arg5,
        in("r9")  arg6,
        lateout("rax") ret,
	lateout("rdx") err,
        lateout("rcx") _,
        lateout("r11") _,

	sp = const(offset_of!(gdt::ProcessorControlBlock, tmp_user_stack_ptr)),
	ksp = const(offset_of!(gdt::ProcessorControlBlock, tss) + offset_of!(TaskStateSegment, privilege_stack_table)),
    );
    (ret, err)
}
