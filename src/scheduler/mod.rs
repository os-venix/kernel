use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Once, RwLock};
use anyhow::{anyhow, Result};
use alloc::string::String;
use x86_64::VirtAddr;
use alloc::ffi::CString;
use alloc::slice;
use alloc::vec;

use crate::memory;
use crate::sys::vfs;

mod elf_loader;

pub struct Process {
    pub address_space: memory::user_address_space::AddressSpace,
    file_descriptors: Vec<Arc<vfs::FileDescriptor>>,
    at_entry: VirtAddr,
    at_base: VirtAddr,
    args: Vec<String>,
    envvars: Vec<String>,
    rip: u64,
    rsp: u64,
}

impl Process {
    pub fn new() -> Self {
	let mut address_space = memory::user_address_space::AddressSpace::new();
	unsafe {
	    address_space.switch_to();
	}

	Process {
	    address_space: address_space,
	    file_descriptors: Vec::new(),
	    at_entry: VirtAddr::new(0),
	    at_base: VirtAddr::new(0),
	    args: vec!(String::from("init")),
	    envvars: vec!(String::from("PATH=/bin:/usr/bin")),
	    rip: 0,
	    rsp: 0,
	}
    }

    pub fn init_stack(&mut self) {
	let (rsp, _) = match memory::kernel_allocate(
	    8 * 1024 * 1024,  // 8MiB
	    memory::MemoryAllocationType::RAM,
	    memory::MemoryAllocationOptions::Arbitrary,
	    memory::MemoryAccessRestriction::UserByAddressSpace(&mut self.address_space)) {
	    Ok(i) => i,
	    Err(e) => panic!("Could not allocate stack memory for process: {:?}", e),
	};

	self.rsp = rsp.as_u64();
    }

    pub fn attach_loaded_elf(&mut self, elf: elf_loader::Elf) {
	self.at_entry = VirtAddr::new(elf.entry);
	self.rip = elf.entry;
    }

    pub fn init_stack_and_start(&mut self) -> ! {
	let envvars_buf_size: usize = self.envvars.iter()
	    .map(|env_var| env_var.len() + 1)
	    .sum();
	let args_buf_size: usize = self.args.iter()
	    .map(|arg| arg.len() + 1)
	    .sum();
	
	self.rsp -= envvars_buf_size as u64 + args_buf_size as u64;
	let stack_ptr = VirtAddr::new(self.rsp);
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

	    envvar_p.push(self.rsp + current_offs as u64);
	}
	let mut args_p: Vec<u64> = Vec::new();
	for arg in self.args.clone() {
	    let arg_len = arg.len() + 1;
	    let arg_cstring = CString::new(arg.as_str()).unwrap();
	    data_to[current_offs - arg_len..current_offs].copy_from_slice(arg_cstring.as_bytes_with_nul());

	    current_offs -= arg_len;
	    args_p.push(self.rsp + current_offs as u64);
	}


	self.rsp -= /* auxv = */(2 * 16) +
	    (self.envvars.len() as u64 * 8) +
	    (self.args.len() as u64 * 8) +
	/* padding = */(4 * 8);
	let stack_ptr = VirtAddr::new(self.rsp);

	let ptrs_to = unsafe {
	    slice::from_raw_parts_mut(
		stack_ptr.as_mut_ptr::<u64>(),
		/* auxv = */(2 * 2) + self.envvars.len() + self.args.len() + 4,
	    )
	};

	ptrs_to[0] = self.args.len() as u64;
	for (i, arg) in args_p.iter().enumerate() {
	    ptrs_to[1 + i] = *arg;
	}
	ptrs_to[1 + args_p.len()] = 0;
	for (i, envvar) in envvar_p.iter().enumerate() {
	    ptrs_to[2 + args_p.len() + i] = *envvar;
	}
	ptrs_to[2 + args_p.len() + envvar_p.len()] = 0;

	ptrs_to[3 + args_p.len() + envvar_p.len()] = 7;  // AT_BASE
	ptrs_to[4 + args_p.len() + envvar_p.len()] = self.at_base.as_u64();

	ptrs_to[5 + args_p.len() + envvar_p.len()] = 9;  // AT_ENTRY
	ptrs_to[6 + args_p.len() + envvar_p.len()] = self.at_entry.as_u64();

	ptrs_to[7 + args_p.len() + envvar_p.len()] = 0;  // AT_NULL	

	log::info!("RSP 0x{:x}", self.rsp);
	unsafe {
	    core::arch::asm!(
		"mov rsp, {stackptr}",
		"sysretq",

		in("rcx") self.rip,
		stackptr = in(reg) self.rsp,
		in("r11") 0x202,
		options(noreturn),
	    );
	}
    }
}

pub static PROCESS_TABLE: Once<RwLock<Vec<Process>>> = Once::new();
pub static RUNNING_PROCESS: Once<RwLock<Option<usize>>> = Once::new();

pub fn init() {
    PROCESS_TABLE.call_once(|| RwLock::new(Vec::new()));
    RUNNING_PROCESS.call_once(|| RwLock::new(None));
}

pub fn start_new_process(filename: String) -> usize {
    let pid = {
	let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
	process_tbl.push(Process::new());

	let mut running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").write();

	let pid = process_tbl.len() - 1;
	*running_process = Some(pid);
	pid
    };
    let elf = elf_loader::Elf::new(filename).expect("Failed to load ELF");
    {
	let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
	process_tbl[pid].attach_loaded_elf(elf);
	process_tbl[pid].init_stack();
    }

    pid
}

pub fn open_fd(file: String) -> u64 {
    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	let fd = Arc::new(vfs::FileDescriptor::new(file));
	process_tbl[pid].file_descriptors.push(fd);

	process_tbl[pid].file_descriptors.len() as u64 - 1
    } else {
	panic!("Attempted to open a file on a nonexistent process");
    }
}

pub fn get_actual_fd(fd: u64) -> Result<Arc<vfs::FileDescriptor>> {
    let process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").read();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	if let Some(actual_fd) = process_tbl[pid].file_descriptors.get(fd as usize) {
	    Ok(actual_fd.clone())
	} else {
	    Err(anyhow!("Attempted to access nonexistent file descriptor"))
	}
    } else {
	panic!("Attempted to read open FDs on nonexistent process");
    }
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

pub fn start_active_process() -> ! {
    let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

    if let Some(pid) = *running_process {
	process_tbl[pid].init_stack_and_start();
    } else {
	panic!("Attempted to access user address space when no process is running");
    }
}
