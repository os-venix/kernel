//use alloc::vec::Vec;
use x86_64::{
    VirtAddr,
    structures::paging::{PageTable, page_table::PageTableFlags}};
use alloc::vec::Vec;

#[derive(Debug)]
struct MemoryRegion {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug)]
pub struct VenixPageAllocator {
    hhdm_offset: u64,

    // Runt mode
    p4_first_completely_free: VirtAddr,

    // Full mode
    free_regions: Option<Vec<MemoryRegion>>,
}

impl VenixPageAllocator {
    pub fn new(p4: PageTable, hhdm_offset: u64) -> Self {
	let p4_size = 1 << 39;
	let first_free_p4_entry = p4.iter().enumerate()
	    .filter(|(i, p)| *i >= 256 && p.is_unused())
	    .map(|(i, _)| i * p4_size)
	    .next()
	    .expect("Could not find an appropriate p4 entry to initialise") as u64;

	VenixPageAllocator {
	    hhdm_offset: hhdm_offset,
	    p4_first_completely_free: VirtAddr::new_truncate(first_free_p4_entry),
	    free_regions: None,
	}
    }

    unsafe fn gather_unused_regions_from_page(
	&self,
	level: u8,
	offset_above: u64,
	page_table: *const PageTable) -> Vec<MemoryRegion> {
	let p4_size = 1 << 39;
	let p3_size = 1 << 30;
	let p2_size = 1 << 21;
	let p1_size = 1 << 12;

	let mut entries: Vec<MemoryRegion> = {
	    let entries: Vec<MemoryRegion> = (*page_table).iter()
		.enumerate()
		.filter(|(_, entry)| !(entry.flags().contains(PageTableFlags::PRESENT)))
		.filter(|(index, _)| level != 4 || *index >= 256 as usize)  // Make sure all kernel allocs are in the HH
		.map(|(index, _)| match level {
		    4 => index as u64 * p4_size,
		    3 => offset_above + (index as u64 * p3_size),
		    2 => offset_above + (index as u64 * p2_size),
		    1 => offset_above + (index as u64 * p1_size),
		    _ => panic!("Invalid page level while calculating free virtual space")
		})
		.map(|start| MemoryRegion {
		    start: start,
		    end: match level {
			4 => start + p4_size,
			3 => start + p3_size,
			2 => start + p2_size,
			1 => start + p1_size,
			_ => panic!("Invalid page level while calculating free virtual space")
		    },
		})
		.collect();
	    let mut compacted_entries: Vec<MemoryRegion> = Vec::new();
	    let mut current_start: u64 = 0;
	    for (idx, entry) in entries.iter().enumerate() {
		if idx == 0 {
		    current_start = entry.start;
		} else if entry.start != entries[idx - 1].end {
		    current_start = entry.start;
		}

		if idx == entries.len() - 1 {
		    compacted_entries.push(MemoryRegion {
			start: current_start,
			end: entry.end,
		    });
		} else if entry.end != entries[idx + 1].start {
		    compacted_entries.push(MemoryRegion {
			start: current_start,
			end: entry.end,
		    });
		}
	    }

	    compacted_entries
	};

	{
	    let mut uncompacted_entries: Vec<MemoryRegion> = Vec::new();
	    match level {
		4 => uncompacted_entries.extend(
		    (*page_table).iter()
			.enumerate()
			.filter(|(idx, entry)| entry
				.flags()
				.contains(PageTableFlags::PRESENT) &&
				*idx >= 256)
			.flat_map(|(idx, entry)| self.gather_unused_regions_from_page(
			    3, idx as u64 * p4_size, (entry.addr().as_u64() + self.hhdm_offset) as *const PageTable)
			)),
		3 => uncompacted_entries.extend(
		    (*page_table).iter()
			.enumerate()
			.filter(|(_, entry)| entry.flags().contains(PageTableFlags::PRESENT) &&
				!(entry.flags().contains(PageTableFlags::HUGE_PAGE)))
			.flat_map(|(idx, entry)| self.gather_unused_regions_from_page(
			    2, offset_above + (idx as u64 * p3_size), (entry.addr().as_u64() + self.hhdm_offset) as *const PageTable)
			)),
		2 => uncompacted_entries.extend(
		    (*page_table).iter()
			.enumerate()
			.filter(|(_, entry)| entry.flags().contains(PageTableFlags::PRESENT) &&
				!(entry.flags().contains(PageTableFlags::HUGE_PAGE)))
			.flat_map(|(idx, entry)| self.gather_unused_regions_from_page(
				1, offset_above + (idx as u64 * p2_size), (entry.addr().as_u64() + self.hhdm_offset) as *const PageTable)
			)),
		_ => (),
	    }

	    let mut compacted_entries: Vec<MemoryRegion> = Vec::new();
	    let mut current_start: u64 = 0;
	    for (idx, entry) in uncompacted_entries.iter().enumerate() {
		if idx == 0 {
		    current_start = entry.start;
		} else if entry.start != uncompacted_entries[idx - 1].end {
		    current_start = entry.start;
		}

		if idx == uncompacted_entries.len() - 1 {
		    compacted_entries.push(MemoryRegion {
			start: current_start,
			end: entry.end,
		    });
		} else if entry.end != uncompacted_entries[idx].start {
		    compacted_entries.push(MemoryRegion {
			start: current_start,
			end: entry.end,
		    });
		}
	    }

	    entries.extend(compacted_entries)
	}

	entries	
    }

    pub unsafe fn move_to_full_mode(&mut self, p4: *const PageTable) {
	self.free_regions = Some(self.gather_unused_regions_from_page(4, 0, p4));
    }

    // Returns the first virtaddr in the range
    pub fn get_page_range(&mut self, size: u64) -> VirtAddr {
	let size_in_pages = size/4096 + if size % 4096 != 0 { 1 } else { 0 };

	if let Some(ref mut free_regions) = self.free_regions {
	    for idx in 0 .. free_regions.len() {
		if free_regions[idx].end - free_regions[idx].start == size_in_pages * 4096 {
		    let region = free_regions.remove(idx);
		    return VirtAddr::new_truncate(region.start as u64);
		} else if free_regions[idx].end - free_regions[idx].start > size_in_pages * 4096 {
		    let start = free_regions[idx].start;
		    free_regions[idx].start += size_in_pages * 4096;
		    return VirtAddr::new_truncate(start as u64);
		}
	    }
	    panic!("Kernel OOM");
	} else {
	    if size_in_pages > 1 << 39 {
		panic!("Attempted to allocate more than a p4 entry in runt mode.");
	    }

	    return self.p4_first_completely_free;
	}
    }
}
