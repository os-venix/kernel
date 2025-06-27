use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{RwLock, Mutex};
use alloc::string::String;
use x86_64::VirtAddr;
use alloc::ffi::CString;
use alloc::slice;
use alloc::vec;
use alloc::collections::BTreeMap;
use alloc::boxed::Box;
use core::pin::Pin;
use core::future::Future;
use core::task::Waker;

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
	future: Arc<Mutex<Pin<Box<dyn Future<Output = syscall::SyscallResult> + Send + 'static>>>>,
	waker: Option<Waker>,
    },
}

#[derive(Clone, Copy)]
pub enum TaskType {
    User,
    Kernel,
}

#[derive(Clone)]
pub struct FileDescriptor {
    pub file_description: Arc<RwLock<vfs::FileDescriptor>>,
    pub flags: u64,
}

pub struct Process {
    pub address_space: Arc<RwLock<memory::user_address_space::AddressSpace>>,
    file_descriptors: RwLock<BTreeMap<u64, FileDescriptor>>,
    args: RwLock<Vec<String>>,
    envvars: RwLock<Vec<String>>,
    auxvs: RwLock<Vec<AuxVector>>,
    context: RwLock<ProcessContext>,
    state: RwLock<TaskState>,
    task_type: RwLock<TaskType>,
    cwd: RwLock<String>,
    signals: RwLock<BTreeMap<u64, signal::SignalHandler>>,
}

unsafe impl Send for Process { }
unsafe impl Sync for Process { }

impl Process {
    pub fn new_kthread(rip: u64) -> Self {
	let mut address_space = memory::user_address_space::AddressSpace::new();
	unsafe {
	    address_space.switch_to();
	}
	
	let (kernel_code, kernel_data, _, _) = gdt::get_code_selectors();
	
	let (rsp, _) = match memory::user_allocate(
	    8 * 1024 * 1024,  // 8MiB
	    memory::MemoryAllocationType::RAM,
	    memory::MemoryAllocationOptions::Arbitrary,
	    memory::MemoryAccessRestriction::User,
	    &mut address_space) {
	    Ok(i) => i,
	    Err(e) => panic!("Could not allocate stack memory for process: {:?}", e),
	};

	Process {
	    address_space: Arc::new(RwLock::new(address_space)),
	    file_descriptors: RwLock::new(BTreeMap::new()),
	    args: RwLock::new(vec!(String::from("init"))),
	    envvars: RwLock::new(vec!(String::from("PATH=/bin:/usr/bin"))),
	    auxvs: RwLock::new(Vec::new()),
	    context: RwLock::new(ProcessContext {
		gprs: GeneralPurposeRegisters::default(),
		rflags: 0x202,
		rip: rip,
		rsp: rsp.as_u64() + (8 * 1024 * 1024),
		cs: kernel_code.0 as u64,
		ss: kernel_data.0 as u64,
	    }),
	    state: RwLock::new(TaskState::Running),
	    task_type: RwLock::new(TaskType::Kernel),
	    cwd: RwLock::new(String::from("/")),
	    signals: RwLock::new(BTreeMap::new()),
	}
    }

    pub fn from_existing(&self, rip: u64) -> Self {
	let mut new_address_space = memory::user_address_space::AddressSpace::new();
	{
	    let address_space = self.address_space.read();
	    unsafe {
		new_address_space.switch_to();
	    }

	    new_address_space.create_copy_of_address_space(&address_space);
	}

	let signals = {
	    let old_signals = self.signals.read();
	    old_signals.clone()
	};

	let auxvs = {
	    let old_auxvs = self.auxvs.read();
	    old_auxvs.clone()
	};

	let (cs, ss) = {
	    let old_context = self.context.read();
	    (old_context.cs, old_context.ss)
	};

	let file_descriptors = {
	    let old_fds = self.file_descriptors.read();
	    old_fds.clone()
	};

	let args = {
	    let old_args = self.args.read();
	    old_args.clone()
	};

	let envvars = {
	    let old_envvars = self.envvars.read();
	    old_envvars.clone()
	};

	let task_type = {
	    let old_task_type = self.task_type.read();
	    old_task_type.clone()
	};

	let cwd = {
	    let old_cwd = self.cwd.read();
	    old_cwd.clone()
	};

	Process {
	    address_space: Arc::new(RwLock::new(new_address_space)),
	    file_descriptors: RwLock::new(file_descriptors.clone()),
	    args: RwLock::new(args),
	    envvars: RwLock::new(envvars),
	    auxvs: RwLock::new(auxvs),
	    context: RwLock::new(ProcessContext {
		gprs: GeneralPurposeRegisters::default(),
		rflags: 0x202,
		rip: rip,
		rsp: 0,
		cs: cs,
		ss: ss,
	    }),
	    state: RwLock::new(TaskState::Setup),
	    task_type: RwLock::new(task_type),
	    cwd: RwLock::new(cwd),
	    signals: RwLock::new(signals),
	}
    }

    pub fn clear(self: Arc<Self>, new_args: Vec<String>, new_envvars: Vec<String>) {
	{
	    let mut address_space = self.address_space.write();
	    address_space.clear_user_space();
	}

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

    pub fn init_stack(self: Arc<Self>) {
	let (rsp, _) = {
	    let mut address_space = self.address_space.write();
	    match memory::user_allocate(
		8 * 1024 * 1024,  // 8MiB
		memory::MemoryAllocationType::RAM,
		memory::MemoryAllocationOptions::Arbitrary,
		memory::MemoryAccessRestriction::User,
		&mut address_space) {
		Ok(i) => i,
		Err(e) => panic!("Could not allocate stack memory for process: {:?}", e),
	    }
	};

	{
	    let mut context = self.context.write();
	    context.rsp = rsp.as_u64() + 8 * 1024 * 1024;  // Start at the end of the stack and grow down
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

    pub fn init_stack_and_start(self: Arc<Self>) {
	let mut context = self.context.write();
	let mut state = self.state.write();
	let mut task_type = self.task_type.write();
	let auxvs = self.auxvs.read();
	let envvars = self.envvars.read();
	let args = self.args.read();

	let envvars_buf_size: usize = envvars.iter()
	    .map(|env_var| env_var.len() + 1)
	    .sum();
	let args_buf_size: usize = args.iter()
	    .map(|arg| arg.len() + 1)
	    .sum();
	
	context.rsp -= envvars_buf_size as u64 + args_buf_size as u64;
	let stack_ptr = VirtAddr::new(context.rsp);
	let data_to = unsafe {
	    slice::from_raw_parts_mut(
		stack_ptr.as_mut_ptr::<u8>(),
		envvars_buf_size + args_buf_size)
	};

	let mut current_offs = envvars_buf_size + args_buf_size;
	let mut envvar_p: Vec<u64> = Vec::new();
	for envvar in envvars.clone() {
	    let envvar_len = envvar.len() + 1;
	    let envvar_cstring = CString::new(envvar.as_str()).unwrap();
	    data_to[current_offs - envvar_len..current_offs].copy_from_slice(envvar_cstring.as_bytes_with_nul());
	    current_offs -= envvar_len;

	    envvar_p.push(context.rsp + current_offs as u64);
	}
	let mut args_p: Vec<u64> = Vec::new();
	for arg in args.clone() {
	    let arg_len = arg.len() + 1;
	    let arg_cstring = CString::new(arg.as_str()).unwrap();
	    data_to[current_offs - arg_len..current_offs].copy_from_slice(arg_cstring.as_bytes_with_nul());

	    current_offs -= arg_len;
	    args_p.push(context.rsp + current_offs as u64);
	}

	context.rsp -= /* auxv = */(auxvs.len() as u64 * 16) +
	    (envvars.len() as u64 * 8) +
	    (args.len() as u64 * 8) +
	/* padding = */(3 * 8);
	let alignment = context.rsp % 16;
	context.rsp -= alignment;  // Align the stack to a u128, as required by the standard
	let stack_ptr = VirtAddr::new(context.rsp).as_mut_ptr::<u64>();

	unsafe {
	    let mut sp = stack_ptr; // mutable walking pointer of type *mut u64

	    // argc
	    core::ptr::write_unaligned(sp, args.len() as u64);
	    sp = sp.add(1);

	    // argv[0..n]
	    for arg in &args_p {
		core::ptr::write_unaligned(sp, *arg);
		sp = sp.add(1);
	    }

	    // NULL after argv
	    core::ptr::write_unaligned(sp, 0);
	    sp = sp.add(1);

	    // envp[0..n]
	    for env in &envvar_p {
		core::ptr::write_unaligned(sp, *env);
		sp = sp.add(1);
	    }

	    // NULL after envp
	    core::ptr::write_unaligned(sp, 0);
	    sp = sp.add(1);

	    // auxv entries (key, value)
	    for auxv in auxvs.iter() {
		core::ptr::write_unaligned(sp, auxv.auxv_type);
		sp = sp.add(1);
		core::ptr::write_unaligned(sp, auxv.value);
		sp = sp.add(1);
	    }

	    // write padding (as bytes)
	    let mut pad_ptr = sp as *mut u8;
	    for _ in 0..alignment {
		core::ptr::write(pad_ptr, 0u8);
		pad_ptr = pad_ptr.add(1);
	    }
	}

	*task_type = TaskType::User;
	*state = TaskState::Running;
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
	context.clone()
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

	let get_next_fd = || {
	    use core::u64;

	    // Fast path: try next-after-max
	    if let Some((&max_key, _)) = file_descriptors.iter().next_back() {
		if max_key < u64::MAX {
		    let candidate = max_key + 1;
		    if !file_descriptors.contains_key(&candidate) {
			return candidate;
		    }
		}
	    }

	    // Slow path: scan from 0 up to u64::MAX
	    for i in 0..=u64::MAX {
		if !file_descriptors.contains_key(&i) {
		    return i;
		}
	    }

	    // Should be unreachable unless every possible key is used
	    panic!("No available u64 keys left!");
	};

	let fd_num = get_next_fd();
	file_descriptors.insert(fd_num, fd);

	fd_num
    }

    // TODO: better error handling for out of FDs
    pub fn emplace_fd_at(self: Arc<Self>, fd: FileDescriptor, mut fd_num: u64, try_greater: bool) -> u64 {
	let mut file_descriptors = self.file_descriptors.write();

	if file_descriptors.contains_key(&fd_num) && !try_greater {
	    panic!("No available file descriptors");
	}

	if file_descriptors.contains_key(&fd_num) {
	    let get_next_fd = || {
		use core::u64;
		// Fast path: try just above the largest key
		if let Some((&max_key, _)) = file_descriptors.iter().next_back() {
		    // If max_key is below min_key, jump to fd_num itself
		    let candidate = max_key.saturating_add(1).max(fd_num);
		    if candidate <= u64::MAX && !file_descriptors.contains_key(&candidate) {
			return candidate;
		    }
		}

		// Slow path: linearly scan starting from min_key
		for i in fd_num..=u64::MAX {
		    if !file_descriptors.contains_key(&i) {
			return i;
		    }
		}

		panic!("No available u64 keys left from {} onward!", fd_num);
	    };

	    fd_num = get_next_fd();
	}

	file_descriptors.insert(fd_num, fd);
	fd_num
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

	return match file_descriptors.remove(&fd) {
	    Some(_) => (),
	    None => panic!("No open FD found"),
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

    pub fn set_cwd(self: Arc<Self>, new_cwd: String) {
	let mut cwd = self.cwd.write();
	*cwd = new_cwd;
    }

    pub fn get_cwd(&self) -> String {
	let cwd = self.cwd.read();
	cwd.clone()
    }	
}
