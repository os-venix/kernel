use core::mem::offset_of;
use x86_64::structures::tss::TaskStateSegment;
use core::ffi::{CStr, c_int};
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
use num_enum::TryFromPrimitive;
use alloc::ffi::CString;
use core::ptr;
use spin::RwLock;
use core::slice;
use alloc::vec;
use core::mem;

use crate::sys::ioctl;
use crate::gdt;
use crate::scheduler;
use crate::scheduler::signal;
use crate::scheduler::elf_loader;
use crate::sys::vfs;
use crate::memory;
use crate::process;

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
    ERANGE = 34,
}

#[repr(u64)]
#[derive(Debug, TryFromPrimitive)]
enum FcntlOperation {
    F_DUPFD = 1,
    F_GETFD = 3,
    F_SETFD = 4,
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

    let process = scheduler::get_current_process();
    let kbuf = memory::copy_from_user(VirtAddr::new(buf), count as usize).unwrap();

    let actual_fd = process.get_file_descriptor(fd);
    // let actual_fd = match process.get_file_descriptor(fd) {
    // 	Ok(fd) => fd.file_description,
    // 	Err(_) => {
    // 	    return SyscallResult {
    // 		return_value: 0xFFFF_FFFF_FFFF_FFFF,
    // 		err_num: CanonicalError::EBADF as u64
    // 	    };
    // 	},
    // };

    let mut w = actual_fd.file_description.write();
    match w.write(kbuf, count) {
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
    let process = scheduler::get_current_process();
    let actual_fd = process.get_file_descriptor(fd);
    // let actual_fd = match process.get_file_descriptor(fd) {
    // 	Ok(fd) => fd.file_description,
    // 	Err(_) => {
    // 	    return SyscallResult {
    // 		return_value: 0xFFFF_FFFF_FFFF_FFFF,
    // 		err_num: CanonicalError::EBADF as u64
    // 	    };
    // 	},
    // };

    let mut w = actual_fd.file_description.write();
    let read_buffer = match w.read(count).await {
	Ok(b) => b,
	Err(_) => return SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::EIO as u64
	},
    };

    match memory::copy_to_user(VirtAddr::new(buf), read_buffer.to_vec().as_slice()) {
	Ok(()) => SyscallResult {
	    return_value: read_buffer.len() as u64,
	    err_num: CanonicalError::EOK as u64,
	},
	Err(_) => SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::EIO as u64,  // TODO: This is probably not EIO. Look up what it should be canonically
	},
    }
}

pub async fn sys_open(path_ptr: u64, flags: u64) -> SyscallResult {
    let path = match memory::copy_string_from_user(VirtAddr::new(path_ptr)) {
	Ok(path) => path,
	Err(_) => {
	    return SyscallResult {
		return_value: 0xFFFF_FFFF_FFFF_FFFF,
		err_num: CanonicalError::EINVAL as u64
	    };
	},
    };

    let process = scheduler::get_current_process();

    // TODO: check that the file exists
    // Bottom 3 bits are mode. We don't currently enforce mode, but in order to progress, let's strip it out.
    // Similarly, there isn't yet a concept of a controlling TTY, so let's not worry about that either for now
    // TODO - support read/write/exec/etc modes
    // TODO - support O_NOCTTY (0x80)
    if flags & 0xFFFF_FFFF_FFFF_FF78 != 0 {
	log::info!("Open flags are 0x{:x} for {}", flags, path);
	unimplemented!();
    }

    if let Err(_) = vfs::stat(path.clone()).await {
	// File does not exist
	return SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::EACCESS as u64
	};
    }

    let fd = process::FileDescriptor {
	flags: flags,
	file_description: Arc::new(RwLock::new(vfs::FileDescriptor::new(path))),
    };

    let fd_num = process.emplace_fd(fd);

    SyscallResult {
	return_value: fd_num,
	err_num: CanonicalError::EOK as u64,
    }
}

async fn sys_close(fd: u64) -> SyscallResult {
    let process = scheduler::get_current_process();
    process.close_fd(fd);

    SyscallResult {
	return_value: 0,
	err_num: 0,
    }
}

async fn sys_ioctl(fd_num: u64, ioctl: u64, buf: u64) -> SyscallResult {
    let op = match ioctl::IoCtl::try_from(ioctl) {
	Ok(v) => v,
	Err(_) => {
	    log::info!("Got ioctl number 0x{:x}", ioctl);
	    unimplemented!();
	},
    };

    let process = scheduler::get_current_process();
    let actual_fd = process.get_file_descriptor(fd_num);
    // let actual_fd = match process.get_file_descriptor(fd) {
    // 	Ok(fd) => fd.file_description,
    // 	Err(_) => {
    // 	    return SyscallResult {
    // 		return_value: 0xFFFF_FFFF_FFFF_FFFF,
    // 		err_num: CanonicalError::EBADF as u64
    // 	    };
    // 	},
    // };

    let r = actual_fd.file_description.read();
    match r.ioctl(op, buf) {
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
    let path = match memory::copy_string_from_user(VirtAddr::new(filename)) {
	Ok(path) => path,
	Err(_) => {
	    return SyscallResult {
		return_value: 0xFFFF_FFFF_FFFF_FFFF,
		err_num: CanonicalError::EINVAL as u64
	    };
	},
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

async fn sys_fstat(fd: u64, buf: u64) -> SyscallResult {
    let process = scheduler::get_current_process();
    let actual_fd = process.get_file_descriptor(fd);
    let file_description = actual_fd.file_description.read();

    let path = match &*file_description {
	vfs::FileDescriptor::File { file_name, .. } => file_name.clone(),
	_ => return SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::EINVAL as u64,
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

async fn sys_dup(fd_num: u64) -> SyscallResult {
    let process = scheduler::get_current_process();
    let actual_fd = process.get_file_descriptor(fd_num);
    // let actual_fd = match process.get_file_descriptor(fd) {
    // 	Ok(fd) => fd.file_description,
    // 	Err(_) => {
    // 	    return SyscallResult {
    // 		return_value: 0xFFFF_FFFF_FFFF_FFFF,
    // 		err_num: CanonicalError::EBADF as u64
    // 	    };
    // 	},
    // };

    let new_fd = process.emplace_fd(actual_fd);
    SyscallResult {
	return_value: new_fd,
	err_num: CanonicalError::EOK as u64,
    }
}

async fn sys_fcntl(fd_num: u64, operation: u64, param: u64) -> SyscallResult {
    let op = match FcntlOperation::try_from(operation) {
	Ok(v) => v,
	Err(_) => {
	    log::info!("Got fcntl number 0x{:x}", operation);
	    return SyscallResult {
		return_value:0xFFFF_FFFF_FFFF_FFFF,
		err_num: CanonicalError::EINVAL as u64,
	    };
//	    unimplemented!();
	},
    };

    match op {
	FcntlOperation::F_DUPFD => {
	    let process = scheduler::get_current_process();
	    let actual_fd = process.get_file_descriptor(fd_num);
	    // let actual_fd = match process.get_file_descriptor(fd) {
	    // 	Ok(fd) => fd.file_description,
	    // 	Err(_) => {
	    // 	    return SyscallResult {
	    // 		return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    // 		err_num: CanonicalError::EBADF as u64
	    // 	    };
	    // 	},
	    // };

	    let new_fd = process.emplace_fd_at(actual_fd, param, true);
	    return SyscallResult {
		return_value: new_fd,
		err_num: CanonicalError::EOK as u64,
	    };
	},
	FcntlOperation::F_GETFD => {
	    let process = scheduler::get_current_process();
	    let actual_fd = process.get_file_descriptor(fd_num);
	    // let actual_fd = match process.get_file_descriptor(fd) {
	    // 	Ok(fd) => fd.file_description,
	    // 	Err(_) => {
	    // 	    return SyscallResult {
	    // 		return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    // 		err_num: CanonicalError::EBADF as u64
	    // 	    };
	    // 	},
	    // };

	    return SyscallResult {
		return_value: actual_fd.flags,
		err_num: CanonicalError::EOK as u64,
	    };
	},
	FcntlOperation::F_SETFD => {
	    let process = scheduler::get_current_process();
	    // Bottom 3 bits are mode. We don't currently enforce mode, but in order to progress, let's strip it out.
	    // Similarly, there isn't yet a concept of a controlling TTY, so let's not worry about that either for now
	    // TODO - support read/write/exec/etc modes
	    // TODO - support O_NOCTTY (0x80)
	    if param & 0xFFFF_FFFF_FFFF_FF78 != 0 {
		log::info!("SETFD flags are 0x{:x}", param);
		unimplemented!();
	    }

	    process.set_fd_flags(fd_num, param);
	    return SyscallResult {
		return_value: 0,
		err_num: CanonicalError::EOK as u64,
	    };
	},
    }
}

async fn sys_seek(fd_num: u64, offset: u64, whence: u64) -> SyscallResult {
    let process = scheduler::get_current_process();
    let actual_fd = process.get_file_descriptor(fd_num);
    // let actual_fd = match process.get_file_descriptor(fd) {
    // 	Ok(fd) => fd.file_description,
    // 	Err(_) => {
    // 	    return SyscallResult {
    // 		return_value: 0xFFFF_FFFF_FFFF_FFFF,
    // 		err_num: CanonicalError::EBADF as u64
    // 	    };
    // 	},
    // };

    // Valid values are SEEK_SET, SEEK_CUR, or SEEK_END
    if whence > 3 || whence == 0 {
	return SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::EINVAL as u64
	};
    }

    let mut w = actual_fd.file_description.write();
    match w.seek(offset, whence).await {
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
    if count == 0 {
	return SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::EINVAL as u64
	};
    }

    // TODO: properly pass parameters, and properly name them. There's a lot of unimplemented stuff here
    if r8 != 0xFFFF_FFFF_FFFF_FFFF {
	unimplemented!();
    }

    let process = scheduler::get_current_process();
    let mut task_type = process.task_type.write();
    let start = match *task_type {
	process::TaskType::Kernel => {
	    let (start, _) = if start_val == 0 {
		match memory::kernel_allocate(
		    count,
		    memory::MemoryAllocationType::RAM) {
		    Ok(i) => i,
		    Err(e) => panic!("Could not allocate memory for mmap: {:?}", e),
		}
	    } else {
		unimplemented!();
		// match memory::kernel_allocate(
		//     count,
		//     memory::MemoryAllocationType::RAM,
		//     memory::MemoryAccessRestriction::UserByStart(VirtAddr::new(start_val))) {
		//     Ok(i) => i,
		//     Err(e) => panic!("Could not allocate memory for mmap: {:?}", e),
		// }
	    };

	    start
	},
	process::TaskType::User(ref mut address_space) => {
	    let (start, _) = if start_val == 0 {
		match memory::user_allocate(
		    count,
		    memory::MemoryAllocationType::RAM,
		    memory::MemoryAccessRestriction::User,
		    address_space) {
		    Ok(i) => i,
		    Err(e) => panic!("Could not allocate memory for mmap: {:?}", e),
		}
	    } else {
		match memory::user_allocate(
		    count,
		    memory::MemoryAllocationType::RAM,
		    memory::MemoryAccessRestriction::UserByStart(VirtAddr::new(start_val)),
		    address_space) {
		    Ok(i) => i,
		    Err(e) => panic!("Could not allocate memory for mmap: {:?}", e),
		}		
	    };

	    start
	},
    };

    SyscallResult {
	return_value: start.as_u64(),
	err_num: CanonicalError::EOK as u64,
    }
}

async fn sys_pipe(fds: u64, flags: u64) -> SyscallResult {
    let process = scheduler::get_current_process();
    let file_description = Arc::new(RwLock::new(vfs::FileDescriptor::new_pipe()));

    let fd1 = process::FileDescriptor {
	flags: flags,
	file_description: file_description.clone(),
    };
    let fd2 = process::FileDescriptor {
	flags: flags,
	file_description: file_description.clone(),
    };

    let fd1_number = process.clone().emplace_fd(fd1);
    let fd2_number = process.emplace_fd(fd2);

    let v = vec![fd1_number as u32, fd2_number as u32];

    unsafe {
	memory::copy_to_user(VirtAddr::new(fds), slice::from_raw_parts(
	    v.as_ptr() as *const u8,
	    v.len() * mem::size_of::<u32>())).unwrap();
    }

    SyscallResult {
	return_value: 0,
	err_num: CanonicalError::EOK as u64,
    }
}
    
async fn sys_getcwd(buf: u64, count: u64) -> SyscallResult {
    let process = scheduler::get_current_process();
    let cwd = process.get_cwd();

    memory::copy_string_to_user(VirtAddr::new(buf), cwd).unwrap();

    SyscallResult {
	return_value: 0,
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
    let path = memory::copy_string_from_user(VirtAddr::new(path_ptr)).unwrap();
    let mut args = {
	let mut args: Vec<String> = Vec::new();
	let mut argc_ptr = VirtAddr::new(args_ptr);
	loop {
	    let argp = memory::copy_value_from_user::<VirtAddr>(argc_ptr).unwrap();
	    if argp == VirtAddr::new(0) {
		break;
	    }
	    
	    let arg = memory::copy_string_from_user(argp).unwrap();

	    args.push(arg);
	    argc_ptr += 8;
	}

	args
    };
    args.insert(0, path.clone());

    let envvars = unsafe {
	let mut envvars: Vec<String> = Vec::new();

	let mut envvar_ptr = VirtAddr::new(envvars_ptr);
	loop {
	    let envvarp = memory::copy_value_from_user::<VirtAddr>(envvar_ptr).unwrap();
	    if envvarp == VirtAddr::new(0) {
		break;
	    }

	    let envvar = memory::copy_string_from_user(envvarp).unwrap();

	    envvars.push(envvar);
	    envvar_ptr += 8;
	}

	envvars
    };

    let process = scheduler::get_current_process();
    process.clone().execve(args, envvars);

    let elf = elf_loader::Elf::new(path).await.expect("Failed to load ELF");
    let ld = elf_loader::Elf::new(String::from("/usr/lib/ld.so")).await.expect("Failed to load ld.so");
    process.clone().attach_loaded_elf(elf, ld);

    process.clone().init_stack_and_start();

    SyscallResult {
	return_value: 0,
	err_num: CanonicalError::EOK as u64,
    }
}

async fn sys_getpid() -> SyscallResult {
    let pid = scheduler::get_current_pid();
    SyscallResult {
	return_value: pid,
	err_num: CanonicalError::EOK as u64,
    }
}

async fn sys_getppid() -> SyscallResult {
    // We don't yet support process parentage
    SyscallResult {
	return_value: 0,
	err_num: CanonicalError::EOK as u64,
    }
}

async fn sys_getpgid() -> SyscallResult {
    // We don't yet support process groups
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

async fn sys_sigaction(signum: u64, new_sigaction: u64, old_sigaction: u64) -> SyscallResult {
    let process = scheduler::get_current_process();
    if old_sigaction != 0 {
	if let Some(signal) = process.get_current_signal_handler(signum) {
	    let sigaction = signal::create_sigaction(signal);

	    match memory::copy_value_to_user::<signal::SigAction>(
		VirtAddr::new(new_sigaction), &sigaction) {
		Ok(s) => (),
		Err(_) => {
		    return SyscallResult {
			return_value: 0xFFFF_FFFF_FFFF_FFFF,
			err_num: CanonicalError::EINVAL as u64
		    };
		},
	    }
	}
    }

    if new_sigaction != 0 {
	let sa = match memory::copy_value_from_user::<signal::SigAction>(VirtAddr::new(new_sigaction)) {
	    Ok(s) => s,
	    Err(_) => {
		return SyscallResult {
		    return_value: 0xFFFF_FFFF_FFFF_FFFF,
		    err_num: CanonicalError::EINVAL as u64
		};
	    },
	};

	let signal_handler = signal::parse_sigaction(sa);
	process.install_signal_handler(signum, signal_handler);
    }

    SyscallResult {
	return_value: 0,
	err_num: CanonicalError::EOK as u64,
    }
}

async fn sys_sigprocmask(how: u64, set: u64, oldset: u64) -> SyscallResult {
    let process = scheduler::get_current_process();

    let newset = match memory::copy_value_from_user::<u64>(VirtAddr::new(set)) {
	Ok(s) => s,
	Err(_) => {
	    return SyscallResult {
		return_value: 0xFFFF_FFFF_FFFF_FFFF,
		err_num: CanonicalError::EINVAL as u64
	    };
	},
    };

    if oldset != 0 {
	let old_val = process.get_current_sigprocmask();
	if let Err(_) = memory::copy_value_to_user::<u64>(VirtAddr::new(oldset), &old_val) {
	    return SyscallResult {
		return_value: 0xFFFF_FFFF_FFFF_FFFF,
		err_num: CanonicalError::EINVAL as u64
	    };
	}
    }

    match how {
	1 => process.signal_mask_block(newset),
	2 => process.signal_mask_unblock(newset),
	3 => process.signal_mask_setmask(newset),
	_ => return SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::EINVAL as u64,
	},
    }

    SyscallResult {
	return_value: 0,
	err_num: CanonicalError::EOK as u64,
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
	0x06 => Box::pin(sys_dup(rdi)),
	0x07 => Box::pin(sys_fcntl(rdi, rsi, rdx)),
	0x08 => Box::pin(sys_seek(rdi, rsi, rdx)),
	0x09 => Box::pin(sys_mmap(rdi, rsi, r8)),
	0x0a => Box::pin(sys_pipe(rdi, rsi)),
	0x0b => Box::pin(sys_fstat(rdi, rsi)),
	0x0c => scheduler::exit(rdi),  // Doesn't return, so no need for async fn here
	0x10 => Box::pin(sys_sigaction(rdi, rsi, rdx)),
	0x11 => Box::pin(sys_sigprocmask(rdi, rsi, rdx)),
	0x20 => Box::pin(sys_getcwd(rdi, rsi)),
	0x39 => Box::pin(sys_fork(rcx)),
	0x3b => Box::pin(sys_execve(rdi, rsi, rdx)),
	0x3c => Box::pin(sys_getpid()),
	0x3d => Box::pin(sys_getppid()),
	0x3e => Box::pin(sys_getpgid()),
	0x12c => Box::pin(sys_tcb_set(rdi)),
	_ => panic!("Invalid syscall 0x{:X}", rax),
    }
}

#[no_mangle]
unsafe extern "C" fn syscall_inner(stack_frame: process::GeneralPurposeRegisters) -> ! {
    let rsp: u64;
    core::arch::asm!(
	"mov {rsp}, gs:[{sp}]",
	rsp = out(reg) rsp,
	sp = const(offset_of!(gdt::ProcessorControlBlock, tmp_user_stack_ptr)),
    );

    let rax = stack_frame.rax;
    let rdx = stack_frame.rdx;
    let rip = stack_frame.rcx;

    let process = scheduler::get_current_process();
    process.clone().set_registers(rsp, rip, stack_frame.r11, &stack_frame);

    let fut = do_syscall(
	rax,
	stack_frame.rdi,
	stack_frame.rsi,
	rdx,
	stack_frame.r10,
	stack_frame.r8,
	stack_frame.r9,
	stack_frame.rcx);

    process.set_state(process::TaskState::AsyncSyscall {
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

	"mov rax, gs:[{kcr3}]",
	"mov cr3, rax",

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
	kcr3 = const(offset_of!(gdt::ProcessorControlBlock, kernel_cr3)),
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
