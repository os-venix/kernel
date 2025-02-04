use alloc::string::String;
use anyhow::{anyhow, Result};
use xmas_elf::{header, ElfFile, program::{SegmentData, Type}};
use core::{slice, default::Default};
use x86_64::VirtAddr;

use crate::sys;
use crate::memory;

pub struct Elf {
    pub entry: u64,
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

	let virtual_offset: u64 = match elf.header.pt2.type_().as_type() {
	    header::Type::Executable => 0,// Todo,
	    _ => unimplemented!(),
	};

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

	    let virt_start_addr = VirtAddr::new(virtual_offset + program_header.virtual_addr());

	    match memory::kernel_allocate(
		program_header.mem_size(),
		memory::MemoryAllocationType::RAM,
		memory::MemoryAllocationOptions::Arbitrary,
		memory::MemoryAccessRestriction::UserByStart(virt_start_addr)) {
		Ok(_) => (),
		Err(e) => {
		    return Err(anyhow!("Could not allocate memory for {}: {:?}", file_name, e));
		}
	    }

	    let data = match program_header.get_data(&elf) {
		Ok(SegmentData::Undefined(data)) => data,
		Ok(_) => return Err(anyhow!("Coud not parse program header: invalid SegmentData type")),
		Err(e) => return Err(anyhow!("Could not parse program header: {}", e)),
	    };

	    let data_to = unsafe {
		slice::from_raw_parts_mut(virt_start_addr.as_mut_ptr::<u8>(), program_header.mem_size() as usize)
	    };

	    if program_header.mem_size() == program_header.file_size() {
		data_to.copy_from_slice(data);
	    } else if program_header.file_size() == 0 {
		// BSS segment
		data_to.fill_with(Default::default);
	    }
	}

	Ok(Elf {
	    entry: elf.header.pt2.entry_point()
	})
    }
}
