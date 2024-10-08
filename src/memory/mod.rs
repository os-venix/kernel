use spin::RwLock;
use x86_64::VirtAddr;
use x86_64::structures::paging::{mapper::MapToError, FrameAllocator, Mapper, Size4KiB, Page, PageTable, PageTableFlags, RecursivePageTable};

mod frame_allocator;
mod page_allocator;

static KERNEL_PAGE_TABLE: RwLock<Option<RecursivePageTable>> = RwLock::new(None);
static VENIX_FRAME_ALLOCATOR: RwLock<Option<frame_allocator::VenixFrameAllocator>> = RwLock::new(None);
static VENIX_PAGE_ALLOCATOR: RwLock<Option<page_allocator::VenixPageAllocator>> = RwLock::new(None);

pub fn init(recursive_index: bootloader_api::info::Optional<u16>, memory_map: &'static bootloader_api::info::MemoryRegions) {
    let idx = recursive_index.into_option().expect("No recursive page table index found.");

    let idx_64 = idx as u64;
    let sign_extended = ((idx_64 << (64 - 9)) as i64) >> (64 - 9);
    let pt4_addr = ((sign_extended as u64) << 39) | (idx_64 << 30) | (idx_64 << 21) | (idx_64 << 12);

    let pt4_ptr = pt4_addr as *mut PageTable;

    {
	let mut w = KERNEL_PAGE_TABLE.write();
	*w = Some(unsafe {
	    let pt4 = &mut *pt4_ptr;
	    RecursivePageTable::new(pt4).unwrap()
	})
    }

    {
	let mut w = VENIX_FRAME_ALLOCATOR.write();
	*w = Some(unsafe {
	    frame_allocator::VenixFrameAllocator::new(&memory_map)
	})
    }

    {
	let mut w = VENIX_PAGE_ALLOCATOR.write();
	let r = KERNEL_PAGE_TABLE.read();
	let level_4_table = r.as_ref().expect("Attempted to read missing Kernel page table").level_4_table().clone();
	*w = Some(page_allocator::VenixPageAllocator::new(level_4_table));
    }
}

pub fn init_full_mode() {
    let mut frame_allocator = VENIX_FRAME_ALLOCATOR.write();
    frame_allocator.as_mut().expect("Attempted to access missing frame allocator").move_to_full_mode();
}

pub fn allocate_by_size_kernel(size: u64) -> Result<VirtAddr, MapToError<Size4KiB>> {
    let page_range = {
	let start = {
	    let r = VENIX_PAGE_ALLOCATOR.read();
	    r.as_ref().expect("Attempted to read missing Kernel page allocator").get_page_range(size)
	};
	let end = start + (size - 1);

	let start_page = Page::containing_address(start);
	let end_page = Page::containing_address(end);

	Page::range_inclusive(start_page, end_page)
    };

    for page in page_range {
	let mut frame_allocator = VENIX_FRAME_ALLOCATOR.write();
	let frame = frame_allocator.as_mut().expect("Attempted to use missing frame allocator").allocate_frame()
	    .ok_or(MapToError::FrameAllocationFailed)?;

	let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
	unsafe {
	    let mut mapper = KERNEL_PAGE_TABLE.write();
	    
	    mapper.as_mut().expect("Attempted to use missing kernel page table")
		.map_to(page, frame, flags, frame_allocator.as_mut().expect("Attempted to use missing frame allocator"))?.flush()
	};
    }

    Ok(page_range.start.start_address())
}
