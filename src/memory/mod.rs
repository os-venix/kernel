use spin::RwLock;
use x86_64::{PhysAddr, VirtAddr};
use x86_64::structures::paging::{frame::PhysFrame, mapper::MapToError, FrameAllocator, Mapper, Size4KiB, Page, PageTable, PageTableFlags, RecursivePageTable};
use alloc::vec::Vec;

mod frame_allocator;
mod page_allocator;

static KERNEL_PAGE_TABLE: RwLock<Option<RecursivePageTable>> = RwLock::new(None);
static VENIX_FRAME_ALLOCATOR: RwLock<Option<frame_allocator::VenixFrameAllocator>> = RwLock::new(None);
static VENIX_PAGE_ALLOCATOR: RwLock<Option<page_allocator::VenixPageAllocator>> = RwLock::new(None);

#[derive(PartialEq, Eq)]
pub enum MemoryAllocationType {
    RAM,
    MMIO,
}

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
    {
	let mut frame_allocator = VENIX_FRAME_ALLOCATOR.write();
	frame_allocator.as_mut().expect("Attempted to access missing frame allocator").move_to_full_mode();
    }
    unsafe {
	let r = KERNEL_PAGE_TABLE.read();
	let level_4_table = r.as_ref().expect("Attempted to read missing Kernel page table").level_4_table();
	let mut page_allocator = VENIX_PAGE_ALLOCATOR.write();
	page_allocator.as_mut().expect("Attempted to access missing page allocator").move_to_full_mode(level_4_table);
    }
}

pub fn get_usable_ram() -> u64 {
    let r = VENIX_FRAME_ALLOCATOR.read();
    r.as_ref().expect("Attempted to read missing frame allocator").get_usable_memory()
}

pub fn allocate_by_size_kernel(size: u64) -> Result<VirtAddr, MapToError<Size4KiB>> {
    let page_range = {
	let start = {
	    let mut w = VENIX_PAGE_ALLOCATOR.write();
	    w.as_mut().expect("Attempted to read missing Kernel page allocator").get_page_range(size)
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

pub fn allocate_contiguous_region_kernel(size: u64, start_addr: PhysAddr, alloc_type: MemoryAllocationType) -> Result<VirtAddr, MapToError<Size4KiB>> {
    if alloc_type == MemoryAllocationType::RAM {
	// TODO: Check that the requested region is usable RAM and is free
	panic!("Allocate contiguous region of usable physical RAM - not yet implemented!");
    }

    let page_range = {
	let start = {
	    let mut w = VENIX_PAGE_ALLOCATOR.write();
	    w.as_mut().expect("Attempted to read missing Kernel page allocator").get_page_range(size)
	};
	let end = start + (size - 1);

	let start_page = Page::containing_address(start);
	let end_page = Page::containing_address(end);

	Page::range_inclusive(start_page, end_page)
    };

    let frame_range: Vec<PhysFrame> = (0 .. size)
	.step_by(4096)
	.map(|addr| PhysFrame::containing_address(start_addr + addr))
	.collect();

    for (page, &frame) in page_range.zip(frame_range.iter()) {
	let mut frame_allocator = VENIX_FRAME_ALLOCATOR.write();
	let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
	unsafe {
	    let mut mapper = KERNEL_PAGE_TABLE.write();
	    mapper.as_mut().expect("Attempted to use missing kernel page table")
		.map_to(page, frame, flags, frame_allocator.as_mut().expect("Attempted to use missing frame allocator"))?.flush();
	};
    }

    Ok(page_range.start.start_address())
}
