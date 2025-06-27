use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Once, RwLock, Mutex};
use anyhow::{anyhow, Result};
use alloc::string::String;
use alloc::collections::BTreeMap;
use alloc::boxed::Box;
use core::pin::Pin;
use core::future::Future;
use core::task::Waker;

use crate::sys::vfs;
use crate::drivers::hpet;
use crate::sys::syscall;
use crate::process;

pub mod elf_loader;
pub mod signal;

pub static PROCESS_TABLE: Once<RwLock<BTreeMap<u64, Arc<process::Process>>>> = Once::new();
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
    process_tbl.insert(0, Arc::new(process::Process::new_kthread(idle_thread as usize as u64)));
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
	process_tbl.insert(pid, Arc::new(process::Process::new_kthread(f as usize as u64)));

	let mut running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").write();
	*running_process = Some(pid);
    };
}

pub fn get_current_process() -> Arc<process::Process> {
    let process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").read();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	return process_tbl[&pid].clone();
    } else {
	panic!("Attempted to access user address space when no process is running");
    }
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

	process_tbl.insert(pid, Arc::new(new_process));
    };

    pid
}

pub async fn execve(filename: String, args: Vec<String>, envvars: Vec<String>) {
    let mut args_with_cmd = args.clone();
    args_with_cmd.insert(0, filename.clone());

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
	    process_tbl.get_mut(&pid).unwrap().clone().clear(args_with_cmd, envvars);
	} else {
	    panic!("Attempted to access user address space when no process is running");
	}
    }

    let elf = elf_loader::Elf::new(filename).await.expect("Failed to load ELF");
    let ld = elf_loader::Elf::new(String::from("/usr/lib/ld.so")).await.expect("Failed to load ld.so");

    {
	let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
	let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

	if let Some(pid) = *running_process {
	    process_tbl.get_mut(&pid).unwrap().clone().attach_loaded_elf(elf, ld);
	    process_tbl.get_mut(&pid).unwrap().clone().init_stack();
	    process_tbl.get_mut(&pid).unwrap().clone().init_stack_and_start();
	} else {
	    panic!("Attempted to access user address space when no process is running");
	}
    }
}

pub fn exit(exit_code: u64) -> ! {
    if exit_code != 0 {
	log::info!("Exited with code {}", exit_code);
    }

    {
	let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
	let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

	if let Some(pid) = *running_process {
	    // Free associated memory, and drop the process
	    process_tbl.get_mut(&pid).unwrap().clone().clear(Vec::new(), Vec::new());
	    process_tbl.remove(&pid);
	} else {
	    panic!("Attempted to access user address space when no process is running");
	}
    }

    schedule_next();
}

pub fn set_registers_for_current_process(rsp: u64, rip: u64, rflags: u64, registers: &process::GeneralPurposeRegisters) {
    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	process_tbl.get_mut(&pid).unwrap().clone().set_registers(rsp, rip, rflags, registers)
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
	match &mut process.get_state() {
	    process::TaskState::Setup => {},
	    process::TaskState::Running => {},
	    process::TaskState::AsyncSyscall { future, waker } => {
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
	let mut running_process = RUNNING_PROCESS
	    .get()
	    .expect("RUNNING_PROCESS not initialized")
	    .write();
	let mut process_tbl = PROCESS_TABLE
	    .get()
	    .expect("PROCESS_TABLE not initialized")
	    .write();

	let process = process_tbl.get_mut(&pid).unwrap();
	// Switch to address space
	*running_process = Some(pid);
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
	    process_tbl.get_mut(&pid).unwrap().clone().syscall_return(result.return_value as u64, result.err_num);
        }
        core::task::Poll::Pending => {	    
	    let mut process_tbl = PROCESS_TABLE
		.get()
		.expect("PROCESS_TABLE not initialized")
		.write();
	    let process = process_tbl.get_mut(&pid).unwrap();
	    process.clone().set_state(process::TaskState::AsyncSyscall {
                future,
                waker: Some(waker),
            });
	}
    }
}

fn next_task() -> process::ProcessContext {
    let mut running_process = RUNNING_PROCESS
	.get()
	.expect("RUNNING_PROCESS not initialized")
	.write();

    let mut process_tbl = PROCESS_TABLE
	.get()
	.expect("PROCESS_TABLE not initialized")
	.write();

    // Pull out idle process context; this is where we go if nothing else is currently runnable
    let idle_ctx = process_tbl.get(&0).unwrap().get_context();

    // Convert to vector to allow indexed wraparound search
    let mut tasks: Vec<(u64, &mut Arc<process::Process>)> = process_tbl.iter_mut().map(|(pid, p)| (*pid, p)).collect();
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

	match &mut process.get_state() {
	    process::TaskState::Setup => {},
            process::TaskState::Running => {
		*running_process = Some(*pid);

		// Switch to address space
		let address_space = process.address_space.read();
		unsafe {
                    address_space.switch_to();
		}

		return process.get_context();
            }
            process::TaskState::AsyncSyscall { future: _, waker: _ } => { },
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
extern "C" fn context_switch(context: &process::ProcessContext) -> ! {
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

pub fn open_fd(file: String, flags: u64) -> u64 {
    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	let fd = process::FileDescriptor {
	    flags: flags,
	    file_description: Arc::new(RwLock::new(vfs::FileDescriptor::new(file))),
	};
	process_tbl.get_mut(&pid).unwrap().clone().emplace_fd(fd)
    } else {
	panic!("Attempted to open a file on a nonexistent process");
    }
}

pub fn pipe_fd(flags: u64) -> (u64, u64) {
    let file_description = Arc::new(RwLock::new(vfs::FileDescriptor::new_pipe()));

    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	// TODO - one of these should be read, the other write
	let fd1 = process::FileDescriptor {
	    flags: flags,
	    file_description: file_description.clone(),
	};
	let fd2 = process::FileDescriptor {
	    flags: flags,
	    file_description: file_description.clone(),
	};

	let fd1_number = process_tbl.get_mut(&pid).unwrap().clone().emplace_fd(fd1);
	let fd2_number = process_tbl.get_mut(&pid).unwrap().clone().emplace_fd(fd2);

	(fd1_number, fd2_number)
    } else {
	panic!("Attempted to open a file on a nonexistent process");
    }
}

pub fn dup_fd(fd: process::FileDescriptor) -> u64 {
    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    let mut running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").write();

    if let Some(pid) = *running_process {
	process_tbl.get_mut(&pid).unwrap().clone().emplace_fd(fd)
    } else {
	panic!("Attempted to read open FDs on nonexistent process");
    }
}

pub fn dup_fd_exact(fd: process::FileDescriptor, mut fd_num: u64, try_greater: bool) -> Result<u64, syscall::CanonicalError> {
    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    let mut running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").write();

    if let Some(pid) = *running_process {
	Ok(process_tbl.get_mut(&pid).unwrap().clone().emplace_fd_at(fd, fd_num, try_greater))
    } else {
	panic!("Attempted to read open FDs on nonexistent process");
    }
}

pub fn get_actual_fd(fd: u64) -> Result<process::FileDescriptor> {
    let process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").read();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	Ok(process_tbl[&pid].get_file_descriptor(fd))
    } else {
	panic!("Attempted to read open FDs on nonexistent process");
    }
}

pub fn set_fd_flags(fd: u64, flags: u64) -> Result<()> {
    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	if let Some(process) = process_tbl.get_mut(&pid) {
	    process.clone().set_fd_flags(fd, flags);
	    Ok(())
	} else {
	    panic!("Attempted to read open FDs on nonexistent process");
	}
    } else {
	panic!("Attempted to read open FDs on nonexistent process");
    }
}

pub fn close_fd(fd: u64) -> Result<()> {
    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	process_tbl.get_mut(&pid).unwrap().clone().close_fd(fd);
	Ok(())
    } else {
	panic!("Attempted to open a file on a nonexistent process");
    }
}

pub fn set_task_state(state: process::TaskState) {
    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	process_tbl.get_mut(&pid).unwrap().clone().set_state(state);
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

pub fn get_current_pid() -> u64 {    
    let mut running_process = RUNNING_PROCESS
	.get()
	.expect("RUNNING_PROCESS not initialized")
	.read();
    running_process.expect("Couldn't find running PID")
}

pub fn get_current_cwd() -> String {
    let process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").read();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	process_tbl[&pid].clone().get_cwd()
    } else {
	panic!("Attempted to access user address space when no process is running");
    }
}

pub fn install_signal_handler(signal: u64, handler: u64) {
    let signal_handler = signal::parse_sigaction(handler);

    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	process_tbl.get_mut(&pid).unwrap().clone().install_signal_handler(signal, signal_handler);
    } else {
	panic!("Attempted to access user address space when no process is running");
    }
}
