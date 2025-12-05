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
};
use x86_64::registers::control::Cr3;
use alloc::vec::Vec;
use limine::memory_map::Entry;
use alloc::slice;
use alloc::string::String;
use alloc::fmt;
use core::error::Error;

mod frame_allocator;
mod page_allocator;
pub mod user_address_space;
use crate::scheduler;
use crate::process;

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
    Ram,
    Mmio(u64),
    Dma,
    UserBuffer(Vec<PhysAddr>),
}

#[derive(PartialEq, Eq, Debug)]
pub enum MemoryAccessRestriction {
    User,
    UserByStart(VirtAddr),
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
	    frame_allocator::VenixFrameAllocator::new(memory_map)
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

pub fn user_allocate(
    size: u64,
    access_restriction: MemoryAccessRestriction,
    address_space: &mut user_address_space::AddressSpace) -> Result<(VirtAddr, Vec<PhysAddr>), MapToError<Size4KiB>> {
    let page_range = {
	let start = match access_restriction {
	    MemoryAccessRestriction::User => address_space.get_page_range(size),
	    MemoryAccessRestriction::UserByStart(addr) => match address_space.get_page_range_from_start(addr, size as usize) {
		Ok(_) => addr,
		Err(_) => panic!("Couldn't get memory at 0x{:x}, already allocated", addr.as_u64()),
	    }
	};

	let end = start + (size - 1);

	let start_page = Page::containing_address(start);
	let end_page = Page::containing_address(end);

	Page::range_inclusive(start_page, end_page)
    };

    let frame_range: Vec<PhysFrame> = {
	let mut range = Vec::new();	    
	let mut frame_allocator = VENIX_FRAME_ALLOCATOR.write();

	for _ in page_range {
	    let frame = frame_allocator.as_mut().expect("Attempted to use missing frame allocator").allocate_frame()
		.ok_or(MapToError::FrameAllocationFailed)?;
	    range.push(frame);
	}

	range
    };

    let direct_map_offset = DIRECT_MAP_OFFSET.get().expect("No direct map offset");
    let pt4_addr = match access_restriction {
	MemoryAccessRestriction::User => address_space.get_pt4() + direct_map_offset,
	MemoryAccessRestriction::UserByStart(_) => address_space.get_pt4() + direct_map_offset,
    };
    let pt4_ptr = pt4_addr as *mut PageTable;

    let mut mapper = unsafe {
	let pt4 = &mut *pt4_ptr;
	OffsetPageTable::new(pt4, VirtAddr::new(*direct_map_offset))
    };

    let mut frame_allocator = VENIX_FRAME_ALLOCATOR.write();

    for (page, &frame) in page_range.zip(frame_range.iter()) {
	address_space.assign_virt_phys(page.start_address(), frame.start_address());
	let flags = match access_restriction {
	    MemoryAccessRestriction::User => PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
	    MemoryAccessRestriction::UserByStart(_) => PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
	};

	unsafe {
	    mapper.map_to(page, frame, flags, frame_allocator.as_mut().expect("Attempted to use missing frame allocator"))?.flush();
	};
    }

    Ok((page_range.start.start_address(), frame_range.iter().map(|frame| frame.start_address()).collect()))
}

pub fn kernel_allocate(
    size: u64,
    alloc_type: MemoryAllocationType) -> Result<(VirtAddr, Vec<PhysAddr>), MapToError<Size4KiB>> {

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

    let frame_range: Vec<PhysFrame> = match alloc_type {
	MemoryAllocationType::Ram => {
	    let mut range = Vec::new();	    
	    let mut frame_allocator = VENIX_FRAME_ALLOCATOR.write();

	    for _ in page_range {
		let frame = frame_allocator.as_mut().expect("Attempted to use missing frame allocator").allocate_frame()
		    .ok_or(MapToError::FrameAllocationFailed)?;
		range.push(frame);
	    }

	    range
	},
	MemoryAllocationType::Mmio(start_addr) =>
	    (0 .. size)
	    .step_by(4096)
	    .map(|addr| PhysFrame::containing_address(PhysAddr::new(start_addr + addr)))
	    .collect(),
	MemoryAllocationType::Dma => {
	    let aligned_size = size.div_ceil(4096) * 4096;

	    let start = {
		let mut frame_allocator = VENIX_FRAME_ALLOCATOR.write();
		frame_allocator.as_mut().expect("Attempted to use missing frame allocator").allocate_dma_frames(aligned_size)
		    .ok_or(MapToError::FrameAllocationFailed)?
	    };

	    (0 .. size)
		.step_by(4096)
		.map(|addr| PhysFrame::containing_address(start + addr))
		.collect()
	},
	MemoryAllocationType::UserBuffer(buf) => buf.iter()
	    .map(|p| PhysFrame::from_start_address(*p).unwrap())
	    .collect(),
    };

    let mut mapper = KERNEL_PAGE_TABLE.write();
    let mut frame_allocator = VENIX_FRAME_ALLOCATOR.write();
    
    for (page, &frame) in page_range.zip(frame_range.iter()) {
	let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::GLOBAL;
	unsafe {
	    mapper.as_mut().unwrap().map_to(page, frame, flags, frame_allocator.as_mut().expect("Attempted to use missing frame allocator"))?.flush();
	};
    }

    Ok((page_range.start.start_address(), frame_range.iter().map(|frame| frame.start_address()).collect()))
}

// This function handles MMIO allocation. The reason we use it, rather than calling kernel_allocate directly, is that
// in theory, an MMIO region may span page boundaries, and the caller should not be expected to properly align.
//
// This function performs that alignment.
pub fn allocate_mmio(
    phys_addr: usize, size: usize) -> Result<VirtAddr, MapToError<Size4KiB>> {
    let start_phys_addr = phys_addr - (phys_addr % 4096);  // Page align
    let end_phys_addr = (phys_addr + size).div_ceil(4096) * 4096;

    let total_size = end_phys_addr - start_phys_addr;

    let allocated_region = kernel_allocate(
	total_size as u64,
	MemoryAllocationType::Mmio(start_phys_addr as u64)
    )?.0;

    let offset_from_start = phys_addr - start_phys_addr;
    let virt_addr = allocated_region + offset_from_start as u64;

    Ok(virt_addr)
}

pub fn get_ptr_in_hhdm(phys_addr: PhysAddr) -> VirtAddr {
    let hhdm = DIRECT_MAP_OFFSET.get().expect("Could not read HHDM");
    VirtAddr::new(phys_addr.as_u64() + hhdm)
}

#[derive(Debug)]
pub enum CopyError {
    Fault,               // page not present / translation failed
    TempAllocFailed,     // couldn't allocate temporary kernel mapping
}

impl fmt::Display for CopyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
	match self {
	    CopyError::Fault => write!(f, "Attempted to copy from a user page not in the user page map."),
	    CopyError::TempAllocFailed => write!(f, "Couldn't temp map user pages into kernel for copy."),
	}
    }
}

impl Error for CopyError {}

#[derive(Debug)]
pub enum UserStringCopyError {
    Fault,               // page not present / translation failed
    TempAllocFailed,     // couldn't allocate temporary kernel mapping
    TooLong,             // attempted to copy a string >1MiB
    InvalidUtf8,         // couldn't translate the string
}

fn copy_to_user_internal(
    address_space: &user_address_space::AddressSpace, dest: VirtAddr, src: &[u8]) -> Result<(), CopyError> {
    if src.is_empty() {
	return Ok(());
    }

    let total_len = src.len() as u64;
    let first_page_index = dest.as_u64() / 4096;
    let last_page_index = (dest.as_u64() + total_len - 1) / 4096;
    let n_pages = (last_page_index - first_page_index + 1) as usize;

    // Walk all pages and validate presence + writability, collecting phys page bases.
    // We also need the per-page user offset and per-page copy length.
    let mut phys_pages: Vec<PhysAddr> = Vec::with_capacity(n_pages);

    // We'll maintain an offset into the src buffer that we are copying.
    let mut cur_vaddr = dest.as_u64();

    for _ in 0..n_pages {
        // Translate the start of this page (virtual address)
        let page_base_vaddr = VirtAddr::new((cur_vaddr / 4096) * 4096);
        match address_space.mapped_regions.get(&page_base_vaddr) {
            Some(phys_page_base) => phys_pages.push(*phys_page_base),  // TODO: this should validate permissions. The page should be both user accessible, and writable
            None => return Err(CopyError::Fault),  // Page is not in the shadow map. Ithasn't been mapped
        }

        // advance
        cur_vaddr = page_base_vaddr.as_u64() + 4096; // next page start (even if dest started in middle)
    }

    let kernel_buf = match kernel_allocate(n_pages as u64 * 4096, MemoryAllocationType::UserBuffer(phys_pages)) {
	Ok((buf, _)) => buf,
	Err(_) => return Err(CopyError::TempAllocFailed),
    };

    let data_to = unsafe {
	slice::from_raw_parts_mut((kernel_buf + (dest.as_u64() % 4096)).as_mut_ptr::<u8>(), src.len())
    };

    data_to.copy_from_slice(src);

    let mut mapper = KERNEL_PAGE_TABLE.write();
    for i in 0 .. n_pages {
	let p: Page<Size4KiB> = Page::from_start_address(kernel_buf + i as u64 * 4096).expect("Malformed start address");
	let (_, flush) = mapper.as_mut().unwrap().unmap(p).expect("Attempting to unmap page failed");
	flush.flush();
    }

    Ok(())
}

fn copy_from_user_internal(
    address_space: &user_address_space::AddressSpace, src: VirtAddr, len: usize) -> Result<Vec<u8>, CopyError> {
    if len == 0 {
	return Ok(Vec::new());
    }

    let first_page_index = src.as_u64() / 4096;
    let last_page_index = (src.as_u64() + len as u64 - 1) / 4096;
    let n_pages = (last_page_index - first_page_index + 1) as usize;

    // Walk all pages and validate presence + writability, collecting phys page bases.
    // We also need the per-page user offset and per-page copy length.
    let mut phys_pages: Vec<PhysAddr> = Vec::with_capacity(n_pages);

    // We'll maintain an offset into the src buffer that we are copying.
    let mut cur_vaddr = src.as_u64();

    for _ in 0..n_pages {
        // Translate the start of this page (virtual address)
        let page_base_vaddr = VirtAddr::new((cur_vaddr / 4096) * 4096);
        match address_space.mapped_regions.get(&page_base_vaddr) {
            Some(phys_page_base) => phys_pages.push(*phys_page_base),  // TODO: this should validate permissions. The page should be both user accessible, and writable
            None => return Err(CopyError::Fault),  // Page is not in the shadow map. Ithasn't been mapped
        }

        // advance
        cur_vaddr = page_base_vaddr.as_u64() + 4096; // next page start (even if dest started in middle)
    }

    let kernel_buf = match kernel_allocate(n_pages as u64 * 4096, MemoryAllocationType::UserBuffer(phys_pages)) {
	Ok((buf, _)) => buf,
	Err(_) => return Err(CopyError::TempAllocFailed),
    };

    let mut result: Vec<u8> = Vec::with_capacity(len);
    let mut remaining = len;

    for i in 0 .. n_pages {
        let kva = (kernel_buf + (i as u64) * 4096).as_ptr::<u8>();
        // For first page, start at user_buf % PAGE_SIZE; otherwise 0.
        let page_offset = if i == 0 {
            (src.as_u64() % 4096) as usize
        } else {
            0usize
        };

        let available_in_page = 4096_usize - page_offset;
        let to_copy = if remaining <= available_in_page { remaining } else { available_in_page };

        // Build a slice from mapped kernel VA and append
        unsafe {
            let src_slice = core::slice::from_raw_parts(kva.add(page_offset), to_copy);
            result.extend_from_slice(src_slice);
        }

        remaining -= to_copy;
        if remaining == 0 { break; }
    }

    let mut mapper = KERNEL_PAGE_TABLE.write();
    for i in 0 .. n_pages {
	let p: Page<Size4KiB> = Page::from_start_address(kernel_buf + i as u64 * 4096).expect("Malformed start address");
	let (_, flush) = mapper.as_mut().unwrap().unmap(p).expect("Attempting to unmap page failed");
	flush.flush();
    }

    Ok(result)
}

pub fn copy_to_user(dest: VirtAddr, src: &[u8]) -> Result<(), CopyError> {
    let process = scheduler::get_current_process();
    let mut task_type = process.task_type.write();
    match *task_type {
	process::TaskType::Kernel => {
	    let data_to = unsafe {
		slice::from_raw_parts_mut(dest.as_mut_ptr::<u8>(), src.len())
	    };
	    data_to.copy_from_slice(src);

	    Ok(())
	},
	process::TaskType::User(ref mut address_space) => copy_to_user_internal(address_space, dest, src),
    }
}

pub fn copy_from_user(src: VirtAddr, len: usize) -> Result<Vec<u8>, CopyError> {
    let process = scheduler::get_current_process();
    let mut task_type = process.task_type.write();
    match *task_type {
	process::TaskType::Kernel => {
	    let data_to = unsafe {
		slice::from_raw_parts(src.as_ptr::<u8>(), len)
	    };

	    Ok(data_to.to_vec())
	},
	process::TaskType::User(ref mut address_space) => copy_from_user_internal(address_space, src, len),
    }
}

pub fn copy_string_from_user(user_buf: VirtAddr) -> Result<String, UserStringCopyError> {
    const PAGE_SIZE: usize = 4096;
    const MAX_BYTES: usize = 1024 * 1024;  // 1 MB cap to avoid DoS

    let mut cursor = user_buf;
    let mut collected: Vec<u8> = Vec::new();

    loop {
        // Copy one page
        let bytes = copy_from_user(cursor, PAGE_SIZE)
            .map_err(|e| match e {
		CopyError::Fault => UserStringCopyError::Fault,
		CopyError::TempAllocFailed => UserStringCopyError::TempAllocFailed,
	    })?;

        // Look for NUL terminator
        if let Some(pos) = bytes.iter().position(|&b| b == 0) {
            // push everything before the NUL
            collected.extend_from_slice(&bytes[..pos]);

            // UTF-8 validation
            return String::from_utf8(collected)
                .map_err(|_| UserStringCopyError::InvalidUtf8);
        }

        // No NUL found; append entire page
        collected.extend_from_slice(&bytes);

        // DoS / runaway prevention
        if collected.len() > MAX_BYTES {
            return Err(UserStringCopyError::TooLong);
        }

        // Move to next page
        cursor = VirtAddr::new(cursor.as_u64() + PAGE_SIZE as u64);
    }
}

pub fn copy_string_to_user(dst: VirtAddr, src: String) -> Result<(), CopyError> {
    // Ensure C-string termination. Avoid reallocating if the user already provided a null.
    let mut bytes = src.into_bytes();
    if !bytes.ends_with(&[0]) {
        bytes.push(0);
    }

    // Delegate to the generic byte-copying function.
    copy_to_user(dst, &bytes)
}

pub fn copy_value_from_user<T: Copy>(user_ptr: VirtAddr) -> Result<T, CopyError> {
    use core::{mem, ptr};
    let size = mem::size_of::<T>();

    let bytes = copy_from_user(user_ptr, size)?;
    let mut tmp = mem::MaybeUninit::<T>::uninit();

    unsafe {
        ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            tmp.as_mut_ptr() as *mut u8,
            size,
        );

        Ok(tmp.assume_init())
    }
}

pub fn copy_value_to_user<T: Copy>(user_ptr: VirtAddr, value: &T) -> Result<(), CopyError> {
    use core::{mem, slice};
    let size = mem::size_of::<T>();

    let bytes: &[u8] = unsafe {
        slice::from_raw_parts(
            (value as *const T) as *const u8,
            size,
        )
    };
    copy_to_user(user_ptr, bytes)
}
