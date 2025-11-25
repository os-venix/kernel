use x86_64::{PhysAddr, VirtAddr};
use x86_64::structures::paging::{
    frame::PhysFrame,
    page_table::PageTableEntry,
    page::{
	Page,
	PageRangeInclusive,
    },
    mapper::CleanUp,
    OffsetPageTable,
    PageTable,
    PageTableFlags,
    Size4KiB,
    Mapper,
    FrameDeallocator,
};
use x86_64::registers::control::{Cr3, Cr3Flags};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use anyhow::{anyhow, Result};
use alloc::slice;
use core::cmp::Ordering;

use crate::memory;

#[derive(Debug, PartialEq, Eq, Clone)]
struct MemoryRegion {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug)]
struct PageVirtPhys {
    pub virt_start: VirtAddr,
    pub phys_start: PhysAddr,
}

#[derive(PartialEq, Eq, Debug)]
pub struct AddressSpace {
    pt4: PhysFrame,
    free_regions: Vec<MemoryRegion>,
    pub mapped_regions: BTreeMap<VirtAddr, PhysAddr>,
}

impl AddressSpace {
    pub fn new() -> Self {
	let (virt, phys) = memory::kernel_allocate(
	    4096, memory::MemoryAllocationType::Ram).expect("Allocation failed");
	
	let data_to_z = unsafe {
	    slice::from_raw_parts_mut(virt.as_mut_ptr::<u8>(), 4096_usize)
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
		end: p4_size * 255,  // Anywhere in the lower half
            }]),
	    mapped_regions: BTreeMap::new(),
	}
    }

    pub fn create_copy_of_address_space(&mut self, other: &Self) {
	unsafe fn inner(level: u8,
		 offset_above: u64,
		 page_table: *const PageTable) -> Vec<PageVirtPhys> {
	    let p4_size = 1 << 39;
	    let p3_size = 1 << 30;
	    let p2_size = 1 << 21;
	    let p1_size = 1 << 12;

	    if level != 1 {
		let next_level = match level {
		    4 => 3,
		    3 => 2,
		    2 => 1,
		    _ => panic!("Invalid page level when copying userspace"),
		};
		let idx_size = match level {
		    4 => p4_size,
		    3 => p3_size,
		    2 => p2_size,
		    _ => panic!("Invalid page level when copying userspace"),
		};
		(*page_table).iter()
		    .enumerate()
		    .filter(|(_, entry)| entry.flags().contains(PageTableFlags::PRESENT))
		    .filter(|(index, _)| level != 4 || *index < 256_usize)  // Make sure all user allocs are in the LH
		    .flat_map(|(idx, entry)| inner(
			next_level, offset_above + (idx as u64 * idx_size),
			(entry.addr().as_u64() + memory::DIRECT_MAP_OFFSET.get().unwrap()) as *const PageTable))
		    .collect()
	    } else {
		
		(*page_table).iter()
		    .enumerate()
		    .filter(|(_, entry)| entry.flags().contains(PageTableFlags::PRESENT))
		    .map(|(index, entry)| PageVirtPhys {
			virt_start: VirtAddr::new(offset_above + (index as u64 * p1_size)),
			phys_start: entry.addr(),
		    })
		    .collect()
	    }
	}

	let complete_map = unsafe {
	    let pt4_virt = VirtAddr::new(other.pt4.start_address().as_u64() + memory::DIRECT_MAP_OFFSET.get().unwrap());
	    let pt4 = &mut *pt4_virt.as_mut_ptr::<PageTable>();
	    inner(4, 0, pt4)
	};

	for entry in complete_map {
	    memory::user_allocate(
		4096,
		memory::MemoryAccessRestriction::UserByStart(entry.virt_start),
		self).expect("Unable to allocate to copy userspace");
	    
	    let data_to = unsafe {
		slice::from_raw_parts_mut(entry.virt_start.as_mut_ptr::<u8>(), 4096_usize)
	    };
	    let data_from = unsafe {
		slice::from_raw_parts_mut(VirtAddr::new(entry.phys_start.as_u64() + memory::DIRECT_MAP_OFFSET.get().unwrap()).as_mut_ptr::<u8>(), 4096)
	    };
	    
	    data_to.copy_from_slice(data_from);
	}
    }

    pub unsafe fn switch_to(&self) {
	Cr3::write(self.pt4, Cr3Flags::PAGE_LEVEL_CACHE_DISABLE);
    }

    pub fn get_pt4(&self) -> u64 {
	self.pt4.start_address().as_u64()
    }

    fn map_a_region(&mut self, region: MemoryRegion) {
	let mut pa = region.start;
	while pa < region.end {
	    let va = VirtAddr::new(pa);
	    self.mapped_regions.insert(va, PhysAddr::new(0));
	    pa += 4096;
	}
    }

    pub fn assign_virt_phys(&mut self, virt: VirtAddr, phys: PhysAddr) {
	if let Some(entry) = self.mapped_regions.get_mut(&virt) {
	    *entry = phys;
	}
    }

    // Returns the first virtaddr in the range
    pub fn get_page_range(&mut self, size: u64) -> VirtAddr {
	let size_in_pages = size/4096 + if size % 4096 != 0 { 1 } else { 0 };

	for idx in 0 .. self.free_regions.len() {
	    if self.free_regions[idx].start == 0 {
		self.free_regions[idx].start = 4096;
	    }

	    match (self.free_regions[idx].end - self.free_regions[idx].start).cmp(&(size_in_pages*4096)) {
		Ordering::Equal => {
		    let region = self.free_regions.remove(idx);

		    self.map_a_region(region.clone());
		    return VirtAddr::new(region.start);
		},
		Ordering::Greater => {
		    let start = self.free_regions[idx].start;
		    self.free_regions[idx].start += size_in_pages * 4096;

		    self.map_a_region(MemoryRegion {
			start,
			end: start + size_in_pages * 4096,
		    });
		    
		    return VirtAddr::new(start);
		},
		Ordering::Less => (),
	    }
	}
	panic!("OOM");
    }

    pub fn get_page_range_from_start(&mut self, virt_addr: VirtAddr, size: usize) -> Result<()> {
	let addr = virt_addr.as_u64() - (virt_addr.as_u64() % 4096);
	let mut end_addr = virt_addr.as_u64() + (size as u64);
	if ((virt_addr.as_u64()) + (size as u64)) % 4096 != 0 {
	    end_addr += 4096 - ((virt_addr.as_u64() + (size as u64)) % 4096);  // Page align
	}
	let total_size = end_addr - addr;
	let size_in_pages = total_size/4096;

	for idx in 0 .. self.free_regions.len() {
	    if self.free_regions[idx].start == addr && (
		self.free_regions[idx].end - self.free_regions[idx].start == size_in_pages * 4096) {
		// Remove the whole region
		let region = self.free_regions.remove(idx);
		self.map_a_region(region);
		return Ok(());
	    } else if self.free_regions[idx].start < addr && (
		self.free_regions[idx].start < addr + size_in_pages * 4096) && (
		self.free_regions[idx].end == addr + size_in_pages * 4096) {
		// Resize region so that it ends where the alloc starts 
		self.free_regions[idx].end = addr;
		self.map_a_region(MemoryRegion {
		    start: addr,
		    end: addr + size_in_pages * 4096,
		});

		return Ok(());		    
	    } else if self.free_regions[idx].start < addr && (
		self.free_regions[idx].start < addr + size_in_pages * 4096) && (
		self.free_regions[idx].end > addr + size_in_pages * 4096) {
		// Resize the region so that it ends where the alloc starts, and add a new region from the alloc end to old region end
		let old_end = self.free_regions[idx].end;
		self.free_regions[idx].end = addr;
		self.free_regions.push(MemoryRegion {
		    start: addr + size_in_pages * 4096,
		    end: old_end,
		});
		self.map_a_region(MemoryRegion {
		    start: addr,
		    end: addr + size_in_pages * 4096,
		});

		return Ok(());
	    } else if self.free_regions[idx].start == addr && (
		self.free_regions[idx].end > addr + size_in_pages * 4096) {
		// Resize region so that it starts where the alloc ends
		self.free_regions[idx].start = addr + size_in_pages * 4096;
		self.map_a_region(MemoryRegion {
		    start: addr,
		    end: addr + size_in_pages * 4096,
		});
		return Ok(());
	    }
	}

	Err(anyhow!("Block {:x} is already used", addr))
    }

    pub fn clear_user_space(&mut self) {
	let p4_size = 1 << 39;
	let first_user_page = Page::containing_address(VirtAddr::new(0));
	let last_user_page = Page::containing_address(VirtAddr::new((p4_size * 256) - 1));

	let pt4_virt = VirtAddr::new(self.pt4.start_address().as_u64() + memory::DIRECT_MAP_OFFSET.get().unwrap());
	let mut offset_pt = unsafe {
	    let pt4 = &mut *pt4_virt.as_mut_ptr::<PageTable>();
	    OffsetPageTable::new(pt4, VirtAddr::new(*memory::DIRECT_MAP_OFFSET.get().unwrap()))
	};

	let mut frame_allocator = memory::VENIX_FRAME_ALLOCATOR.write();

	for (virt, _) in self.mapped_regions.iter() {
	    let p: Page<Size4KiB> = Page::from_start_address(*virt).expect("Malformed start address");
	    let (frame, flush) = offset_pt.unmap(p).expect("Attempting to unmap page failed");
	    unsafe {
		frame_allocator.as_mut().expect("Attempted to clear userspace before memory initialised").deallocate_frame(frame);
	    }
	    flush.flush();
	}

	let user_page_range = PageRangeInclusive {
	    start: first_user_page,
	    end: last_user_page,
	};

	unsafe {
	    offset_pt.clean_up_addr_range(user_page_range, frame_allocator.as_mut().expect("Attempted to clear userspace before memory initialised"));
	}

	self.free_regions = Vec::from([MemoryRegion {
	    start: 0x100000,
	    end: p4_size * 255,  // Anywhere in the lower half
        }]);
	self.mapped_regions = BTreeMap::new();
    }
}
