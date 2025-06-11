use crate::driver;
use crate::memory;
use alloc::sync::Arc;
use core::ascii;
use core::slice;
use alloc::string::String;
use spin::{Once, RwLock};
use bytes::Bytes;
use core::cmp;
use core::future::Future;
use core::pin::Pin;
use core::task::Context;
use core::task::Poll;
use alloc::boxed::Box;
use futures_util::future::BoxFuture;

use crate::sys::ioctl;
use crate::sys::syscall;

// Although strictly speaking a subsystem, not a device, this is implemented as a device. This allows us to make use of devfs,
// allowing for reading and writing from usermode.
pub struct ConsoleDevice {
    pub key_buffer: RwLock<Bytes>,
    pub local_loopback: bool,
}
unsafe impl Send for ConsoleDevice { }
unsafe impl Sync for ConsoleDevice { }

impl ConsoleDevice {
    fn register_key(&self, k: char) {
	{
	    let mut key_buffer = self.key_buffer.write();
	    let new_buf = [key_buffer.as_ref(), &[k as u8]].concat().into();
	    *key_buffer = new_buf;
	}

	if self.local_loopback {
	    let printk = crate::PRINTK.get().expect("Unable to get printk");
	    printk.write_char(k);
	}
    }
}

impl driver::Device for ConsoleDevice {
    fn read(self: Arc<Self>, offset: u64, size: u64, access_restriction: memory::MemoryAccessRestriction) -> BoxFuture<'static, Result<Bytes, syscall::CanonicalError>> {
	struct Wait {
	    size: u64
	}

	impl Future for Wait {
	    type Output = Result<Bytes, syscall::CanonicalError>;

	    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
		let console = CONSOLE.get().unwrap();
		let mut key_buffer = console.key_buffer.write();
		
		// Check for a complete line (canonical mode)
		if let Some(pos) = (*key_buffer).iter().position(|&b| b == b'\n') {
		    // Include the newline character
		    let line_len = pos + 1;
		    let to_read = cmp::min(line_len, self.size as usize);

		    Poll::Ready(Ok((*key_buffer).split_to(to_read)))
		} else {
		    // No full line available yet
		    Poll::Pending
		}
	    }
	}

	Box::pin(async move {
	    Wait { size }.await
	})
    }

    fn write(&self, buf: *const u8, size: u64) -> Result<u64, ()> {
	let s = unsafe {
	    slice::from_raw_parts(buf as *const ascii::Char, size as usize).as_str()
	};

	let printk = crate::PRINTK.get().expect("Unable to get printk");
	printk.write_str(s);

	Ok(size)
    }

    fn ioctl(&self, ioctl: u64) -> Result<(Bytes, usize, u64), ()> {
	// TODO: This should be converted way earlier to an IoCtl enum. Putting here for now because I'm lazy
	match ioctl {
	    0x5413 /*ioctl::IoCtl::TIOCGWINSZ */ => {
		let printk = crate::PRINTK.get().expect("Unable to get printk");
		Ok((Bytes::copy_from_slice(&[
		    printk.get_rows(),
		    printk.get_cols(),
		    0, 0]), 4usize, 0))
	    },
	    _ => Err(()),
	}
    }
}

static CONSOLE: Once<Arc<ConsoleDevice>> = Once::new();

pub fn init() {
    let device = Arc::new(ConsoleDevice {
	key_buffer: RwLock::new(Bytes::new()),
	local_loopback: true,
    });
    CONSOLE.call_once(|| device.clone());
    let devid = driver::register_device(device);
    driver::register_devfs(String::from("console"), devid);
}

pub fn register_keypress(k: char) {
    CONSOLE.get().expect("Attempted to register keypress before Console subsystem initialised").register_key(k);
}
