use core::mem::offset_of;
use x86_64::structures::tss::TaskStateSegment;
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
use core::slice;
use core::mem;
use bitflags::bitflags;

use crate::sys::ioctl;
use crate::gdt;
use crate::scheduler;
use crate::scheduler::signal;
use crate::scheduler::elf_loader;
use crate::vfs;
use crate::memory;
use crate::process;
use crate::vfs::filesystem::VNode;

macro_rules! syscall_try {
    ($expr:expr) => {
        match $expr {
            Ok(val) => val,
            Err(err) => {
                return SyscallResult {
                    return_value: 0xFFFF_FFFF_FFFF_FFFF,
                    err_num: err as u64,
                };
            }
        }
    };
}

macro_rules! syscall_success {
    ($expr:expr) => {
        return SyscallResult {
            return_value: $expr,
            err_num: CanonicalError::Ok as u64,
        }
    };
}

macro_rules! syscall_err {
    ($expr:expr) => {
        return SyscallResult {
            return_value: 0xFFFF_FFFF_FFFF_FFFF,
            err_num: $expr as u64,
        }
    };
}

#[repr(u64)]
#[derive(Debug)]
#[allow(dead_code)]
pub enum CanonicalError {
    Ok = 0,
    NoEnt = 2,
    Io = 5,
    Badf = 9,
    Again = 11,
    Access = 13,
    Fault = 14,
    NotDir = 20,
    Inval = 22,
    SPipe = 29,
    Range = 34,
}

#[repr(u64)]
#[derive(Debug, TryFromPrimitive)]
enum FcntlOperation {
    DupFD = 1,
    GetFD = 3,
    SetFD = 4,
    GetFlags = 5,
}

bitflags! {
    #[repr(transparent)]
    #[derive(Debug, Clone, Copy)]
    pub struct PollEvents: u16 {
        const In        = 0x01;
        const Out       = 0x02;
        const Pri       = 0x04;
        const Hup       = 0x08;
        const Err       = 0x10;
        const RdHup     = 0x20;
        const Nval      = 0x40;
        const WrNorm    = 0x80;
        const RdNorm    = 0x100;
        const WrBand    = 0x200;
        const RdBand    = 0x400;
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PollFd {
    pub fd: i32,
    pub events: PollEvents,
    pub revents: PollEvents,
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
    LStar::write(VirtAddr::new(syscall_enter as *const () as usize as u64));

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
	    err_num: CanonicalError::Io as u64
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
    // 		err_num: CanonicalError::Badf as u64
    // 	    };
    // 	},
    // };

    let w = actual_fd.file_handle;
    match w.write(bytes::Bytes::from(kbuf)).await {
	Ok(len) => SyscallResult {
	    return_value: len,
	    err_num: CanonicalError::Ok as u64,
	},
	Err(e) => SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: e as u64
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

    let w = actual_fd.file_handle;
    let read_buffer = syscall_try!(w.read(count).await);

    match memory::copy_to_user(VirtAddr::new(buf), read_buffer.to_vec().as_slice()) {
	Ok(()) => SyscallResult {
	    return_value: read_buffer.len() as u64,
	    err_num: CanonicalError::Ok as u64,
	},
	Err(_) => SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::Io as u64,  // TODO: This is probably not EIO. Look up what it should be canonically
	},
    }
}

pub async fn sys_open(path_ptr: u64, flags: u64) -> SyscallResult {
    let path = match memory::copy_string_from_user(VirtAddr::new(path_ptr)) {
	Ok(path) => path,
	Err(_) => {
	    return SyscallResult {
		return_value: 0xFFFF_FFFF_FFFF_FFFF,
		err_num: CanonicalError::Inval as u64
	    };
	},
    };

    let process = scheduler::get_current_process();

    // TODO: check that the file exists
    // Bottom 3 bits are mode. We don't currently enforce mode, but in order to progress, let's strip it out.
    // Similarly, there isn't yet a concept of a controlling TTY, so let's not worry about that either for now
    // TODO - support read/write/exec/etc modes
    // TODO - support O_NOCTTY (0x80)
    // TODO - support O_TRUNC (0x200)
    // TODO - support O_NOFOLLOW (0x10)
    if flags & 0xFFFF_FFFF_FFFF_FD68 != 0 {
	log::info!("Open flags are 0x{:x} for {}", flags, path);
	unimplemented!();
    }

    let fh = syscall_try!(vfs::vfs_open(&path).await);
    let fd = process::FileDescriptor {
	flags,
	file_handle: fh,
    };
    let fd_num = process.emplace_fd(fd);

    SyscallResult {
	return_value: fd_num,
	err_num: CanonicalError::Ok as u64,
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

    let r = actual_fd.file_handle;
    let r = syscall_try!(r.ioctl(op, buf).await);
    syscall_success!(r);
}

async fn sys_stat(filename: u64, _buf: u64) -> SyscallResult {
    let path = match memory::copy_string_from_user(VirtAddr::new(filename)) {
	Ok(path) => path,
	Err(_) => {
	    return SyscallResult {
		return_value: 0xFFFF_FFFF_FFFF_FFFF,
		err_num: CanonicalError::Inval as u64
	    };
	},
    };

    let fh = syscall_try!(vfs::vfs_open(&path).await);
    match fh.stat() {
	Ok(_ret) => SyscallResult {
	    return_value: 0,
	    err_num: CanonicalError::Ok as u64
	},
	Err(e) => SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: e as u64
	}
    }
}

async fn sys_fstat(fd: u64, _buf: u64) -> SyscallResult {
    let process = scheduler::get_current_process();
    let actual_fd = process.get_file_descriptor(fd);

    match actual_fd.file_handle.stat() {
	Ok(_ret) => SyscallResult {
	    return_value: 0,
	    err_num: CanonicalError::Ok as u64
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
	err_num: CanonicalError::Ok as u64,
    }
}

async fn sys_fcntl(fd_num: u64, operation: u64, param: u64) -> SyscallResult {
    let op = match FcntlOperation::try_from(operation) {
	Ok(v) => v,
	Err(_) => {
	    log::info!("Got fcntl number 0x{:x}", operation);
	    return SyscallResult {
		return_value:0xFFFF_FFFF_FFFF_FFFF,
		err_num: CanonicalError::Inval as u64,
	    };
//	    unimplemented!();
	},
    };

    match op {
	FcntlOperation::DupFD => {
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
	    SyscallResult {
		return_value: new_fd,
		err_num: CanonicalError::Ok as u64,
	    }
	},
	FcntlOperation::GetFD => {
	    // TODO: these are the wrong flags. This should be reading cloexec only.
	    let process = scheduler::get_current_process();
	    let actual_fd = process.get_file_descriptor(fd_num);

	    SyscallResult {
		return_value: actual_fd.flags,
		err_num: CanonicalError::Ok as u64,
	    }
	},
	FcntlOperation::SetFD => {
	    // TODO: these are the wrong flags. This should be reading cloexec only.

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
	    SyscallResult {
		return_value: 0,
		err_num: CanonicalError::Ok as u64,
	    }
	},
	FcntlOperation::GetFlags => {
	    let process = scheduler::get_current_process();
	    let actual_fd = process.get_file_descriptor(fd_num);

	    SyscallResult {
		return_value: actual_fd.flags,
		err_num: CanonicalError::Ok as u64,
	    }
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

    let whence = match whence {
	1 => vfs::filesystem::SeekFrom::Cur(offset as i64),
	2 => vfs::filesystem::SeekFrom::End(offset as i64),
	3 => vfs::filesystem::SeekFrom::Set(offset as i64),

	_ => syscall_err!(CanonicalError::Inval),
    };

    let w = actual_fd.file_handle;
    let offs = syscall_try!(w.seek(whence));
    syscall_success!(offs);
}

async fn sys_poll(fds: u64, nfds: u64, _timeout: u64) -> SyscallResult {
    // If there aren't any FDs, just don't do anything
    if nfds == 0 {
	return SyscallResult {
	    return_value: 0,
	    err_num: CanonicalError::Ok as u64,
	};
    }

    // There's no way to combine futures yet in the kernel, so we can only handle 1 FD
    if nfds > 1 {
	unimplemented!();
    }

    // Parse the FDs
    let mut fds_vec = Vec::with_capacity(nfds as usize);
    {
	let mut addr = VirtAddr::new(fds);
	for _ in 0..nfds {
	    let fd = match memory::copy_value_from_user::<PollFd>(addr) {
		Ok(fd) => fd,
		Err(_) => return SyscallResult {
		    return_value: 0xFFFF_FFFF_FFFF_FFFF,
		    err_num: CanonicalError::Fault as u64,
		},
	    };
	    addr += core::mem::size_of::<PollFd>().try_into().unwrap();

	    fds_vec.push(fd);
	}
    }

    // Skip if negative
    if let Ok(fd) = TryInto::<u64>::try_into(fds_vec[0].fd) {
	// Actually poll
	let process = scheduler::get_current_process();
	let actual_fd = process.get_file_descriptor(fd);
	let r = actual_fd.file_handle;

	let revents = syscall_try!(r.poll(fds_vec[0].events).await);
	fds_vec[0].revents = revents;
    }

    // Copy the results back to userspace
    {
	let mut addr = VirtAddr::new(fds);
	for i in 0..nfds {
	    match memory::copy_value_to_user::<PollFd>(addr, &fds_vec[i as usize]) {
		Ok(()) => (),
		Err(_) => return SyscallResult {
		    return_value: 0xFFFF_FFFF_FFFF_FFFF,
		    err_num: CanonicalError::Fault as u64,
		},
	    }
	    addr += core::mem::size_of::<PollFd>().try_into().unwrap();
	}
    }

    SyscallResult {
	return_value: 1,
	err_num: CanonicalError::Ok as u64,
    }
}

async fn sys_mmap(start_val: u64, count: u64, r8: u64) -> SyscallResult {
    if count == 0 {
	return SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::Inval as u64
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
		    memory::MemoryAllocationType::Ram) {
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
		    memory::MemoryAccessRestriction::User,
		    address_space) {
		    Ok(i) => i,
		    Err(e) => panic!("Could not allocate memory for mmap: {:?}", e),
		}
	    } else {
		match memory::user_allocate(
		    count,
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
	err_num: CanonicalError::Ok as u64,
    }
}

async fn sys_pipe(fds: u64, flags: u64) -> SyscallResult {
    let process = scheduler::get_current_process();
    let file_description = Arc::new(vfs::fifo::Fifo::new());

    let fd1 = process::FileDescriptor {
	flags,
	file_handle: syscall_try!(file_description.clone().open()),
    };
    let fd2 = process::FileDescriptor {
	flags,
	file_handle: syscall_try!(file_description.clone().open()),
    };

    let fd1_number = process.clone().emplace_fd(fd1);
    let fd2_number = process.emplace_fd(fd2);

    let v = [fd1_number as u32, fd2_number as u32];

    unsafe {
	memory::copy_to_user(VirtAddr::new(fds), slice::from_raw_parts(
	    v.as_ptr() as *const u8,
	    v.len() * mem::size_of::<u32>())).unwrap();
    }

    SyscallResult {
	return_value: 0,
	err_num: CanonicalError::Ok as u64,
    }
}
    
async fn sys_getcwd(buf: u64, _count: u64) -> SyscallResult {
    let process = scheduler::get_current_process();
    let cwd = process.get_cwd();

    memory::copy_string_to_user(VirtAddr::new(buf), cwd).unwrap();

    SyscallResult {
	return_value: 0,
	err_num: CanonicalError::Ok as u64,
    }
}

async fn sys_fork() -> SyscallResult {
    let pid = scheduler::fork_current_process();
    SyscallResult {
	return_value: pid,
	err_num: CanonicalError::Ok as u64,
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

    log::info!("y");

    let process = scheduler::get_current_process();
    log::info!("y.1");
    process.clone().execve(args, envvars);
    log::info!("y.2");

    let elf = elf_loader::Elf::new(path).await.expect("Failed to load ELF");
    log::info!("y.3");
    let ld = elf_loader::Elf::new(String::from("/usr/lib/ld.so")).await.expect("Failed to load ld.so");
    log::info!("y.4");
    process.clone().attach_loaded_elf(elf, ld);

    log::info!("z");

    if let Err(_e) = process.clone().init_stack_and_start() {
	return SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::Io as u64,
	};
    }

    SyscallResult {
	return_value: 0,
	err_num: CanonicalError::Ok as u64,
    }
}

async fn sys_getpid() -> SyscallResult {
    let pid = scheduler::get_current_pid();
    SyscallResult {
	return_value: pid,
	err_num: CanonicalError::Ok as u64,
    }
}

async fn sys_getppid() -> SyscallResult {
    // We don't yet support process parentage
    SyscallResult {
	return_value: 0,
	err_num: CanonicalError::Ok as u64,
    }
}

async fn sys_getpgid() -> SyscallResult {
    // We don't yet support process groups
    SyscallResult {
	return_value: 0,
	err_num: CanonicalError::Ok as u64,
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
		Ok(()) => (),
		Err(_) => {
		    return SyscallResult {
			return_value: 0xFFFF_FFFF_FFFF_FFFF,
			err_num: CanonicalError::Inval as u64
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
		    err_num: CanonicalError::Inval as u64
		};
	    },
	};

	let signal_handler = signal::parse_sigaction(sa);
	process.install_signal_handler(signum, signal_handler);
    }

    SyscallResult {
	return_value: 0,
	err_num: CanonicalError::Ok as u64,
    }
}

async fn sys_sigprocmask(how: u64, set: u64, oldset: u64) -> SyscallResult {
    let process = scheduler::get_current_process();

    let newset = match memory::copy_value_from_user::<u64>(VirtAddr::new(set)) {
	Ok(s) => s,
	Err(_) => {
	    return SyscallResult {
		return_value: 0xFFFF_FFFF_FFFF_FFFF,
		err_num: CanonicalError::Inval as u64
	    };
	},
    };

    if oldset != 0 {
	let old_val = process.get_current_sigprocmask();
	if memory::copy_value_to_user::<u64>(VirtAddr::new(oldset), &old_val).is_err() {
	    return SyscallResult {
		return_value: 0xFFFF_FFFF_FFFF_FFFF,
		err_num: CanonicalError::Inval as u64
	    };
	}
    }

    match how {
	1 => process.signal_mask_block(newset),
	2 => process.signal_mask_unblock(newset),
	3 => process.signal_mask_setmask(newset),
	_ => return SyscallResult {
	    return_value: 0xFFFF_FFFF_FFFF_FFFF,
	    err_num: CanonicalError::Inval as u64,
	},
    }

    SyscallResult {
	return_value: 0,
	err_num: CanonicalError::Ok as u64,
    }
}

fn do_syscall(rax: u64, rdi: u64, rsi: u64, rdx: u64, _r10: u64, r8: u64, _r9: u64) -> Pin<Box<dyn Future<Output = SyscallResult> + Send + 'static>> {
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
	0x0d => Box::pin(sys_poll(rdi, rsi, rdx)),
	0x10 => Box::pin(sys_sigaction(rdi, rsi, rdx)),
	0x11 => Box::pin(sys_sigprocmask(rdi, rsi, rdx)),
	0x20 => Box::pin(sys_getcwd(rdi, rsi)),
	0x39 => Box::pin(sys_fork()),
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
	stack_frame.r9);

    process.set_state(process::TaskState::AsyncSyscall {
	future: Arc::new(Mutex::new(fut)),
    });

    scheduler::schedule_next();
}

// TODO - load kernel stack; may need to use swapgs for that
#[unsafe(naked)]
#[allow(named_asm_labels)]
unsafe extern "C" fn syscall_enter () -> ! {
    core::arch::naked_asm!(
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
