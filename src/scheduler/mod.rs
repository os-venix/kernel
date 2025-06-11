use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Once, RwLock, Mutex};
use anyhow::{anyhow, Result};
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
use crate::drivers::hpet;
use crate::sys::syscall;
use crate::gdt;

mod elf_loader;

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
struct ProcessContext {
    gprs: GeneralPurposeRegisters,
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

pub enum TaskState {
    Setup,
    Running,
    AsyncSyscall {
	future: Arc<Mutex<Pin<Box<dyn Future<Output = syscall::SyscallResult> + Send + 'static>>>>,
	waker: Option<Waker>,
    },
}

pub enum TaskType {
    User,
    Kernel,
}

pub struct Process {
    pub address_space: Arc<RwLock<memory::user_address_space::AddressSpace>>,
    file_descriptors: BTreeMap<u64, Arc<RwLock<vfs::FileDescriptor>>>,
    next_fd: u64,
    args: Vec<String>,
    envvars: Vec<String>,
    auxvs: Vec<AuxVector>,
    stack_bottom: VirtAddr,
    context: ProcessContext,
    state: TaskState,
    task_type: TaskType,
}

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
	    file_descriptors: BTreeMap::new(),
	    next_fd: 0,
	    args: vec!(String::from("init")),
	    envvars: vec!(String::from("PATH=/bin:/usr/bin")),
	    auxvs: Vec::new(),
	    stack_bottom: rsp,
	    context: ProcessContext {
		gprs: GeneralPurposeRegisters::default(),
		rflags: 0x202,
		rip: rip,
		rsp: rsp.as_u64() + (8 * 1024 * 1024),
		cs: kernel_code.0 as u64,
		ss: kernel_data.0 as u64,
	    },
	    state: TaskState::Running,
	    task_type: TaskType::Kernel,
	}
    }

    pub fn from_existing(&self, rip: u64) -> Self {
	let address_space = self.address_space.read();
	let mut new_address_space = memory::user_address_space::AddressSpace::new();
	unsafe {
	    new_address_space.switch_to();
	}

	new_address_space.create_copy_of_address_space(&address_space);

	Process {
	    address_space: Arc::new(RwLock::new(new_address_space)),
	    file_descriptors: self.file_descriptors.clone(),
	    next_fd: self.next_fd,
	    args: self.args.clone(),
	    envvars: self.envvars.clone(),
	    auxvs: self.auxvs.clone(),
	    stack_bottom: self.stack_bottom,
	    context: ProcessContext {
		gprs: GeneralPurposeRegisters::default(),
		rflags: 0x202,
		rip: rip,
		rsp: 0,
		cs: self.context.cs,
		ss: self.context.ss,
	    },
	    state: TaskState::Setup,
	    task_type: TaskType::User,
	}
    }

    pub fn clear(&mut self, args: Vec<String>, envvars: Vec<String>) {
	{
	    let mut address_space = self.address_space.write();
	    address_space.clear_user_space();
	}

	self.args = args;
	self.envvars = envvars;
	self.context.gprs = GeneralPurposeRegisters::default();
	self.context.rflags = 0x202;
	self.auxvs = Vec::new();
    }

    pub fn init_stack(&mut self) {
	let mut address_space = self.address_space.write();
	let (rsp, _) = match memory::user_allocate(
	    8 * 1024 * 1024,  // 8MiB
	    memory::MemoryAllocationType::RAM,
	    memory::MemoryAllocationOptions::Arbitrary,
	    memory::MemoryAccessRestriction::User,
	    &mut address_space) {
	    Ok(i) => i,
	    Err(e) => panic!("Could not allocate stack memory for process: {:?}", e),
	};

	self.stack_bottom = rsp;
	self.context.rsp = rsp.as_u64() + 8 * 1024 * 1024;  // Start at the end of the stack and grow down
    }

    pub fn attach_loaded_elf(&mut self, elf: elf_loader::Elf, ld_so: elf_loader::Elf) {
	self.auxvs.push(AuxVector {
	    auxv_type: AT_BASE,
	    value: ld_so.base
	});
	self.auxvs.push(AuxVector {
	    auxv_type: AT_ENTRY,
	    value: elf.entry
	});
	self.auxvs.push(AuxVector {
	    auxv_type: AT_PHDR,
	    value: elf.program_header
	});
	self.auxvs.push(AuxVector {
	    auxv_type: AT_PHENT,
	    value: elf.program_header_entry_size
	});
	self.auxvs.push(AuxVector {
	    auxv_type: AT_PHNUM,
	    value: elf.program_header_entry_count
	});
	self.auxvs.push(AuxVector {
	    auxv_type: AT_NUL,
	    value: 0
	});

	self.context.rip = ld_so.entry;
    }

    pub fn init_stack_and_start(&mut self) {
	let envvars_buf_size: usize = self.envvars.iter()
	    .map(|env_var| env_var.len() + 1)
	    .sum();
	let args_buf_size: usize = self.args.iter()
	    .map(|arg| arg.len() + 1)
	    .sum();
	
	self.context.rsp -= envvars_buf_size as u64 + args_buf_size as u64;
	let stack_ptr = VirtAddr::new(self.context.rsp);
	let data_to = unsafe {
	    slice::from_raw_parts_mut(
		stack_ptr.as_mut_ptr::<u8>(),
		envvars_buf_size + args_buf_size)
	};

	let mut current_offs = envvars_buf_size + args_buf_size;
	let mut envvar_p: Vec<u64> = Vec::new();
	for envvar in self.envvars.clone() {
	    let envvar_len = envvar.len() + 1;
	    let envvar_cstring = CString::new(envvar.as_str()).unwrap();
	    data_to[current_offs - envvar_len..current_offs].copy_from_slice(envvar_cstring.as_bytes_with_nul());
	    current_offs -= envvar_len;

	    envvar_p.push(self.context.rsp + current_offs as u64);
	}
	let mut args_p: Vec<u64> = Vec::new();
	for arg in self.args.clone() {
	    let arg_len = arg.len() + 1;
	    let arg_cstring = CString::new(arg.as_str()).unwrap();
	    data_to[current_offs - arg_len..current_offs].copy_from_slice(arg_cstring.as_bytes_with_nul());

	    current_offs -= arg_len;
	    args_p.push(self.context.rsp + current_offs as u64);
	}

	self.context.rsp -= /* auxv = */(self.auxvs.len() as u64 * 16) +
	    (self.envvars.len() as u64 * 8) +
	    (self.args.len() as u64 * 8) +
	/* padding = */(3 * 8);
	let alignment = 16 - (self.context.rsp % 16);
	self.context.rsp -= alignment;  // Align the stack to a u128, as required by the standard
	let stack_ptr = VirtAddr::new(self.context.rsp).as_mut_ptr::<u64>();

	unsafe {
	    let mut sp = stack_ptr; // mutable walking pointer of type *mut u64

	    // argc
	    core::ptr::write_unaligned(sp, self.args.len() as u64);
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
	    for auxv in &self.auxvs {
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

	self.task_type = TaskType::User;
	self.state = TaskState::Running;
    }

    pub fn set_registers(&mut self, rsp: u64, rip: u64, rflags: u64, registers: &GeneralPurposeRegisters) {
	self.context.rsp = rsp;
	self.context.rip = rip;
	self.context.rflags = rflags;
	self.context.gprs = *registers;
    }
}

pub static PROCESS_TABLE: Once<RwLock<BTreeMap<u64, Process>>> = Once::new();
pub static RUNNING_PROCESS: Once<RwLock<Option<u64>>> = Once::new();
pub static NEXT_PID: Once<Mutex<u64>> = Once::new();

fn idle_thread() -> ! {
    loop {
        unsafe { core::arch::asm!("hlt"); }
    }
}

pub fn init() {
    PROCESS_TABLE.call_once(|| RwLock::new(BTreeMap::new()));
    RUNNING_PROCESS.call_once(|| RwLock::new(None));
    NEXT_PID.call_once(|| Mutex::new(1));  // PID 0 is idle thread

    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    process_tbl.insert(0, Process::new_kthread(idle_thread as usize as u64));
}

pub fn start() -> ! {
    // This function doesn't actually need to do anything at all. This provides a stable, monotonic tick to the kernel.
    // By virtue of the fact that interrutps all return via the scheduler, a new process will always be scheduled as appropriate.
    hpet::add_periodic(1, Box::new(|| {}));
    schedule_next();
}

pub fn kthread_start(f: fn() -> !) {
    let pid = {
	let mut next_pid = NEXT_PID.get().expect("Attempted to access next PID before it is initialised").lock();
	let pid = *next_pid;
	*next_pid += 1;
	pid
    };

    {
	let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
	process_tbl.insert(pid, Process::new_kthread(f as usize as u64));

	let mut running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").write();
	*running_process = Some(pid);
    };
}

pub fn fork_current_process(rip: u64) -> u64 {
    let pid = {
	let mut next_pid = NEXT_PID.get().expect("Attempted to access next PID before it is initialised").lock();
	let pid = *next_pid;
	*next_pid += 1;
	pid
    };

    {
	let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();
	let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();

	let new_process = process_tbl[&running_process.expect("No running process")].from_existing(rip);

	process_tbl.insert(pid, new_process);
    };

    pid
}

pub async fn execve(filename: String, args: Vec<String>, envvars: Vec<String>) {
    {
	let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
	let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

	if let Some(pid) = *running_process {
	    // Switch to address space
	    {
		let address_space = process_tbl.get_mut(&pid).unwrap().address_space.read();
		unsafe {
                    address_space.switch_to();
		}
	    }
	    process_tbl.get_mut(&pid).unwrap().clear(args, envvars);
	} else {
	    panic!("Attempted to access user address space when no process is running");
	}
    }

    let elf = elf_loader::Elf::new(filename).await.expect("Failed to load ELF");
    let ld = elf_loader::Elf::new(String::from("/lib/ld.so")).await.expect("Failed to load ld.so");

    {
	let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
	let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

	if let Some(pid) = *running_process {
	    process_tbl.get_mut(&pid).unwrap().attach_loaded_elf(elf, ld);
	    process_tbl.get_mut(&pid).unwrap().init_stack();
	    process_tbl.get_mut(&pid).unwrap().init_stack_and_start();
	} else {
	    panic!("Attempted to access user address space when no process is running");
	}
    }
}

pub fn exit() -> ! {
    {
	let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
	let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

	if let Some(pid) = *running_process {
	    // Free associated memory, and drop the process
	    process_tbl.get_mut(&pid).unwrap().clear(Vec::new(), Vec::new());
	    process_tbl.remove(&pid);
	} else {
	    panic!("Attempted to access user address space when no process is running");
	}
    }

    schedule_next();
}

pub fn set_registers_for_current_process(rsp: u64, rip: u64, rflags: u64, registers: &GeneralPurposeRegisters) {
    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	process_tbl.get_mut(&pid).unwrap().set_registers(rsp, rip, rflags, registers)
    } else {
	panic!("Attempted to access user address space when no process is running");
    }
}

fn get_futures_to_poll() -> BTreeMap<u64, (Arc<Mutex<Pin<Box<dyn Future<Output = syscall::SyscallResult> + Send + 'static>>>>, Option<Waker>)> {
    let mut r: BTreeMap<u64, (Arc<Mutex<Pin<Box<dyn Future<Output = syscall::SyscallResult> + Send + 'static>>>>, Option<Waker>)> = BTreeMap::new();
    let mut process_tbl = PROCESS_TABLE
	.get()
	.expect("PROCESS_TABLE not initialized")
	.write();

    for (pid, process) in process_tbl.iter_mut() {
	match &mut process.state {
	    TaskState::Setup => {},
	    TaskState::Running => {},
	    TaskState::AsyncSyscall { future, waker } => {
		let dummy: Pin<Box<dyn Future<Output = syscall::SyscallResult> + Send + 'static>> = Box::pin(async {		
		    syscall::SyscallResult {
			return_value: 0,
			err_num: syscall::CanonicalError::EOK as u64,
		    }
		});
		let future_taken = core::mem::replace(future, Arc::new(Mutex::new(dummy)));
		r.insert(*pid, (future_taken, waker.take()));
	    }
	}
    }

    r
}

fn poll_process_future(pid: u64, future: Arc<Mutex<Pin<Box<dyn Future<Output = syscall::SyscallResult> + Send + 'static>>>>, waker: Option<Waker>) {
    use core::task::{RawWaker, RawWakerVTable, Context};

    fn dummy_raw_waker() -> RawWaker {
        fn clone(_: *const ()) -> RawWaker { dummy_raw_waker() }
        fn wake(_: *const ()) {}
        fn wake_by_ref(_: *const ()) {}
        fn drop(_: *const ()) {}

        RawWaker::new(core::ptr::null(), &RawWakerVTable::new(clone, wake, wake_by_ref, drop))
    }

    let waker = waker
        .unwrap_or_else(|| unsafe { Waker::from_raw(dummy_raw_waker()) });

    let mut ctx = Context::from_waker(&waker);

    {	
	let mut process_tbl = PROCESS_TABLE
	    .get()
	    .expect("PROCESS_TABLE not initialized")
	    .write();
	let process = process_tbl.get_mut(&pid).unwrap();
	// Switch to address space
	let address_space = process.address_space.read();
	unsafe {
            address_space.switch_to();
	}
    }

    match future.clone().lock().as_mut().poll(&mut ctx) {
        core::task::Poll::Ready(result) => {
	    let mut process_tbl = PROCESS_TABLE
		.get()
		.expect("PROCESS_TABLE not initialized")
		.write();
	    let process = process_tbl.get_mut(&pid).unwrap();

	    process.state = TaskState::Running;
	    process.context.gprs.rax = result.return_value as u64;
	    process.context.gprs.rdx = result.err_num;
        }
        core::task::Poll::Pending => {	    
	    let mut process_tbl = PROCESS_TABLE
		.get()
		.expect("PROCESS_TABLE not initialized")
		.write();
	    let process = process_tbl.get_mut(&pid).unwrap();
	    process.state = TaskState::AsyncSyscall {
                future,
                waker: Some(waker),
            };
	}
    }
}

fn next_task() -> ProcessContext {
    let mut running_process = RUNNING_PROCESS
	.get()
	.expect("RUNNING_PROCESS not initialized")
	.write();

    let mut process_tbl = PROCESS_TABLE
	.get()
	.expect("PROCESS_TABLE not initialized")
	.write();

    // Pull out idle process context; this is where we go if nothing else is currently runnable
    let idle_ctx = process_tbl.get(&0).unwrap().context.clone();

    // Convert to vector to allow indexed wraparound search
    let mut tasks: Vec<(u64, &mut Process)> = process_tbl.iter_mut().map(|(pid, p)| (*pid, p)).collect();
    tasks.sort_by_key(|(pid, _)| *pid); // Ensure stable order

    // Get current PID
    let current_pid = running_process.clone();

    // Find index of current process (if any)
    let start_idx = current_pid
	.and_then(|pid| tasks.iter().position(|(p, _)| *p == pid))
	.map(|i| (i + 1) % tasks.len()) // Start just after current PID
	.unwrap_or(0);

    let tasks_len = tasks.len();
    for i in 0..tasks_len {
	let (pid, process) = &mut tasks[(start_idx + i) % tasks_len];

	// Idle thread is thread of last resort. Only schedule it if nothing else is found.
	if *pid == 0 {
	    continue;
	}

	match &mut process.state {
	    TaskState::Setup => {},
            TaskState::Running => {
		*running_process = Some(*pid);

		// Switch to address space
		let address_space = process.address_space.read();
		unsafe {
                    address_space.switch_to();
		}

		return process.context;
            }
            TaskState::AsyncSyscall { future: _, waker: _ } => { },
	}
    }

    *running_process = Some(0);

    {
	// Switch to address space
	let address_space = process_tbl.get(&0).unwrap().address_space.read();
	unsafe {
            address_space.switch_to();
	}
    }
    idle_ctx
}

#[naked]
#[allow(named_asm_labels)]
extern "C" fn context_switch(context: &ProcessContext) -> ! {
    unsafe {
	core::arch::asm!(
	    // First, build up the stack frame for the iret
	    "mov rcx, [rdi + 0x98]",  // SS
	    "mov rbx, [rdi + 0x90]",  // CS
	    "mov rax, [rdi + 0x88]",  // RSP
	    "mov rdx, [rdi + 0x80]",  // RIP
	    "mov rsi, [rdi + 0x78]",  // RFLAGS

	    "push rcx",
	    "push rax",
	    "push rsi",
	    "push rbx",
	    "push rdx",

	    // Next, restore the registers themselves
	    "mov r15, [rdi + 0x00]",
	    "mov r14, [rdi + 0x08]",
	    "mov r13, [rdi + 0x10]",
	    "mov r12, [rdi + 0x18]",
	    "mov r11, [rdi + 0x20]",
	    "mov r10, [rdi + 0x28]",
	    "mov r9, [rdi + 0x30]",
	    "mov r8, [rdi + 0x38]",
	    "mov rbp, [rdi + 0x40]",
	    // RDI would go here, but has to be done at the end
	    "mov rsi, [rdi + 0x50]",
	    "mov rdx, [rdi + 0x58]",
	    "mov rcx, [rdi + 0x60]",
	    "mov rbx, [rdi + 0x68]",
	    "mov rax, [rdi + 0x70]",
	    "mov rdi, [rdi + 0x48]",

	    // Next, swap GS if needed
	    "test qword ptr [rsp + 0x08], 0x03",
	    "je 3f",
	    "swapgs",
	    "3:",

	    // Lastly, iret to the process
	    "iretq",

	    options(noreturn),
	);
    }
}

// TODO: use waker-based queues to avoid the need to continually poll.
pub fn schedule_next() -> ! {
    let futures = get_futures_to_poll();

    for (pid, f) in futures {
	let future = f.0;
	let waker = f.1;
	poll_process_future(pid, future, waker);
    }

    let context = next_task();    
    context_switch(&context);
}

pub fn open_fd(file: String) -> u64 {
    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	let fd = Arc::new(RwLock::new(vfs::FileDescriptor::new(file)));
	let fd_number = process_tbl[&pid].next_fd;
	process_tbl.get_mut(&pid).unwrap().next_fd += 1;

	process_tbl.get_mut(&pid).unwrap().file_descriptors.insert(fd_number, fd);
	fd_number
    } else {
	panic!("Attempted to open a file on a nonexistent process");
    }
}

pub fn get_actual_fd(fd: u64) -> Result<Arc<RwLock<vfs::FileDescriptor>>> {
    let process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").read();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	if let Some(actual_fd) = process_tbl[&pid].file_descriptors.get(&fd) {
	    Ok(actual_fd.clone())
	} else {
	    Err(anyhow!("Attempted to access nonexistent file descriptor"))
	}
    } else {
	panic!("Attempted to read open FDs on nonexistent process");
    }
}

pub fn close_fd(fd: u64) -> Result<()> {
    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	return match process_tbl.get_mut(&pid).unwrap().file_descriptors.remove(&fd) {
	    Some(_) => Ok(()),
	    None => Err(anyhow!("No open FD found")),
	}
    } else {
	panic!("Attempted to open a file on a nonexistent process");
    }
}

pub fn set_task_state(state: TaskState) {
    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	process_tbl.get_mut(&pid).unwrap().state = state;
    } else {
	panic!("Attempted to open a file on a nonexistent process");
    }
}

pub fn get_active_page_table() -> u64 {
    let process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").read();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	let address_space = process_tbl[&pid].address_space.read();
	address_space.get_pt4()
    } else {
	panic!("Attempted to access user address space when no process is running");
    }
}
