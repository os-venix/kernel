use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering::{Acquire, SeqCst, Release}};

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

pub struct Semaphore {
    state: AtomicU32,
}

impl Semaphore {
    pub const fn new() -> Self {
	Self {
	    state: AtomicU32::new(0),
	}
    }

    pub fn signal(&mut self) {
	self.state.fetch_add(1, Release);
    }

    pub fn wait_for_event(&mut self, timeout: Option<u16>) -> bool {
	while self.state.fetch_update(SeqCst, SeqCst, |x| if x == 0 { None } else { Some(x - 1) }).is_err() {

	}

	true
    }

    pub fn reset(&mut self) {
	self.state.store(0, SeqCst);
    }
}
