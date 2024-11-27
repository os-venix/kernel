use alloc::vec::Vec;
use spin::{Once, RwLock};

use crate::memory;

pub struct Process {
    pub address_space: memory::user_address_space::AddressSpace,
    rip: u64,
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

    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    process_tbl.push(Process {
	address_space: address_space,
	rip: 0,
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

pub fn get_active_page_table() -> u64 {
    let process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").read();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	process_tbl[pid].address_space.get_pt4()
    } else {
	panic!("Attempted to access user address space when no process is running");
    }
}

pub fn is_process_running() -> bool {
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();
    running_process.is_some()
}

pub fn set_process_rip(pid: usize, start: u64) {
    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();

    if pid >= process_tbl.len() {
	panic!("Attempted to switch to a nonexistent process");
    }

    process_tbl[pid].rip = start;
}

pub fn start_active_process() {
    let rip = {
	let process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").read();
	let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

	if let Some(pid) = *running_process {
	    process_tbl[pid].rip.clone()
	} else {
	    panic!("Attempted to access user address space when no process is running");
	}
    };

    unsafe {
	core::arch::asm!(
	    "sysretq",

	    in("rcx") rip,
	    in("r11") 0x202,
	);
    }
}
