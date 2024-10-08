use spin::RwLock;
use x86_64::structures::paging::{PageTable, RecursivePageTable};

mod frame_allocator;

static KERNEL_PAGE_TABLE: RwLock<Option<RecursivePageTable>> = RwLock::new(None);
static VENIX_FRAME_ALLOCATOR: RwLock<Option<frame_allocator::VenixFrameAllocator>> = RwLock::new(None);

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
}
