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

use crate::memory;
use crate::sys::vfs;

mod elf_loader;

const AT_NUL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_BASE: u64 = 7;
const AT_ENTRY: u64 = 9;

#[derive(Clone)]
struct AuxVector {
    auxv_type: u64,
    value: u64,
}

#[derive(Clone)]
pub struct Process {
    pub address_space: Arc<RwLock<memory::user_address_space::AddressSpace>>,
    file_descriptors: BTreeMap<u64, Arc<RwLock<vfs::FileDescriptor>>>,
    next_fd: u64,
    args: Vec<String>,
    envvars: Vec<String>,
    auxvs: Vec<AuxVector>,
    rip: u64,
    rsp: u64,
}

impl Process {
    pub fn new() -> Self {
	let address_space = memory::user_address_space::AddressSpace::new();
	unsafe {
	    address_space.switch_to();
	}

	Process {
	    address_space: Arc::new(RwLock::new(address_space)),
	    file_descriptors: BTreeMap::new(),
	    next_fd: 0,
	    args: vec!(String::from("init")),
	    envvars: vec!(String::from("PATH=/bin:/usr/bin")),
	    auxvs: Vec::new(),
	    rip: 0,
	    rsp: 0,
	}
    }

    pub fn from_existing(existing: &Self, rip: u64) -> Self {
	Process {
	    address_space: existing.address_space.clone(),
	    file_descriptors: existing.file_descriptors.clone(),
	    next_fd: existing.next_fd,
	    args: existing.args.clone(),
	    envvars: existing.envvars.clone(),
	    auxvs: existing.auxvs.clone(),
	    rip: rip,
	    rsp: 0,
	}
    }

    pub fn init_stack(&mut self) {
	let mut address_space = self.address_space.write();
	let (rsp, _) = match memory::kernel_allocate(
	    8 * 1024 * 1024,  // 8MiB
	    memory::MemoryAllocationType::RAM,
	    memory::MemoryAllocationOptions::Arbitrary,
	    memory::MemoryAccessRestriction::UserByAddressSpace(&mut address_space)) {
	    Ok(i) => i,
	    Err(e) => panic!("Could not allocate stack memory for process: {:?}", e),
	};

	self.rsp = rsp.as_u64() + 8 * 1024 * 1024;  // Start at the end of the stack and grow down
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

	self.rip = ld_so.entry;
    }

    pub fn init_stack_and_start(&mut self) -> (u64, u64) {
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

	self.rsp -= /* auxv = */(self.auxvs.len() as u64 * 16) +
	    (self.envvars.len() as u64 * 8) +
	    (self.args.len() as u64 * 8) +
	/* padding = */(3 * 8);
	let stack_ptr = VirtAddr::new(self.rsp);

	let ptrs_to = unsafe {
	    slice::from_raw_parts_mut(
		stack_ptr.as_mut_ptr::<u64>(),
		/* auxv = */(self.auxvs.len() * 2) + self.envvars.len() + self.args.len() + 3,
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
	for (i, auxv) in self.auxvs.iter().enumerate() {
	    ptrs_to[3 + args_p.len() + envvar_p.len() + (i * 2)] = auxv.auxv_type;
	    ptrs_to[4 + args_p.len() + envvar_p.len() + (i * 2)] = auxv.value;
	}

	(self.rsp, self.rip)
    }
}

pub static PROCESS_TABLE: Once<RwLock<BTreeMap<u64, Process>>> = Once::new();
pub static RUNNING_PROCESS: Once<RwLock<Option<u64>>> = Once::new();
pub static NEXT_PID: Once<Mutex<u64>> = Once::new();

pub fn init() {
    PROCESS_TABLE.call_once(|| RwLock::new(BTreeMap::new()));
    RUNNING_PROCESS.call_once(|| RwLock::new(None));
    NEXT_PID.call_once(|| Mutex::new(1));  // Don't use PID 0
}

pub fn start_new_process(parent: u64, filename: String) -> u64 {
    let pid = {
	let mut next_pid = NEXT_PID.get().expect("Attempted to access next PID before it is initialised").lock();
	let pid = *next_pid;
	*next_pid += 1;
	pid
    };

    {
	let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
	process_tbl.insert(pid, Process::new());

	let mut running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").write();
	*running_process = Some(pid);
    };
    let elf = elf_loader::Elf::new(filename).expect("Failed to load ELF");
    let ld = elf_loader::Elf::new(String::from("/lib/ld.so")).expect("Failed to load ld.so");
    {
	let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
	process_tbl.get_mut(&pid).unwrap().attach_loaded_elf(elf, ld);
	process_tbl.get_mut(&pid).unwrap().init_stack();
    }

    pid
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

	let existing_process = process_tbl[&running_process.expect("No running process")].clone();

	process_tbl.insert(pid, Process::from_existing(&existing_process, rip));
	process_tbl.get_mut(&pid).unwrap().init_stack();
    };

    pid
}

pub fn switch_to_process(pid: u64) {
    let mut running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").write();
    *running_process = Some(pid);
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

pub fn deschedule() -> Option<u64> {
    let mut running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").write();
    let current_pid = *running_process;
    *running_process = None;

    current_pid
}

pub fn switch_to(pid: u64) {
    let process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").read();

    if !process_tbl.contains_key(&pid) {
	panic!("Attempted to switch to a nonexistent process");
    }

    unsafe {
	let address_space = process_tbl[&pid].address_space.read();
	address_space.switch_to();
    }

    let mut running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").write();
    *running_process = Some(pid);
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

pub fn is_process_running() -> bool {
    let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();
    running_process.is_some()
}

pub fn start_active_process() -> ! {
    let (rsp, rip) = {
	let mut process_tbl = PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
	let running_process = RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

	if let Some(pid) = *running_process {
	    process_tbl.get_mut(&pid).unwrap().init_stack_and_start()
	} else {
	    panic!("Attempted to access user address space when no process is running");
	}
    };

    unsafe {
	core::arch::asm!(
	    "swapgs",
	    "mov rsp, {stackptr}",
	    "sysretq",

	    in("rcx") rip,
	    stackptr = in(reg) rsp,
	    in("r11") 0x202,
	    options(noreturn),
	);
    }
}
