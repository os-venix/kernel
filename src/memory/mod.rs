use spin::RwLock;
use x86_64::{PhysAddr, VirtAddr};
use x86_64::structures::paging::{
    frame::PhysFrame,
    mapper::MapToError,
    FrameAllocator,
    Mapper,
    Size4KiB,
    Page,
    PageTable,
    PageTableFlags,
    RecursivePageTable,
    PageTableIndex,
};
use x86_64::registers::control::{Cr4, Cr4Flags, Cr3, Cr3Flags};
use alloc::vec::Vec;

use crate::scheduler;

mod frame_allocator;
mod page_allocator;
pub mod user_address_space;

static KERNEL_PAGE_TABLE_IDX: RwLock<u64> = RwLock::new(0);
static KERNEL_PAGE_TABLE: RwLock<Option<RecursivePageTable>> = RwLock::new(None);
static KERNEL_PAGE_FRAME: RwLock<Option<PhysFrame>> = RwLock::new(None);
static VENIX_FRAME_ALLOCATOR: RwLock<Option<frame_allocator::VenixFrameAllocator>> = RwLock::new(None);
static VENIX_PAGE_ALLOCATOR: RwLock<Option<page_allocator::VenixPageAllocator>> = RwLock::new(None);

#[derive(PartialEq, Eq)]
pub enum MemoryAllocationType {
    RAM,
    MMIO,
    DMA,
}

#[derive(PartialEq, Eq)]
pub enum MemoryAllocationOptions {
    Arbitrary,
    Contiguous,
    ContiguousByStart(PhysAddr),
}

#[derive(PartialEq, Eq)]
pub enum MemoryAccessRestriction {
    Kernel,
    User,
}

unsafe fn recursively_make_pages_global(
    level: u8,
    indices: (PageTableIndex, PageTableIndex, PageTableIndex, PageTableIndex)) {
    let page_table: *mut PageTable = Page::from_page_table_indices(
	indices.0, indices.1, indices.2, indices.3)
	.start_address()
	.as_mut_ptr();

    (*page_table).iter_mut()
	.enumerate()
	.filter(|(_, entry)| entry.flags().contains(PageTableFlags::PRESENT))  // Only mark entries that are actually in use
	.filter(|(_, entry)| level == 1 || entry.flags().contains(PageTableFlags::WRITABLE))  // If the parent table isn't writable, we can't (and probably don't want to) update
	.for_each(|(index, entry)| match level {
	    4 => recursively_make_pages_global(3, (indices.1, indices.2, indices.3, PageTableIndex::new(index as u16))),
	    3 => recursively_make_pages_global(2, (indices.1, indices.2, indices.3, PageTableIndex::new(index as u16))),
	    2 => entry.set_flags(entry.flags() | PageTableFlags::GLOBAL),
	    _ => panic!("Invalid page level while marking kernel table as global")
	});
}

fn make_all_pages_global(recursive_index: PageTableIndex) {
    unsafe {
	recursively_make_pages_global(4, (recursive_index, recursive_index, recursive_index, recursive_index));
	Cr4::update(|flags| *flags |= Cr4Flags::PAGE_GLOBAL);
    }
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
	let mut w = KERNEL_PAGE_FRAME.write();
	*w = Some(Cr3::read().0)
    }

    make_all_pages_global(PageTableIndex::new(idx));

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

    {
	let mut w = KERNEL_PAGE_TABLE_IDX.write();
	*w = idx_64;
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

pub fn kernel_allocate_early(size: u64) -> Result<VirtAddr, MapToError<Size4KiB>> {
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

	let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::GLOBAL;
	unsafe {
	    let mut mapper = KERNEL_PAGE_TABLE.write();
	    
	    mapper.as_mut().expect("Attempted to use missing kernel page table")
		.map_to(page, frame, flags, frame_allocator.as_mut().expect("Attempted to use missing frame allocator"))?.flush()
	};
    }

    Ok(page_range.start.start_address())
}

pub fn kernel_allocate(
    size: u64,
    alloc_type: MemoryAllocationType,
    alloc_options: MemoryAllocationOptions,
    access_restriction: MemoryAccessRestriction) -> Result<(VirtAddr, Vec<PhysAddr>), MapToError<Size4KiB>> {
    let maybe_pid = if access_restriction == MemoryAccessRestriction::Kernel {
	unsafe {
	    switch_to_kernel()
	}
    } else { None };

    let page_range = {
	let start = match access_restriction {
	    MemoryAccessRestriction::Kernel => {
		let mut w = VENIX_PAGE_ALLOCATOR.write();
		w.as_mut().expect("Attempted to read missing Kernel page allocator").get_page_range(size)
	    },
	    MemoryAccessRestriction::User => {
		let mut process_tbl = scheduler::PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
		let running_process = scheduler::RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

		match *running_process {
		    Some(pid) => process_tbl[pid].address_space.get_page_range(size),
		    None => {
			panic!("Attempted to allocate userspace memory while not in a process address space");
		    },
		}
	    },
	};
	let end = start + (size - 1);

	let start_page = Page::containing_address(start);
	let end_page = Page::containing_address(end);

	Page::range_inclusive(start_page, end_page)
    };

    let frame_range: Vec<PhysFrame> = match alloc_options {
	MemoryAllocationOptions::Arbitrary => {
	    let mut range = Vec::new();	    
	    let mut frame_allocator = VENIX_FRAME_ALLOCATOR.write();

	    for _ in page_range {
		let frame = frame_allocator.as_mut().expect("Attempted to use missing frame allocator").allocate_frame()
		    .ok_or(MapToError::FrameAllocationFailed)?;
		range.push(frame);
	    }

	    range
	},
	MemoryAllocationOptions::Contiguous => {
	    unimplemented!();
	},
	MemoryAllocationOptions::ContiguousByStart(start_addr) => {
	    (0 .. size)
		.step_by(4096)
		.map(|addr| PhysFrame::containing_address(start_addr + addr))
		.collect()
	},
    };

    for (page, &frame) in page_range.zip(frame_range.iter()) {
	let mut frame_allocator = VENIX_FRAME_ALLOCATOR.write();
	let flags = match access_restriction {
	    MemoryAccessRestriction::Kernel => PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::GLOBAL,
	    MemoryAccessRestriction::User => PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
	};

	unsafe {
	    let mut mapper = KERNEL_PAGE_TABLE.write();
	    mapper.as_mut().expect("Attempted to use missing kernel page table")
		.map_to(page, frame, flags, frame_allocator.as_mut().expect("Attempted to use missing frame allocator"))?.flush();
	};
    }

    if let Some(pid) = maybe_pid {
	scheduler::switch_to(pid);
    }

    Ok((page_range.start.start_address(), frame_range.iter().map(|frame| frame.start_address()).collect()))
}

pub fn allocate_arbitrary_contiguous_region_kernel(
    phys_addr: usize, size: usize, alloc_type: MemoryAllocationType) -> Result<(VirtAddr, usize), MapToError<Size4KiB>> {
    let start_phys_addr = phys_addr - (phys_addr % 4096);  // Page align
    let total_size = size + (phys_addr % 4096) + (4096 - (size % 4096));  // Total amount, aligned to page boundaries

    let allocated_region = kernel_allocate(
	total_size as u64,
	alloc_type,
	MemoryAllocationOptions::ContiguousByStart(PhysAddr::new(start_phys_addr as u64)),
	MemoryAccessRestriction::Kernel,
    )?.0;

    let offset_from_start = phys_addr - start_phys_addr;
    let virt_addr = allocated_region + offset_from_start as u64;

    Ok((virt_addr, total_size as usize))
}

pub unsafe fn switch_to_kernel() -> Option<usize> {
    let r = KERNEL_PAGE_FRAME.read();
    let frame = r.as_ref().expect("Attempted to read missing Kernel page frame");
    Cr3::write(*frame, Cr3Flags::empty());

    scheduler::deschedule()
}
