use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Once, RwLock, Mutex};
use alloc::collections::BTreeMap;
use alloc::boxed::Box;

use crate::drivers::hpet;
use crate::sys::syscall;
use crate::process;

pub mod elf_loader;
pub mod signal;
mod process_waker;

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
    process_tbl.insert(0, Arc::new(process::Process::new_kthread(idle_thread as *const () as usize as u64)));
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

pub fn get_current_pid() -> u64 {    
    let running_process = RUNNING_PROCESS
	.get()
	.expect("RUNNING_PROCESS not initialized")
	.read();
    running_process.expect("Couldn't find running PID")
}

pub fn get_process_by_id(id: u64) -> Option<Arc<process::Process>> {
    let process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").read();

    process_tbl.get(&id).cloned()
}

pub fn get_current_process() -> Arc<process::Process> {
    let process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").read();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	process_tbl[&pid].clone()
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

	let new_process = process::Process::from_existing(
	    &process_tbl[&running_process.expect("No running process")], rip);

	process_tbl.insert(pid, Arc::new(new_process));
    };

    pid
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
	    let current_process = process_tbl.get_mut(&pid).unwrap().clone();
	    let mut task_type = current_process.task_type.write();

	    if let process::TaskType::User(ref mut address_space) = *task_type {
		address_space.clear_user_space();
	    }
	    process_tbl.remove(&pid);
	} else {
	    panic!("Attempted to access user address space when no process is running");
	}
    }

    schedule_next();
}

fn get_futures_to_poll() -> BTreeMap<u64, Arc<Mutex<process::SyscallFuture>>> {
    let mut r: BTreeMap<u64, Arc<Mutex<process::SyscallFuture>>> = BTreeMap::new();
    let mut process_tbl = PROCESS_TABLE
	.get()
	.expect("PROCESS_TABLE not initialized")
	.write();

    for (pid, process) in process_tbl.iter_mut() {
	match &mut process.get_state() {
	    process::TaskState::Setup => {},
	    process::TaskState::Running => {},
	    process::TaskState::Waiting { future: _ } => {},
	    process::TaskState::AsyncSyscall { future } => {
		let dummy: process::SyscallFuture = Box::pin(async {
		    syscall::SyscallResult {
			return_value: 0,
			err_num: syscall::CanonicalError::Ok as u64,
		    }
		});
		let future_taken = core::mem::replace(future, Arc::new(Mutex::new(dummy)));
		r.insert(*pid, future_taken);
	    }
	}
    }

    r
}

fn poll_process_future(pid: u64, future: Arc<Mutex<process::SyscallFuture>>) {
    use core::task::Context;

    let waker = process_waker::ProcessWaker::new(pid);
    let mut ctx = Context::from_waker(&waker);

    {
	let mut running_process = RUNNING_PROCESS
	    .get()
	    .expect("RUNNING_PROCESS not initialized")
	    .write();
	*running_process = Some(pid);
    }

    match future.clone().lock().as_mut().poll(&mut ctx) {
        core::task::Poll::Ready(result) => {
	    let mut process_tbl = PROCESS_TABLE
		.get()
		.expect("PROCESS_TABLE not initialized")
		.write();
	    process_tbl.get_mut(&pid).unwrap().clone().syscall_return(result.return_value, result.err_num);
        }
        core::task::Poll::Pending => {	    
	    let mut process_tbl = PROCESS_TABLE
		.get()
		.expect("PROCESS_TABLE not initialized")
		.write();
	    let process = process_tbl.get_mut(&pid).unwrap();
	    process.clone().set_state(process::TaskState::Waiting {
                future,
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
    let current_pid = *running_process;

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

	if let process::TaskState::Running = process.get_state() {
	    *running_process = Some(*pid);

	    // Switch to address space
	    let mut task_type = process.task_type.write();
	    if let process::TaskType::User(ref mut address_space) = *task_type {
		unsafe {
		    address_space.switch_to();
		}
	    }

	    return process.get_context();
        }
    }

    *running_process = Some(0);
    idle_ctx
}

fn context_switch(context: &process::ProcessContext) -> ! {    
    #[unsafe(naked)]
    #[allow(named_asm_labels)]
    unsafe extern "C" fn inner() -> ! {
	core::arch::naked_asm!(
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
	);
    }

    let ptr = context as *const process::ProcessContext;
    unsafe {
	core::arch::asm!(
	    "mov rdi, {0}",
	    "jmp {stub}",

	    in(reg) ptr,
	    stub = sym inner,
	    options(noreturn),
	);
    }
}

// TODO: use waker-based queues to avoid the need to continually poll.
pub fn schedule_next() -> ! {
    let futures = get_futures_to_poll();

    for (pid, future) in futures {
	poll_process_future(pid, future);
    }

    let context = next_task();
    context_switch(&context);
}
