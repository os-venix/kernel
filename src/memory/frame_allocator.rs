//use alloc::vec::Vec;
use bootloader_api::info::{MemoryRegion, MemoryRegions, MemoryRegionKind};

use x86_64::{
    PhysAddr,
    structures::paging::{PhysFrame, FrameAllocator, Size4KiB}};

// Theory of operation:
// Initially, VenixFrameAllocator will start in "runt mode". This, in effect, means that it will behave like a bump allocator, allocating the next frame
// sequentially one at a time, with no ability to free frames.
//
// Once the system heap is up, which requires both a frame allocator and a page allocator, VenixFrameAllocator can be moved into "full mode", after which
// it will create a vector of from-tos. The first element in the vector entry is the starting frame number, inclusive. The second elementis the ending
// frame number, non-inclusive.
//
// TODO: move from frame indices to page address in full mode vector.
pub struct VenixFrameAllocator {
    memory_map: &'static [MemoryRegion],
    runt_mode: bool,

    // Runt mode
    next: usize,

    // Full mode
//    free_regions: Option<Vec<(usize, usize)>>,
}

impl VenixFrameAllocator {
    pub unsafe fn new(memory_map: &'static MemoryRegions) -> Self {
	VenixFrameAllocator {
	    memory_map,
	    runt_mode: true,
	    next: 0,
//	    free_regions: None
	}
    }

    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> {
	self.memory_map.iter()
	    .filter(|r| r.kind == MemoryRegionKind::Usable)
	    .map(|r| r.start .. r.end)
	    .flat_map(|r| r.step_by(4096))
	    .map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

unsafe impl FrameAllocator<Size4KiB> for VenixFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
	if !self.runt_mode {
	    panic!("Frame allocator full mode not yet implemented!");
	}

	let frame = self.usable_frames().nth(self.next);
	self.next += 1;

	frame
    }
}
