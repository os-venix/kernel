use alloc::string::String;
use anyhow::{anyhow, Result};
use xmas_elf::{header, ElfFile, program::{SegmentData, Type}};
use core::{slice, default::Default};
use x86_64::VirtAddr;

use crate::sys;
use crate::memory;

pub struct Elf {
    pub entry: u64,
    pub base: u64,
    pub program_header: u64,
    pub program_header_entry_size: u64,
    pub program_header_entry_count: u64,
}

impl Elf {
    pub fn new(file_name: String) -> Result<Elf> {
	let (file_contents, file_size) = match sys::vfs::read(file_name.clone()) {
	    Ok(f) => f,
	    Err(_) => {
		return Err(anyhow!("Could not load /init/init"));
	    }
	};

	let file_contents_slice = unsafe {
	    slice::from_raw_parts(file_contents, file_size)
	};

	let elf = match ElfFile::new(file_contents_slice) {
	    Ok(f) => f,
	    Err(e) => {
		return Err(anyhow!("Could not initialise /init/init: {}", e));
	    },
	};

	if elf.header.pt2.entry_point() == 0 {
	    panic!("Not an executable with an entry point");
	}

	let mut lowest_virt_addr: Option<u64> = None;
	let mut highest_virt_addr: Option<u64> = None;
	for program_header in elf.program_iter() {
	    // Not sure what's going on here, but these exist, and should be skipped
	    if program_header.virtual_addr() == 0 && program_header.mem_size() == 0 {
		continue;
	    }

	    if lowest_virt_addr == None {
		lowest_virt_addr = Some(program_header.virtual_addr());
	    } else if program_header.virtual_addr() < lowest_virt_addr.expect("Eh") {
		lowest_virt_addr = Some(program_header.virtual_addr());
	    }

	    if highest_virt_addr == None {
		highest_virt_addr = Some(program_header.virtual_addr() + program_header.mem_size());
	    } else if program_header.virtual_addr() + program_header.mem_size() > highest_virt_addr.expect("Eh") {
		highest_virt_addr = Some(program_header.virtual_addr() + program_header.mem_size());
	    }
	}

	let virt_start_addr = match elf.header.pt2.type_().as_type() {
	    header::Type::Executable => {
		let virt_start_addr = VirtAddr::new(lowest_virt_addr.expect("No loadable sections were found"));

		match memory::kernel_allocate(
		    highest_virt_addr.expect("No loadable sections were found") - lowest_virt_addr.expect("No loadable sections were found"),
		    memory::MemoryAllocationType::RAM,
		    memory::MemoryAllocationOptions::Arbitrary,
		    memory::MemoryAccessRestriction::UserByStart(virt_start_addr)) {
		    Ok(_) => (),
		    Err(e) => {
			return Err(anyhow!("Could not allocate memory for {}: {:?}", file_name, e));
		    }
		}

		virt_start_addr
	    },
	    header::Type::SharedObject => {
		let (start, _) = match memory::kernel_allocate(
		    highest_virt_addr.expect("No loadable sections were found") - lowest_virt_addr.expect("No loadable sections were found"),
		    memory::MemoryAllocationType::RAM,
		    memory::MemoryAllocationOptions::Arbitrary,
		    memory::MemoryAccessRestriction::User) {
		    Ok(i) => i,
		    Err(e) => panic!("Could not allocate memory for {}: {:?}", file_name, e),
		};

		start
	    },
	    _ => unimplemented!(),
	};

	{
	    let size_to_zero = highest_virt_addr.expect("No loadable sections were found") - lowest_virt_addr.expect("No loadable sections were found");

	    let data_to_z = unsafe {
		slice::from_raw_parts_mut(virt_start_addr.as_mut_ptr::<u8>(), size_to_zero as usize)
	    };
	    data_to_z.fill_with(Default::default);
	}

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
		let data_to = unsafe {
		    slice::from_raw_parts_mut(virt_header_start_addr.as_mut_ptr::<u8>(), program_header.file_size() as usize)
		};
		data_to.copy_from_slice(data);
	    }
	}

	let entry = match elf.header.pt2.type_().as_type() {
	    header::Type::Executable => elf.header.pt2.entry_point(),
	    header::Type::SharedObject => virt_start_addr.as_u64() + elf.header.pt2.entry_point(),
	    _ => unimplemented!(),
	};

	Ok(Elf {
	    entry: entry,
	    base: virt_start_addr.as_u64(),
	    program_header: virt_start_addr.as_u64() + elf.header.pt2.ph_offset(),
	    program_header_entry_size: elf.header.pt2.ph_entry_size() as u64,
	    program_header_entry_count: elf.header.pt2.ph_count() as u64,
	})
    }
}
