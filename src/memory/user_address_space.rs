use x86_64::VirtAddr;
use x86_64::structures::paging::{
    frame::PhysFrame,
    page_table::PageTableEntry,
    PageTable,
    PageTableFlags,
};
use x86_64::registers::control::{Cr3, Cr3Flags};
use alloc::vec::Vec;
use bootloader_api::info::{MemoryRegion, MemoryRegionKind};

use crate::memory;

pub struct AddressSpace {
    pt4: PhysFrame,
    free_regions: Vec<MemoryRegion>,
}

impl AddressSpace {
    pub fn new() -> Self {
	unsafe {
	    memory::switch_to_kernel();
	}

	let (virt, phys) = memory::kernel_allocate(
	    4096, memory::MemoryAllocationType::RAM,
	    memory::MemoryAllocationOptions::Arbitrary, memory::MemoryAccessRestriction::Kernel).expect("Allocation failed");
	let pt4: &mut PageTable = unsafe {
	    &mut *virt.as_mut_ptr::<PageTable>()
	};

	let frame = PhysFrame::from_start_address(phys[0]).expect("Allocation failed");
	let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

	let mut page_table_entry = PageTableEntry::new();
	page_table_entry.set_frame(frame, flags);

	let idx_64: u64 = {
	    *memory::KERNEL_PAGE_TABLE_IDX.read()
	};

	pt4[idx_64 as usize] = page_table_entry.clone();

	// Map the kernel
	for i in 256 .. 512 {
	    if i == idx_64 {
		continue;
	    }

	    let r = memory::KERNEL_PAGE_TABLE.read();
	    let p4 = r.as_ref().expect("Unable to read kernel page table");

	    let level_4_table = p4.level_4_table();
	    pt4[i as usize] = level_4_table[i as usize].clone();
	}

	for i in 0 .. 512 {
	    let r = memory::KERNEL_PAGE_TABLE.read();
	    let p4 = r.as_ref().expect("Unable to read kernel page table");
	    let level_4_table = p4.level_4_table();

	    if level_4_table[i as usize].flags().contains(PageTableFlags::PRESENT) {
		log::info!("{} {:#?}", i, level_4_table[i as usize]);
	    }
	}
	    

	let p4_size: u64 = 1 << 39;
	AddressSpace {
	    pt4: frame,
	    free_regions: Vec::from([MemoryRegion {
		start: 0x100000,
		end: (p4_size as u64) * 255,  // Anywhere in the lower half
		kind: MemoryRegionKind::Usable,
	    }]),
	}
    }

    pub unsafe fn switch_to(&self) {
	log::info!("{:#?}", self.pt4);
	
	let r = memory::KERNEL_PAGE_FRAME.read();
	let frame = r.as_ref().expect("Attempted to read missing Kernel page frame");
	log::info!("{:#?}", *frame);

	Cr3::write(self.pt4, Cr3Flags::PAGE_LEVEL_CACHE_DISABLE);
	log::info!("Test");
    }

    // Returns the first virtaddr in the range
    pub fn get_page_range(&mut self, size: u64) -> VirtAddr {
	let size_in_pages = size/4096 + if size % 4096 != 0 { 1 } else { 0 };

	for idx in 0 .. self.free_regions.len() {
	    if self.free_regions[idx].start == 0 {
		self.free_regions[idx].start = 4096;
	    }
	    if self.free_regions[idx].end - self.free_regions[idx].start == size_in_pages*4096 {
		let region = self.free_regions.remove(idx);

		let sign_extended = ((region.start << 16) as i64) >> 16;
		return VirtAddr::new(sign_extended as u64);
	    } else if self.free_regions[idx].end - self.free_regions[idx].start > size_in_pages * 4096 {
		let start = self.free_regions[idx].start;
		self.free_regions[idx].start += size_in_pages * 4096;

		let sign_extended = ((start << 16) as i64) >> 16;
		return VirtAddr::new(sign_extended as u64);
	    }
	}
	panic!("OOM");
    }
}
