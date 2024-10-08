//use alloc::vec::Vec;
use x86_64::{
    VirtAddr,
    structures::paging::{PageTable}};

pub struct VenixPageAllocator {
    runt_mode: bool,

    // Runt mode
    p4_first_completely_free: VirtAddr,

    // Full mode
    // List of ranges here :-)
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
	    runt_mode: true,
	    p4_first_completely_free: VirtAddr::new(first_free_p4_entry),
	}
    }

    // Returns the first virtaddr in the range
    pub fn get_page_range(&self, size: u64) -> VirtAddr {
	let size_in_pages = size/4096;

	if self.runt_mode {
	    if size_in_pages > 1 << 39 {
		panic!("Attempted to allocate more than a p4 entry in runt mode.");
	    }

	    return self.p4_first_completely_free;
	} else {
	    panic!("Full mode not implemented yet.");
	}
    }
}
