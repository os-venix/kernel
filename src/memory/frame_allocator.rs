use alloc::collections::BTreeMap;
use limine::memory_map::{Entry, EntryType};
use core::cmp::Ordering;

use x86_64::{
    PhysAddr,
    structures::paging::{PhysFrame, FrameAllocator, FrameDeallocator, Size4KiB}};

#[derive(Clone, Copy)]
struct MemoryRegion {
    pub start: u64,
    pub end: u64,
}

// Theory of operation:
// Initially, VenixFrameAllocator will start in "runt mode". This, in effect, means that it will behave like a bump allocator, allocating the next frame
// sequentially one at a time, with no ability to free frames.
//
// Once the system heap is up, which requires both a frame allocator and a page allocator, VenixFrameAllocator can be moved into "full mode", after which
// it will create a vector of from-tos. The first element in the vector entry is the starting frame number, inclusive. The second elementis the ending
// frame number, non-inclusive.
pub struct VenixFrameAllocator {
    memory_map: &'static [&'static Entry],

    // Runt mode
    next: usize,

    // Full mode
    free_regions: Option<BTreeMap<u64, MemoryRegion>>,
}

impl VenixFrameAllocator {
    pub unsafe fn new(memory_map: &'static [&'static Entry]) -> Self {
	VenixFrameAllocator {
	    memory_map,
	    next: 0,
	    free_regions: None
	}
    }

    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> {
	self.memory_map.iter()
	    .filter(|r| r.entry_type == EntryType::USABLE)
	    .map(|r| r.base .. r.base + r.length)
	    .flat_map(|r| r.step_by(4096))
	    .map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }

    // Used for reporting, how much RAM is present in the system?
    pub fn get_usable_memory(&self) -> u64 {
	self.memory_map.iter()
	    .filter(|r| r.entry_type == EntryType::USABLE)
	    .map(|r| r.length)
	    .sum()
    }

    pub fn move_to_full_mode(&mut self) {
	let mut free_regions: BTreeMap<u64, MemoryRegion> = BTreeMap::new();

	for region in self.memory_map.iter() {
	    if region.entry_type != EntryType::USABLE {
		continue;
	    }
	    
	    if self.next == 0 {
		free_regions.insert(region.base, MemoryRegion {
		    start: region.base,
		    end: region.base + region.length,
		});
		continue;
	    }

	    let size = region.length;
	    let size_in_pages = size / 4096;

	    if size_in_pages as usize <= self.next {
		self.next -= size_in_pages as usize;
		continue;
	    }

	    let region_start = region.base + (self.next as u64 * 4096);

	    free_regions.insert(
		region_start,
		MemoryRegion {
		    start: region_start,
		    end: region.base + region.length,
		});

	    self.next = 0;
	}

	self.free_regions = Some(free_regions);
    }

    // Helper function to check if two regions are adjacent and should be merged    
    fn try_merge_regions(free_regions: &mut BTreeMap<u64, MemoryRegion>, new_region: MemoryRegion) {
	let mut merged = false;

        // Find the region that starts right after or before the new region
        if let Some((&start, &region)) = free_regions.range(..new_region.start).next_back() {
            // Check if the new region is adjacent to the previous one (merge backwards)
            if region.end == new_region.start {
		merged = true;
                free_regions.remove(&start); // Remove the previous region
                let merged = MemoryRegion {
                    start: region.start,
                    end: new_region.end,
                };
                free_regions.insert(merged.start, merged);
            }
        }

        if let Some((&start, &region)) = free_regions.range(new_region.end..).next() {
            // Check if the new region is adjacent to the next one (merge forwards)
            if region.start == new_region.end {
		merged = true;
                free_regions.remove(&start); // Remove the next region
                let merged = MemoryRegion {
                    start: new_region.start,
                    end: region.end,
                };
                free_regions.insert(merged.start, merged);
            }
        }

	if !merged {
            // If no merge happens, just insert the region
            free_regions.insert(new_region.start, new_region);
	}
    }
}

unsafe impl FrameAllocator<Size4KiB> for VenixFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {	
        if let Some(ref mut free_regions) = self.free_regions {
            // Check if we have any free regions
            if let Some(first_region_entry) = free_regions.first_entry() {
                let start_addr = first_region_entry.get().start;
		let end_addr = first_region_entry.get().end;

                // If the first region is exactly one frame (4KiB), just allocate it and remove it
                if end_addr - start_addr == 4096 {
                    // Allocate the first frame and remove the region
                    free_regions.remove(&start_addr); // Remove the region from the map
                    return Some(PhysFrame::containing_address(PhysAddr::new(start_addr)));
                }

                // Otherwise, allocate a frame from the first region and update the region
                let new_start = start_addr + 4096; // Move the start address forward by 4KiB

                // Modify the region in the map (update the start address)
		free_regions.remove(&start_addr);
                free_regions.insert(new_start, MemoryRegion {
                    start: new_start,
                    end: end_addr,
                });

                // Return the allocated frame
                return Some(PhysFrame::containing_address(PhysAddr::new(start_addr)));
            }

	    // We're out of memory
	    return None;
	} else {
	    let frame = self.usable_frames().nth(self.next);
	    self.next += 1;

	    frame
	}
    }
}

impl FrameDeallocator<Size4KiB> for VenixFrameAllocator {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {	
	for region in self.memory_map.iter() {
	    // Check if the frame's start address is within the region bounds
	    let frame_end = frame.start_address().as_u64() + 4096; // Frame's end address

	    if frame.start_address().as_u64() < region.base || frame_end > region.base + region.length {
		continue; // Skip regions that don't contain the frame
	    }

	    // If we attempt to free MMIO, we've gone sideways
	    if region.entry_type != EntryType::USABLE {
		panic!("Attempted to free memory that is not RAM");
	    }
	}

	if let Some(ref mut free_regions) = self.free_regions {
	    // Create the new memory region to be added
            let new_region = MemoryRegion {
                start: frame.start_address().as_u64(),
                end: frame.start_address().as_u64() + 4096,
            };

            // Try merging the new region with the existing free regions
            Self::try_merge_regions(free_regions, new_region);
	} else {
	    panic!("Attempted to deallocate while in runt mode");
	}
    }
}
