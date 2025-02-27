use alloc::vec::Vec;
use limine::memory_map::{Entry, EntryType};

use x86_64::{
    PhysAddr,
    structures::paging::{PhysFrame, FrameAllocator, FrameDeallocator, Size4KiB}};

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
    free_regions: Option<Vec<MemoryRegion>>,
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
	let mut free_regions: Vec<MemoryRegion> = Vec::new();

	for region in self.memory_map.iter() {
	    if region.entry_type != EntryType::USABLE {
		continue;
	    }
	    
	    if self.next == 0 {
		free_regions.push(MemoryRegion {
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

	    free_regions.push(MemoryRegion {
		start: region.base + (self.next as u64 * 4096),
		end: region.base + region.length - (self.next as u64 * 4096),
	    });

	    self.next = 0;
	}

	self.free_regions = Some(free_regions);
    }
}

unsafe impl FrameAllocator<Size4KiB> for VenixFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
	if let Some(ref mut free_regions) = self.free_regions {
	    if free_regions[0].end - free_regions[0].start == 4096 {
		let region = free_regions.remove(0);
		return Some(PhysFrame::containing_address(PhysAddr::new(region.start)));
	    }

	    let start_addr = free_regions[0].start;
	    free_regions[0].start += 4096;

	    Some(PhysFrame::containing_address(PhysAddr::new(start_addr)))
	} else {
	    let frame = self.usable_frames().nth(self.next);
	    self.next += 1;

	    frame
	}
    }
}

impl FrameDeallocator<Size4KiB> for VenixFrameAllocator {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {
	if let Some(ref mut free_regions) = self.free_regions {
	    free_regions.push(MemoryRegion {
		start: frame.start_address().as_u64(),
		end: frame.start_address().as_u64() + 4096,
	    });
	} else {
	    panic!("Attempted to deallocate while in runt mode");
	}
    }
}
