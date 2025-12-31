use alloc::string::String;
use anyhow::{anyhow, Result};
use xmas_elf::{header, ElfFile, program::{SegmentData, Type}};
use x86_64::VirtAddr;
use alloc::vec;

use crate::memory;
use crate::process;
use crate::scheduler;
use crate::vfs;

pub struct Elf {
    pub entry: u64,
    pub base: u64,
    pub program_header: u64,
    pub program_header_entry_size: u64,
    pub program_header_entry_count: u64,
}

impl Elf {
    pub async fn new(file_name: String) -> Result<Elf> {
	log::info!("a");
	let fh = vfs::vfs_open(&file_name).await?;
	log::info!("a");
	let stat = fh.clone().stat()?;

	log::info!("a");
	let file_contents = fh.read(stat.size.unwrap()).await?;

	log::info!("b");
	let elf = match ElfFile::new(&file_contents[..]) {
	    Ok(f) => f,
	    Err(e) => {
		return Err(anyhow!("Could not initialise /init/init: {}", e));
	    },
	};

	log::info!("c");
	if elf.header.pt2.entry_point() == 0 {
	    panic!("Not an executable with an entry point");
	}

	log::info!("a");
	let mut lowest_virt_addr: Option<u64> = None;
	let mut highest_virt_addr: Option<u64> = None;
	for program_header in elf.program_iter() {
	    // Not sure what's going on here, but these exist, and should be skipped
	    if program_header.virtual_addr() == 0 && program_header.mem_size() == 0 {
		continue;
	    }

	    if lowest_virt_addr.is_none() || program_header.virtual_addr() < lowest_virt_addr.expect("Eh") {
		lowest_virt_addr = Some(program_header.virtual_addr());
	    }

	    if highest_virt_addr.is_none() ||
		program_header.virtual_addr() + program_header.mem_size() > highest_virt_addr.expect("Eh") {
		    highest_virt_addr = Some(program_header.virtual_addr() + program_header.mem_size());
		}
	}

	log::info!("a");
	let virt_start_addr = {
	    let process = scheduler::get_current_process();
	    let mut task_type = process.task_type.write();

	    match *task_type {
		process::TaskType::Kernel => unreachable!(),
		process::TaskType::User(ref mut address_space) => {
		    match elf.header.pt2.type_().as_type() {
			header::Type::Executable => {
			    let virt_start_addr = VirtAddr::new(lowest_virt_addr.expect("No loadable sections were found"));

			    match memory::user_allocate(
				highest_virt_addr.expect("No loadable sections were found") - lowest_virt_addr.expect("No loadable sections were found"),
				memory::MemoryAccessRestriction::UserByStart(virt_start_addr),
				address_space) {
				Ok(_) => (),
				Err(e) => {
				    return Err(anyhow!("Could not allocate memory for {}: {:?}", file_name, e));
				}
			    }

			    virt_start_addr
			},
			header::Type::SharedObject => {
			    let (start, _) = match memory::user_allocate(
				highest_virt_addr.expect("No loadable sections were found") - lowest_virt_addr.expect("No loadable sections were found"),
				memory::MemoryAccessRestriction::User,
				address_space) {
				Ok(i) => i,
				Err(e) => panic!("Could not allocate memory for {}: {:?}", file_name, e),
			    };

			    start
			},
			_ => unimplemented!(),
		    }
		},
	    }
	};

	log::info!("a");
	{
	    let size_to_zero = highest_virt_addr.expect("No loadable sections were found") - lowest_virt_addr.expect("No loadable sections were found");
	    let empty_buf = vec![0; size_to_zero as usize];
	    memory::copy_to_user(virt_start_addr, empty_buf.as_slice())?;
	}

	log::info!("a");
	for program_header in elf.program_iter() {
	    // PT_LOAD is a loadable segment that needs to be in the address space.
	    // All else can be skipped.
	    match program_header.get_type() {
		Ok(Type::Load) => (),
		Ok(_) => continue,
		Err(e) => {
		    return Err(anyhow!("Could not parse program header: {}", e));
		},
	    }

	    // Not sure what's going on here, but these exist, and should be skipped
	    if program_header.virtual_addr() == 0 && program_header.mem_size() == 0 {
		continue;
	    }

	    let virt_header_start_addr = match elf.header.pt2.type_().as_type() {
		header::Type::Executable => VirtAddr::new(program_header.virtual_addr()),
		header::Type::SharedObject => virt_start_addr + program_header.virtual_addr(),
		_ => unimplemented!(),
	    };

	    let data = match program_header.get_data(&elf) {
		Ok(SegmentData::Undefined(data)) => data,
		Ok(_) => return Err(anyhow!("Coud not parse program header: invalid SegmentData type")),
		Err(e) => return Err(anyhow!("Could not parse program header: {}", e)),
	    };

	    if program_header.file_size() != 0 {
		memory::copy_to_user(virt_header_start_addr, data)?;
	    }
	}

	log::info!("a");
	let entry = match elf.header.pt2.type_().as_type() {
	    header::Type::Executable => elf.header.pt2.entry_point(),
	    header::Type::SharedObject => virt_start_addr.as_u64() + elf.header.pt2.entry_point(),
	    _ => unimplemented!(),
	};

	log::info!("a");
	Ok(Elf {
	    entry,
	    base: virt_start_addr.as_u64(),
	    program_header: virt_start_addr.as_u64() + elf.header.pt2.ph_offset(),
	    program_header_entry_size: elf.header.pt2.ph_entry_size() as u64,
	    program_header_entry_count: elf.header.pt2.ph_count() as u64,
	})
    }
}
