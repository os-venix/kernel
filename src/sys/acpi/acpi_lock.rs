use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering::{Acquire, Release}};

pub struct Mutex {
    state: AtomicU32,
}

impl Mutex {
    pub const fn new() -> Self {
	Self {
	    state: AtomicU32::new(0),
	}
    }

    pub fn lock(&self) {
	while self.state.swap(1, Acquire) == 1 {
	    unsafe { asm!("pause"); }
	}
    }

    pub fn unlock(&mut self) {
	self.state.store(0, Release);
    }
}
