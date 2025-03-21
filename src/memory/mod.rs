use spin::{Once, RwLock};
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
    OffsetPageTable,
    page::PageRangeInclusive,
};
use x86_64::registers::control::{Cr3, Cr3Flags};
use alloc::vec::Vec;
use limine::memory_map::Entry;
use core::mem::offset_of;

use crate::gdt;
use crate::scheduler;

mod frame_allocator;
mod page_allocator;
pub mod user_address_space;

static KERNEL_PAGE_TABLE: RwLock<Option<OffsetPageTable>> = RwLock::new(None);
static KERNEL_PAGE_FRAME: RwLock<Option<PhysFrame>> = RwLock::new(None);
static VENIX_FRAME_ALLOCATOR: RwLock<Option<frame_allocator::VenixFrameAllocator>> = RwLock::new(None);
static VENIX_PAGE_ALLOCATOR: RwLock<Option<page_allocator::VenixPageAllocator>> = RwLock::new(None);

static DIRECT_MAP_OFFSET: Once<u64> = Once::new();

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct MemoryRegion {
    pub start: u64,
    pub end: u64,
}

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

#[derive(PartialEq, Eq, Debug)]
pub enum MemoryAccessRestriction<'a> {
    EarlyKernel,
    Kernel,
    User,
    UserByStart(VirtAddr),
    UserByAddressSpace(&'a mut user_address_space::AddressSpace),
    UserByAddressSpaceAndStart(&'a mut user_address_space::AddressSpace, VirtAddr),
}

pub fn init(direct_map_offset: u64, memory_map: &'static [&'static Entry]) {
    let pt4_addr = Cr3::read().0.start_address().as_u64() + direct_map_offset;
    let pt4_ptr = pt4_addr as *mut PageTable;

    DIRECT_MAP_OFFSET.call_once(|| direct_map_offset);

    {
	let mut w = KERNEL_PAGE_TABLE.write();
	*w = Some(unsafe {
	    let pt4 = &mut *pt4_ptr;
	    OffsetPageTable::new(pt4, VirtAddr::new(direct_map_offset))
	})
    }

    {
	let mut w = KERNEL_PAGE_FRAME.write();
	*w = Some(Cr3::read().0)
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
	*w = Some(page_allocator::VenixPageAllocator::new(level_4_table, direct_map_offset));
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
    _alloc_type: MemoryAllocationType,
    alloc_options: MemoryAllocationOptions,
    mut access_restriction: MemoryAccessRestriction) -> Result<(VirtAddr, Vec<PhysAddr>), MapToError<Size4KiB>> {
    if access_restriction == MemoryAccessRestriction::Kernel {
	unsafe {
	    switch_to_kernel()
	}
    }

    let page_range = {
	let start = match access_restriction {
	    MemoryAccessRestriction::Kernel | MemoryAccessRestriction::EarlyKernel => {
		let mut w = VENIX_PAGE_ALLOCATOR.write();
		w.as_mut().expect("Attempted to read missing Kernel page allocator").get_page_range(size)
	    },
	    MemoryAccessRestriction::User => {
		let mut process_tbl = scheduler::PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
		let running_process = scheduler::RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

		match *running_process {
		    Some(pid) => {
			let mut address_space = process_tbl.get_mut(&pid).unwrap().address_space.write();
			address_space.get_page_range(size)
		    },
		    None => {
			panic!("Attempted to allocate userspace memory while not in a process address space");
		    },
		}
	    },
	    MemoryAccessRestriction::UserByStart(addr) => {
		let mut process_tbl = scheduler::PROCESS_TABLE.get().expect("Attempted to access process table before it is initialised").write();
		let running_process = scheduler::RUNNING_PROCESS.get().expect("Attempted to access running process before it is initialised").read();

		match *running_process {
		    Some(pid) => {
			let mut address_space = process_tbl.get_mut(&pid).unwrap().address_space.write();
			match address_space.get_page_range_from_start(addr, size as usize) {
			    Ok(_) => (),
			    Err(_) => panic!("Couldn't get memory at 0x{:x}, already allocated", addr.as_u64()),
			}
		    },
		    None => {
			panic!("Attempted to allocate userspace memory while not in a process address space");
		    },
		}

		addr
	    },
	    MemoryAccessRestriction::UserByAddressSpace(ref mut address_space) => address_space.get_page_range(size),
	    MemoryAccessRestriction::UserByAddressSpaceAndStart(ref mut address_space, addr) => match address_space.get_page_range_from_start(addr, size as usize) {
		Ok(_) => addr,
		Err(_) => panic!("Couldn't get memory at 0x{:x}, already allocated", addr.as_u64()),
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

    fn inner_map(mapper: &mut OffsetPageTable,
		 page_range: PageRangeInclusive,
		 frame_range: Vec<PhysFrame>,
		 access_restriction: &MemoryAccessRestriction) -> Result<(), MapToError<Size4KiB>> {
	let mut frame_allocator = VENIX_FRAME_ALLOCATOR.write();

	for (page, &frame) in page_range.zip(frame_range.iter()) {
	    let flags = match *access_restriction {
		MemoryAccessRestriction::Kernel | MemoryAccessRestriction::EarlyKernel => PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::GLOBAL,
		MemoryAccessRestriction::User => PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
		MemoryAccessRestriction::UserByStart(_) => PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
		MemoryAccessRestriction::UserByAddressSpace(_) => PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
		MemoryAccessRestriction::UserByAddressSpaceAndStart(_, _) => PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
	    };

	    unsafe {
		mapper.map_to(page, frame, flags, frame_allocator.as_mut().expect("Attempted to use missing frame allocator"))?.flush();
	    };
	}

	Ok(())
    }

    if access_restriction == MemoryAccessRestriction::Kernel || access_restriction == MemoryAccessRestriction::EarlyKernel {
	let mut mapper = KERNEL_PAGE_TABLE.write();
	inner_map(mapper.as_mut().expect("Attempted to use missing kernel page table"), page_range, frame_range.clone(), &access_restriction)?;
    } else {
	let direct_map_offset = DIRECT_MAP_OFFSET.get().expect("No direct map offset");
	let pt4_addr = match access_restriction {
	    MemoryAccessRestriction::Kernel | MemoryAccessRestriction::EarlyKernel => unreachable!(),
	    MemoryAccessRestriction::User => scheduler::get_active_page_table() + direct_map_offset,
	    MemoryAccessRestriction::UserByStart(_) => scheduler::get_active_page_table() + direct_map_offset,
	    MemoryAccessRestriction::UserByAddressSpace(ref address_space) => address_space.get_pt4() + direct_map_offset,
	    MemoryAccessRestriction::UserByAddressSpaceAndStart(ref address_space, _) => address_space.get_pt4() + direct_map_offset,
	};
	let pt4_ptr = pt4_addr as *mut PageTable;

	let mut mapper = unsafe {
	    let pt4 = &mut *pt4_ptr;
	    OffsetPageTable::new(pt4, VirtAddr::new(*direct_map_offset))
	};

	inner_map(&mut mapper, page_range, frame_range.clone(), &access_restriction)?;
    }

    if access_restriction == MemoryAccessRestriction::Kernel {
	unsafe {
	    switch_to_user();
	}
    }

    Ok((page_range.start.start_address(), frame_range.iter().map(|frame| frame.start_address()).collect()))
}

pub fn allocate_arbitrary_contiguous_region_kernel(
    phys_addr: usize, size: usize, alloc_type: MemoryAllocationType) -> Result<(VirtAddr, usize), MapToError<Size4KiB>> {
    let start_phys_addr = phys_addr - (phys_addr % 4096);  // Page align
    let end_phys_addr = phys_addr + size + (4096 - ((phys_addr + size) % 4096));  // Page align

    let total_size = end_phys_addr - start_phys_addr;

    let allocated_region = kernel_allocate(
	total_size as u64,
	alloc_type,
	MemoryAllocationOptions::ContiguousByStart(PhysAddr::new(start_phys_addr as u64)),
	MemoryAccessRestriction::EarlyKernel,
    )?.0;

    let offset_from_start = phys_addr - start_phys_addr;
    let virt_addr = allocated_region + offset_from_start as u64;

    Ok((virt_addr, total_size as usize))
}

pub fn get_ptr_in_hhdm(phys_addr: PhysAddr) -> VirtAddr {
    let hhdm = DIRECT_MAP_OFFSET.get().expect("Could not read HHDM");
    VirtAddr::new(phys_addr.as_u64() + hhdm)
}

#[allow(named_asm_labels)]
pub unsafe fn switch_to_kernel() {
    // Kernel always has no flags, user always has PAGE_LEVEL_CACHE_DISABLE
    // If we're already in kernel space, don't save CR3
    core::arch::asm!(
	"mov {r}, cr3",
	"test {r}, 0xFFF",  // 
	"je 2f",
	"mov gs:[{user_cr3}], {r}",
	"2:",

	r = out(reg) _,
	user_cr3 = const(offset_of!(gdt::ProcessorControlBlock, user_cr3)),
    );
    let r = KERNEL_PAGE_FRAME.read();
    let frame = r.as_ref().expect("Attempted to read missing Kernel page frame");
    Cr3::write(*frame, Cr3Flags::empty());
}

#[allow(named_asm_labels)]
pub unsafe fn switch_to_user() {
    // Kernel always has no flags, user always has PAGE_LEVEL_CACHE_DISABLE
    // If user_cr3 doesn't contain a user page table, don't switch to it
    core::arch::asm!(
	"mov {r}, gs:[{user_cr3}]",
	"test {r}, 0xFFF",
	"je 2f",
	"mov cr3, {r}",
	"2:",

	r = out(reg) _,
	user_cr3 = const(offset_of!(gdt::ProcessorControlBlock, user_cr3)),
    );
}
