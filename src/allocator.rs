use spin::RwLock;
use crate::memory;
use linked_list_allocator::LockedHeap;

static KERNEL_HEAP_START: RwLock<u64> = RwLock::new(0);
static KERNEL_HEAP_SIZE: usize = 4 * 1024 * 1024; // Half a MiB

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init() {
    let mut w = KERNEL_HEAP_START.write();
    *w = memory::allocate_by_size_kernel(KERNEL_HEAP_SIZE as u64)
	.expect("Unable to allocate heap").as_u64();
    unsafe {
	ALLOCATOR.lock().init(*w as *mut u8, KERNEL_HEAP_SIZE);
    }
}
