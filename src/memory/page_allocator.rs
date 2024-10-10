//use alloc::vec::Vec;
use x86_64::{
    VirtAddr,
    structures::paging::{Page, PageTable, PageTableIndex, page::Size4KiB, page_table::PageTableFlags}};
use alloc::vec::Vec;
use bootloader_api::info::{MemoryRegion, MemoryRegionKind};

pub struct VenixPageAllocator {
    // Runt mode
    p4_first_completely_free: VirtAddr,

    // Full mode
    free_regions: Option<Vec<MemoryRegion>>,
}

impl VenixPageAllocator {
    pub fn new(p4: PageTable) -> Self {
	let p4_size = 1 << 39;
	let first_free_p4_entry = p4.iter().enumerate()
	    .map(|(i, p)| (i * p4_size, p))
	    .filter(|(_, p)| p.is_unused())
	    .map(|(i, _)| i)
	    .next()
	    .expect("Could not find an appropriate p4 entry to initialise") as u64;

	VenixPageAllocator {
	    p4_first_completely_free: VirtAddr::new(first_free_p4_entry),
	    free_regions: None,
	}
    }

    unsafe fn gather_unused_regions_from_page(
	level: u8,
	offset_above: u64,
	indices: (PageTableIndex, PageTableIndex, PageTableIndex, PageTableIndex)) -> Vec<MemoryRegion> {
	let p4_size = 1 << 39;
	let p3_size = 1 << 30;
	let p2_size = 1 << 21;
	let p1_size = 1 << 12;

	let page_table: *const PageTable = Page::from_page_table_indices(
	    indices.0, indices.1, indices.2, indices.3)
	    .start_address()
	    .as_ptr();

	let mut entries: Vec<MemoryRegion> = {
	    let entries: Vec<MemoryRegion> = (*page_table).iter()
		.enumerate()
		.filter(|(_, entry)| !(entry.flags().contains(PageTableFlags::PRESENT)))
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
		    kind: MemoryRegionKind::Usable,
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
			kind: MemoryRegionKind::Usable,
		    });
		} else if entry.end != entries[idx + 1].start {
		    compacted_entries.push(MemoryRegion {
			start: current_start,
			end: entry.end,
			kind: MemoryRegionKind::Usable,
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
				.contains(PageTableFlags::PRESENT) && PageTableIndex::new(*idx as u16) != indices.0) // indices.0 will always be the recursive index
			.flat_map(|(idx, _)| Self::gather_unused_regions_from_page(
			    3, idx as u64 * p4_size, (indices.1, indices.2, indices.3, PageTableIndex::new(idx as u16))))),
		3 => uncompacted_entries.extend(
		    (*page_table).iter()
			.enumerate()
			.filter(|(_, entry)| entry.flags().contains(PageTableFlags::PRESENT))
			.flat_map(|(idx, _)| Self::gather_unused_regions_from_page(
			    2, offset_above + (idx as u64 * p3_size), (indices.1, indices.2, indices.3, PageTableIndex::new(idx as u16))))),
		2 => uncompacted_entries.extend(
		    (*page_table).iter()
			.enumerate()
			.filter(|(_, entry)| entry.flags().contains(PageTableFlags::PRESENT))
			.flat_map(|(idx, _)| Self::gather_unused_regions_from_page(
			    1, offset_above + (idx as u64 * p2_size), (indices.1, indices.2, indices.3, PageTableIndex::new(idx as u16))))),
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
			kind: MemoryRegionKind::Usable,
		    });
		} else if entry.end != uncompacted_entries[idx].start {
		    compacted_entries.push(MemoryRegion {
			start: current_start,
			end: entry.end,
			kind: MemoryRegionKind::Usable,
		    });
		}
	    }

	    entries.extend(compacted_entries)
	}

	entries
	
    }

    pub unsafe fn move_to_full_mode(&mut self, p4: &PageTable) {
	let recursive_index = {
	    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(p4 as *const _ as u64));
	    page.p4_index()
	};

	self.free_regions = Some(Self::gather_unused_regions_from_page(4, 0, (recursive_index, recursive_index, recursive_index, recursive_index)));
    }

    // Returns the first virtaddr in the range
    pub fn get_page_range(&mut self, size: u64) -> VirtAddr {
	let size_in_pages = size/4096;

	if let Some(ref mut free_regions) = self.free_regions {
	    for idx in 0 .. free_regions.len() {
		if free_regions[idx].end - free_regions[idx].start == size {
		    let region = free_regions.remove(idx);
		    return VirtAddr::new(region.start);
		} else if free_regions[idx].end - free_regions[idx].start > size {
		    let start = free_regions[idx].start;
		    free_regions[idx].start += start;
		    return VirtAddr::new(start);
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
