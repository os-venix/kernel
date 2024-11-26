use alloc::vec::Vec;
use spin::{Once, RwLock};

use crate::memory;

pub struct Process {
    pub address_space: memory::user_address_space::AddressSpace,
}

pub static PROCESS_TABLE: Once<RwLock<Vec<Process>>> = Once::new();
pub static RUNNING_PROCESS: Once<RwLock<Option<usize>>> = Once::new();

pub fn init() {
    PROCESS_TABLE.call_once(|| RwLock::new(Vec::new()));
    RUNNING_PROCESS.call_once(|| RwLock::new(None));
}

pub fn start_new_process() -> usize {
    let address_space = memory::user_address_space::AddressSpace::new();
    unsafe {
	address_space.switch_to();
    }
    loop {}

    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    process_tbl.push(Process {
	address_space: address_space
    });

    let mut running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").write();
    *running_process = Some(process_tbl.len() - 1);

    process_tbl.len() - 1
}

pub fn deschedule() -> Option<usize> {
    let mut running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").write();
    let current_pid = *running_process;
    *running_process = None;

    current_pid
}

pub fn switch_to(pid: usize) {
    let process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").read();

    if pid >= process_tbl.len() {
	panic!("Attempted to switch to a nonexistent process");
    }

    unsafe {
	process_tbl[pid].address_space.switch_to();
    }

    let mut running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").write();
    *running_process = Some(pid);
}
