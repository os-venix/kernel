use x86_64::VirtAddr;
use x86_64::structures::paging::{
    frame::PhysFrame,
    page_table::PageTableEntry,
    PageTable,
    PageTableFlags,
};
use x86_64::registers::control::{Cr3, Cr3Flags};
use alloc::vec::Vec;
use anyhow::{anyhow, Result};
use alloc::slice;

use crate::memory;

#[derive(Debug, PartialEq, Eq)]
struct MemoryRegion {
    pub start: u64,
    pub end: u64,
}

#[derive(PartialEq, Eq)]
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
	
	let data_to_z = unsafe {
	    slice::from_raw_parts_mut(virt.as_mut_ptr::<u8>(), 4096 as usize)
	};
	data_to_z.fill_with(Default::default);

	let pt4: &mut PageTable = unsafe {
	    &mut *virt.as_mut_ptr::<PageTable>()
	};

	let frame = PhysFrame::from_start_address(phys[0]).expect("Allocation failed");
	let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

	let mut page_table_entry = PageTableEntry::new();
	page_table_entry.set_frame(frame, flags);

	let r = memory::KERNEL_PAGE_TABLE.read();
	let p4 = r.as_ref().expect("Unable to read kernel page table");

	// Map the kernel
	for i in 256 .. 512 {
	    let level_4_table = p4.level_4_table();
	    pt4[i as usize] = level_4_table[i as usize].clone();
	}

	let p4_size: u64 = 1 << 39;
	AddressSpace {
	    pt4: frame,
	    free_regions: Vec::from([MemoryRegion {
		start: 0x100000,
		end: (p4_size as u64) * 255,  // Anywhere in the lower half
            }]),
	}
    }

    pub unsafe fn switch_to(&self) {
	Cr3::write(self.pt4, Cr3Flags::PAGE_LEVEL_CACHE_DISABLE);
    }

    pub fn get_pt4(&self) -> u64 {
	self.pt4.start_address().as_u64()
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

    pub fn get_page_range_from_start(&mut self, virt_addr: VirtAddr, size: usize) -> Result<()> {
	let addr = virt_addr.as_u64() - (virt_addr.as_u64() % 4096);
	let end_addr = virt_addr.as_u64() + (size as u64) + (4096 - ((virt_addr.as_u64() + (size as u64)) % 4096));  // Page align
	let total_size = end_addr - addr;
	let size_in_pages = total_size/4096;

	for idx in 0 .. self.free_regions.len() {
	    if self.free_regions[idx].start == addr && (
		self.free_regions[idx].end - self.free_regions[idx].start == (size_in_pages as u64) * 4096) {
		// Remove the whole region
		self.free_regions.remove(idx);
		return Ok(());
	    } else if self.free_regions[idx].start < addr && (
		self.free_regions[idx].start < addr + (size_in_pages as u64) * 4096) && (
		self.free_regions[idx].end == addr + (size_in_pages as u64) * 4096) {
		// Resize region so that it ends where the alloc starts 
		self.free_regions[idx].end = addr;

		return Ok(());		    
	    } else if self.free_regions[idx].start < addr && (
		self.free_regions[idx].start < addr + (size_in_pages as u64) * 4096) && (
		self.free_regions[idx].end > addr + (size_in_pages as u64) * 4096) {
		// Resize the region so that it ends where the alloc starts, and add a new region from the alloc end to old region end
		let old_end = self.free_regions[idx].end;
		self.free_regions[idx].end = addr;
		self.free_regions.push(MemoryRegion {
		    start: addr + (size_in_pages as u64) * 4096,
		    end: old_end,
		});

		return Ok(());
	    } else if self.free_regions[idx].start == addr && (
		self.free_regions[idx].end > addr + (size_in_pages as u64) * 4096) {
		// Resize region so that it starts where the alloc ends
		self.free_regions[idx].start = addr + (size_in_pages as u64) * 4096;
		return Ok(());
	    }
	}

	Err(anyhow!("Block {:x} is already used", addr))
    }
}
