use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{RwLock, Mutex};
use alloc::string::String;
use x86_64::VirtAddr;
use alloc::ffi::CString;
use alloc::vec;
use alloc::collections::BTreeMap;
use alloc::boxed::Box;
use core::pin::Pin;
use core::future::Future;

use crate::memory;
use crate::sys::vfs;
use crate::sys::syscall;
use crate::gdt;
use crate::scheduler::elf_loader;
use crate::scheduler::signal;

const AT_NUL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_BASE: u64 = 7;
const AT_ENTRY: u64 = 9;

pub type SyscallFuture = Pin<Box<dyn Future<Output = syscall::SyscallResult> + Send + 'static>>;

#[repr(C)]
#[derive(Copy, Clone, Default, Debug)]
pub struct GeneralPurposeRegisters {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
}

#[derive(Copy, Clone, Default, Debug)]
pub struct ProcessContext {
    pub gprs: GeneralPurposeRegisters,
    rflags: u64,
    rip: u64,
    rsp: u64,
    cs: u64,
    ss: u64,
}

#[derive(Clone)]
struct AuxVector {
    auxv_type: u64,
    value: u64,
}

#[derive(Clone)]
pub enum TaskState {
    Setup,
    Running,
    AsyncSyscall {
	future: Arc<Mutex<SyscallFuture>>,
    },
    Waiting {
	future: Arc<Mutex<SyscallFuture>>,
    },
}

pub enum TaskType {
    User(memory::user_address_space::AddressSpace),
    Kernel,
}

#[derive(Clone)]
pub struct FileDescriptor {
    pub file_description: Arc<RwLock<vfs::FileDescriptor>>,
    pub flags: u64,
}

pub struct Process {
    file_descriptors: RwLock<BTreeMap<u64, FileDescriptor>>,
    args: RwLock<Vec<String>>,
    envvars: RwLock<Vec<String>>,
    auxvs: RwLock<Vec<AuxVector>>,
    context: RwLock<ProcessContext>,
    state: RwLock<TaskState>,
    pub task_type: Arc<RwLock<TaskType>>,
    cwd: RwLock<String>,
    signals: RwLock<BTreeMap<u64, signal::SignalHandler>>,
    sigmask: RwLock<u64>,
}

unsafe impl Send for Process { }
unsafe impl Sync for Process { }

impl Process {
    pub fn new_kthread(rip: u64) -> Self {
	let (kernel_code, kernel_data, _, _) = gdt::get_code_selectors();
	
	let (rsp, _) = match memory::kernel_allocate(
	    8 * 1024 * 1024,  // 8MiB
	    memory::MemoryAllocationType::Ram) {
	    Ok(i) => i,
	    Err(e) => panic!("Could not allocate stack memory for process: {:?}", e),
	};

	Process {
	    file_descriptors: RwLock::new(BTreeMap::new()),
	    args: RwLock::new(vec!(String::from("init"))),
	    envvars: RwLock::new(vec!(String::from("PATH=/bin:/usr/bin"))),
	    auxvs: RwLock::new(Vec::new()),
	    context: RwLock::new(ProcessContext {
		gprs: GeneralPurposeRegisters::default(),
		rflags: 0x202,
		rip,
		rsp: rsp.as_u64() + (8 * 1024 * 1024),
		cs: kernel_code.0 as u64,
		ss: kernel_data.0 as u64,
	    }),
	    state: RwLock::new(TaskState::Running),
	    task_type: Arc::new(RwLock::new(TaskType::Kernel)),
	    cwd: RwLock::new(String::from("/")),
	    signals: RwLock::new(BTreeMap::new()),
	    sigmask: RwLock::new(0),
	}
    }

    pub fn execve(self: Arc<Self>, new_args: Vec<String>, new_envvars: Vec<String>) {
	let mut task_type = self.task_type.write();
	match &mut *task_type {
	    TaskType::Kernel => {
		let address_space = memory::user_address_space::AddressSpace::new();
		*task_type = TaskType::User(address_space);
	    },
	    TaskType::User(ref mut address_space) => {
		address_space.clear_user_space();
		let address_space = memory::user_address_space::AddressSpace::new();
		*task_type = TaskType::User(address_space);
	    },
	};

	let mut context = self.context.write();
	let mut auxvs = self.auxvs.write();
	let mut signals = self.signals.write();
	let mut args = self.args.write();
	let mut envvars = self.envvars.write();

	*args = new_args;
	*envvars = new_envvars;
	context.gprs = GeneralPurposeRegisters::default();
	context.rflags = 0x202;
	*auxvs = Vec::new();
	*signals = BTreeMap::new();
    }

    pub fn from_existing(old: &Self, rip: u64) -> Self {
	let signals = {
	    let old_signals = old.signals.read();
	    old_signals.clone()
	};

	let auxvs = {
	    let old_auxvs = old.auxvs.read();
	    old_auxvs.clone()
	};

	let (cs, ss) = {
	    let old_context = old.context.read();
	    (old_context.cs, old_context.ss)
	};

	let file_descriptors = {
	    let old_fds = old.file_descriptors.read();
	    old_fds.clone()
	};

	let args = {
	    let old_args = old.args.read();
	    old_args.clone()
	};

	let envvars = {
	    let old_envvars = old.envvars.read();
	    old_envvars.clone()
	};

	let task_type = {
	    let old_task_type = old.task_type.read();

	    match &*old_task_type {
		TaskType::Kernel => TaskType::Kernel,
		TaskType::User(address_space) => {
		    let mut new_address_space = memory::user_address_space::AddressSpace::new();
		    unsafe {
			new_address_space.switch_to();
		    }
		    new_address_space.create_copy_of_address_space(address_space);

		    TaskType::User(new_address_space)
		},
	    }
	};

	let cwd = {
	    let old_cwd = old.cwd.read();
	    old_cwd.clone()
	};

	let sigmask = {
	    let old_sigmask = old.sigmask.read();
	    *old_sigmask
	};

	let mut context = {
	    let old_context = old.context.read();
	    *old_context
	};
	context.gprs.rax = 0;

	Process {
	    file_descriptors: RwLock::new(file_descriptors.clone()),
	    args: RwLock::new(args),
	    envvars: RwLock::new(envvars),
	    auxvs: RwLock::new(auxvs),
	    context: RwLock::new(context),
	    state: RwLock::new(TaskState::Running),
	    task_type: Arc::new(RwLock::new(task_type)),
	    cwd: RwLock::new(cwd),
	    signals: RwLock::new(signals),
	    sigmask: RwLock::new(sigmask),
	}
    }

    pub fn attach_loaded_elf(self: Arc<Self>, elf: elf_loader::Elf, ld_so: elf_loader::Elf) {
	let (_, _, user_code, user_data) = gdt::get_code_selectors();

	let mut context = self.context.write();
	let mut auxvs = self.auxvs.write();

	context.cs = user_code.0 as u64;
	context.ss = user_data.0 as u64;
	context.rip = ld_so.entry;

	auxvs.push(AuxVector {
	    auxv_type: AT_BASE,
	    value: ld_so.base
	});
	auxvs.push(AuxVector {
	    auxv_type: AT_ENTRY,
	    value: elf.entry
	});
	auxvs.push(AuxVector {
	    auxv_type: AT_PHDR,
	    value: elf.program_header
	});
	auxvs.push(AuxVector {
	    auxv_type: AT_PHENT,
	    value: elf.program_header_entry_size
	});
	auxvs.push(AuxVector {
	    auxv_type: AT_PHNUM,
	    value: elf.program_header_entry_count
	});
	auxvs.push(AuxVector {
	    auxv_type: AT_NUL,
	    value: 0
	});
    }

    pub fn init_stack_and_start(self: Arc<Self>) -> Result<(), memory::CopyError> {
	let mut context = self.context.write();
	let mut state = self.state.write();
	let auxvs = self.auxvs.read();
	let envvars = self.envvars.read();
	let args = self.args.read();

	let rsp = {
	    let mut task_type = self.task_type.write();
	    let address_space: &mut memory::user_address_space::AddressSpace = match *task_type {
		TaskType::Kernel => panic!("Attempted to start a user process on a kernel task"),
		// TODO: will this break any file I/O, mmap, etc?
		TaskType::User(ref mut address_space) => address_space,
	    };

	    let (rsp, _) = match memory::user_allocate(
		8 * 1024 * 1024,  // 8MiB
		memory::MemoryAccessRestriction::User,
		address_space) {
		Ok(i) => i,
		Err(e) => panic!("Could not allocate stack memory for process: {:?}", e),
	    };

	    rsp
	};
	context.rsp = rsp.as_u64() + 8 * 1024 * 1024;  // Start at the end of the stack and grow down

	let envvars_buf_size: usize = envvars.iter()
	    .map(|env_var| env_var.len() + 1)
	    .sum();
	let args_buf_size: usize = args.iter()
	    .map(|arg| arg.len() + 1)
	    .sum();

	context.rsp -= envvars_buf_size as u64 + args_buf_size as u64;
	let stack_ptr = VirtAddr::new(context.rsp);

	let mut current_offs = envvars_buf_size + args_buf_size;
	let mut envvar_p: Vec<u64> = Vec::new();
	for envvar in envvars.clone() {
	    let envvar_len = envvar.len() + 1;
	    let envvar_cstring = CString::new(envvar.as_str()).unwrap();
	    memory::copy_to_user(
		stack_ptr + (current_offs - envvar_len) as u64, envvar_cstring.as_bytes_with_nul())?;
	    current_offs -= envvar_len;

	    envvar_p.push(context.rsp + current_offs as u64);
	}

	let mut args_p: Vec<u64> = Vec::new();
	for arg in args.clone() {
	    let arg_len = arg.len() + 1;
	    let arg_cstring = CString::new(arg.as_str()).unwrap();
	    memory::copy_to_user(
		stack_ptr + (current_offs - arg_len) as u64, arg_cstring.as_bytes_with_nul())?;

	    current_offs -= arg_len;
	    args_p.push(context.rsp + current_offs as u64);
	}

	context.rsp -= /* auxv = */(auxvs.len() as u64 * 16) +
	    (envvars.len() as u64 * 8) +
	    (args.len() as u64 * 8) +
	/* padding = */(3 * 8);
	let alignment = context.rsp % 16;
	context.rsp -= alignment;  // Align the stack to a u128, as required by the standard

	let mut buf: Vec<u8> = Vec::new();
	// argc
	buf.extend_from_slice(&(args.len() as u64).to_ne_bytes());
	// argv[0..n]
	for arg in &args_p {
	    buf.extend_from_slice(&(*arg).to_ne_bytes());
	}
	// NULL after argv
	buf.extend_from_slice(&0u64.to_ne_bytes());

	// envp[0..n]
	for env in &envvar_p {
	    buf.extend_from_slice(&(*env).to_ne_bytes());
	}
	// NULL after envp
	buf.extend_from_slice(&0u64.to_ne_bytes());

	// auxv entries (key, value)
	for auxv in auxvs.iter() {
	    buf.extend_from_slice(&auxv.auxv_type.to_ne_bytes());
	    buf.extend_from_slice(&auxv.value.to_ne_bytes());
	}

	// padding
	buf.resize(buf.len() + alignment as usize, 0);
	memory::copy_to_user(VirtAddr::new(context.rsp), buf.as_slice())?;

	*state = TaskState::Running;
	Ok(())
    }

    pub fn set_registers(self: Arc<Self>, rsp: u64, rip: u64, rflags: u64, registers: &GeneralPurposeRegisters) {
	let mut context = self.context.write();

	context.rsp = rsp;
	context.rip = rip;
	context.rflags = rflags;
	context.gprs = *registers;
    }

    pub fn get_current_signal_handler(&self, signal: u64) -> Option<signal::SignalHandler> {
	let signals = self.signals.read();
	signals.get(&signal).cloned()
    }

    pub fn install_signal_handler(self: Arc<Self>, signal: u64, handler: signal::SignalHandler) {
	let mut signals = self.signals.write();
	signals.insert(signal, handler);
    }

    pub fn syscall_return(self: Arc<Self>, rax: u64, rdx: u64) {
	let mut context = self.context.write();
	let mut state = self.state.write();

	context.gprs.rax = rax;
	context.gprs.rdx = rdx;

	*state = TaskState::Running;
    }

    pub fn get_context(&self) -> ProcessContext {
	let context = self.context.read();
	*context
    }

    pub fn get_state(&self) -> TaskState {
	let state = self.state.read();
	state.clone()
    }

    pub fn set_state(self: Arc<Self>, new_state: TaskState) {
	let mut state = self.state.write();
	*state = new_state;
    }

    pub fn emplace_fd(self: Arc<Self>, fd: FileDescriptor) -> u64 {
	let mut file_descriptors = self.file_descriptors.write();

	for i in 0..=u64::MAX {
	    if let alloc::collections::btree_map::Entry::Vacant(e) = file_descriptors.entry(i) {
		e.insert(fd);
		return i;
	    }
	}

	// Should be unreachable unless every possible key is used
	panic!("No available u64 keys left!");
    }

    // TODO: better error handling for out of FDs
    pub fn emplace_fd_at(self: Arc<Self>, fd: FileDescriptor, fd_num: u64, try_greater: bool) -> u64 {
	let mut file_descriptors = self.file_descriptors.write();

	if file_descriptors.contains_key(&fd_num) && !try_greater {
	    panic!("No available file descriptors");
	}

	for i in fd_num..=u64::MAX {
	    if let alloc::collections::btree_map::Entry::Vacant(e) = file_descriptors.entry(i) {
		e.insert(fd);
		return i;
	    }
	}

	panic!("No available u64 keys left from {} onward!", fd_num);
    }

    pub fn set_fd_flags(self: Arc<Self>, fd: u64, flags: u64) {
	let mut file_descriptors = self.file_descriptors.write();

	if let Some(actual_fd) = file_descriptors.get_mut(&fd) {
	    actual_fd.flags = flags;
	} else {
	    panic!("Could not find FD");
	}
    }

    pub fn close_fd(self: Arc<Self>, fd: u64) {
	let mut file_descriptors = self.file_descriptors.write();

	match file_descriptors.remove(&fd) {
	    Some(_) => (),
	    None => panic!("No open FD found: {}", fd),
	}
    }

    pub fn get_file_descriptor(&self, fd: u64) -> FileDescriptor {
	let file_descriptors = self.file_descriptors.read();
	
	if let Some(actual_fd) = file_descriptors.get(&fd) {
	    actual_fd.clone()
	} else {
	    panic!("Could not find FD");
	}
    }

    #[allow(dead_code)]
    pub fn set_cwd(self: Arc<Self>, new_cwd: String) {
	let mut cwd = self.cwd.write();
	*cwd = new_cwd;
    }

    pub fn get_cwd(&self) -> String {
	let cwd = self.cwd.read();
	cwd.clone()
    }

    pub fn get_current_sigprocmask(&self) -> u64 {
	let sigmask = self.sigmask.read();
	*sigmask
    }

    pub fn signal_mask_block(self: Arc<Self>, newset: u64) {
	let mut sigmask = self.sigmask.write();
	*sigmask |= newset;
    }

    pub fn signal_mask_unblock(self: Arc<Self>, newset: u64) {
	let mut sigmask = self.sigmask.write();
	*sigmask &= !newset;
    }

    pub fn signal_mask_setmask(self: Arc<Self>, newset: u64) {
	let mut sigmask = self.sigmask.write();
	*sigmask = newset;
    }
}
