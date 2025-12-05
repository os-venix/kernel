use core::sync::atomic::{fence, Ordering};
use core::slice;

pub struct DmaBuffer {
    ptr: *mut u8,
    len: usize,
}

impl DmaBuffer {
    /// Create a new DMA buffer wrapper from raw pointer and length
    /// Unsafe because caller must ensure the pointer is valid and points to DMA-safe memory
    pub unsafe fn new(ptr: *mut u8, len: usize) -> Self {
        Self { ptr, len }
    }

    /// Invalidate CPU caches for the DMA buffer before reading fresh data written by device
    #[inline(always)]
    pub fn invalidate_cache(&self) {
        unsafe {
            let mut p = self.ptr;
            let end = self.ptr.add(self.len);

            // Flush each cache line covering this buffer
            while p < end {
                core::arch::x86_64::_mm_clflush(p as *const _);
                p = p.add(64); // Assuming 64-byte cache line
            }
        }
        // Fence to ensure cache invalidation is completed before further loads
        fence(Ordering::SeqCst);
    }

    /// Flush CPU caches to device before device reads buffer written by CPU
    #[inline(always)]
    #[allow(dead_code)]
    pub fn flush_cache(&self) {
        unsafe {
            let mut p = self.ptr;
            let end = self.ptr.add(self.len);

            while p < end {
                core::arch::x86_64::_mm_clflush(p as *const _);
                p = p.add(64);
            }
        }
        fence(Ordering::SeqCst);
    }

    /// Returns a shared slice after invalidating caches and fencing (for reading fresh data from device)
    pub fn as_slice(&self) -> &[u8] {
        self.invalidate_cache();
        fence(Ordering::Acquire);

        unsafe { slice::from_raw_parts(self.ptr, self.len) }
    }

    /// Returns a mutable slice after flushing caches and fencing (for writing data to device)
    #[allow(dead_code)]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        fence(Ordering::Release);
        self.flush_cache();

        unsafe { slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}
