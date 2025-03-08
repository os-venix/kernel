use alloc::vec;
use alloc::vec::Vec;
use core::{mem::MaybeUninit, ptr, sync::atomic::{AtomicUsize, Ordering}, slice, usize};
use x86_64::{PhysAddr, VirtAddr};

use crate::memory;

// An arena allocator, backed by paged memory directly, that can be used to allocate various types, with optional alignment, for DMA purposes
pub struct Arena {
    backing_store: Vec<(VirtAddr, PhysAddr)>,
    next_free_store_spot: AtomicUsize,
}

unsafe impl Sync for Arena {}
unsafe impl Send for Arena {}

impl Default for Arena {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Arena {
    #[must_use]
    pub fn new() -> Self {	
	let (arena_buf_virt, arena_buf_phys) = memory::kernel_allocate(
	    4096, memory::MemoryAllocationType::DMA,
	    memory::MemoryAllocationOptions::Arbitrary,
	    memory::MemoryAccessRestriction::Kernel)
	    .expect("Unable to allocate memory for DMA arena");

        Arena {
	    backing_store: vec!((arena_buf_virt, arena_buf_phys[0])),
            next_free_store_spot: AtomicUsize::new(0),
        }
    }

    /// Get a pointer to a place in the backing store where a value of type T can be placed.
    fn get_ptr_place<T>(&'a self, alignment: usize) -> Option<(&'a mut MaybeUninit<T>, PhysAddr)> {
	if alignment != 0 {
	    if let Err(e) = self.next_free_store_spot.fetch_update(
		Ordering::Release,
		Ordering::SeqCst,
		|x| Some(x + (alignment - (x % alignment)))) {
		panic!("Could not align arena: {}", e);
	    }
	}

        let place = self.next_free_store_spot.fetch_add(
            core::mem::size_of::<T>(),
            Ordering::Release,
        );

	// TODO: get more memory
        if place + core::mem::size_of::<T>() > 4096 * self.backing_store.len() {
            return None;
        }

        let (virt_page, phys_page) = self.backing_store[place / 4096];
	let virt_addr = virt_page + (place as u64 % 4096);
	let phys_addr = phys_page + (place as u64 % 4096);

        Some((unsafe {
	    virt_addr.as_mut_ptr::<T>().cast::<MaybeUninit<T>>().as_mut().unwrap()
	}, phys_addr))
    }

    /// Get a pointer to a place in the backing store where a slice of size l can be placed.
    fn get_slice_place(&'a self, alignment: usize, length: usize) -> Option<(&'a mut [u8], PhysAddr)> {
	if alignment != 0 {
	    if let Err(e) = self.next_free_store_spot.fetch_update(
		Ordering::Release,
		Ordering::SeqCst,
		|x| Some(x + (alignment - (x % alignment)))) {
		panic!("Could not align arena: {}", e);
	    }
	}

        let place = self.next_free_store_spot.fetch_add(length, Ordering::Release);

	// TODO: get more memory
        if place + length > 4096 * self.backing_store.len() {
            return None;
        }

        let (virt_page, phys_page) = self.backing_store[place / 4096];
	let virt_addr = virt_page + (place as u64 % 4096);
	let phys_addr = phys_page + (place as u64 % 4096);

        Some((unsafe {
	    slice::from_raw_parts_mut(virt_addr.as_mut_ptr::<u8>(), length)
	}, phys_addr))
    }

    /// acquire a reference to a value of type T that is initialized with it's default value.
    /// This is useful for types that do not require initialization.
    pub fn acquire_default<T: Default>(&'a self, alignment: usize) -> Option<(&'a mut T, PhysAddr)> {
        let (ptr, phys_addr) = self.get_ptr_place::<T>(alignment)?;

        ptr.write(T::default());

        Some((unsafe {
            ptr::from_mut(ptr)
                .cast::<T>()
                .as_mut()
                .unwrap_unchecked()
        }, phys_addr))
    }

    /// acquire a reference to a value of type T that is initialized with the given value.
    /// This is useful for types that do not require initialization.
    pub fn acquire<T: Clone>(&'a self, alignment: usize, val: &T) -> Option<(&'a mut T, PhysAddr)> {
        let (ptr, phys_addr) = self.get_ptr_place::<T>(alignment)?;

        ptr.write(val.clone());

        Some((unsafe {
            ptr::from_mut(ptr)
                .cast::<T>()
                .as_mut()
                .unwrap_unchecked()
        }, phys_addr))
    }

    /// acquire a reference to a slice of length l, initialized to 0.
    pub fn acquire_slice(&'a self, alignment: usize, length: usize) -> Option<(&'a [u8], PhysAddr)> {
        let (slice, phys_addr) = self.get_slice_place(alignment, length)?;
	slice.fill_with(Default::default);

	Some((slice, phys_addr))
    }
}
