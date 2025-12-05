use crate::driver;
use crate::memory;
use alloc::sync::Arc;
use core::ascii;
use core::slice;
use alloc::string::String;
use spin::{Once, RwLock};
use bytes::{Bytes, BytesMut, BufMut};
use core::cmp;
use core::ffi::{c_int, c_uint};
use core::future::Future;
use core::pin::Pin;
use core::task::Context;
use core::task::Poll;
use alloc::boxed::Box;
use futures_util::future::BoxFuture;
use x86_64::VirtAddr;

use crate::sys::ioctl;
use crate::sys::syscall;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Termios {
    iflag: c_uint,
    oflag: c_uint,
    cflag: c_uint,
    lflag: c_uint,
    cc: [c_uint; 11],
    ibaud: c_uint,
    obaud: c_uint,
}

// Although strictly speaking a subsystem, not a device, this is implemented as a device. This allows us to make use of devfs,
// allowing for reading and writing from usermode.
pub struct ConsoleDevice {
    pub key_buffer: RwLock<BytesMut>,
    pgrp: RwLock<u64>,

    // Input flags
    crnl: RwLock<bool>,
    nlcr: RwLock<bool>,

    // Local flags
    canonical: RwLock<bool>,
    pub local_loopback: RwLock<bool>,
}
unsafe impl Send for ConsoleDevice { }
unsafe impl Sync for ConsoleDevice { }

impl ConsoleDevice {
    fn register_key(&self, k: char) {
	{
	    let mut key_buffer = self.key_buffer.write();
	    key_buffer.put_u8(k as u8);
	}

	let local_loopback = self.local_loopback.read();
	if *local_loopback {
	    let printk = crate::PRINTK.get().expect("Unable to get printk");
	    printk.write_char(k);
	}
    }
}

impl driver::Device for ConsoleDevice {
    fn read(self: Arc<Self>, _offset: u64, size: u64) -> BoxFuture<'static, Result<Bytes, syscall::CanonicalError>> {
	struct Wait {
	    size: u64
	}

	impl Future for Wait {
	    type Output = Result<Bytes, syscall::CanonicalError>;

	    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
		let console = CONSOLE.get().unwrap();
		let mut key_buffer = console.key_buffer.write();

		let canonical = console.canonical.read();

		let mut return_buf = if *canonical {
		    // Check for a complete line (canonical mode)
		    if let Some(pos) = (*key_buffer).iter().position(|&b| b == b'\r') {
			// Include the newline character
			let line_len = pos + 1;
			let to_read = cmp::min(line_len, self.size as usize);

			(*key_buffer).split_to(to_read)
		    } else {
			return Poll::Pending;
		    }
		} else {
		    // TODO - this should handle VMIN, and doesn't yet - right now, VMIN is assumed as 1
		    let l = (*key_buffer).len();
		    if l > 0 {
			(*key_buffer).split_to(l)
		    } else {
			return Poll::Pending;
		    }
		};

		let crnl = console.crnl.read();
		let nlcr = console.nlcr.read();

		return_buf.iter_mut()
		    .for_each(|c| {
			if *c == b'\r' && *crnl {
			    *c = b'\n';
			} else if *c == b'\n' && *nlcr {
			    *c = b'\r'
			}
		    });

		Poll::Ready(Ok(return_buf.freeze()))
	    }
	}

	Box::pin(Wait { size })
    }

    fn write(&self, buf: *const u8, size: u64) -> Result<u64, ()> {
	// TODO - this should handle ONLCR and doesn't. At present, due to the way printk works, all \n implies \r, which isn't correct
	let s = unsafe {
	    slice::from_raw_parts(buf as *const ascii::Char, size as usize).as_str()
	};

	let printk = crate::PRINTK.get().expect("Unable to get printk");
	printk.write_str(s);

	Ok(size)
    }

    fn ioctl(self: Arc<Self>, ioctl: ioctl::IoCtl, buf: u64) -> Result<u64, ()> {
	match ioctl {
	    ioctl::IoCtl::TCGETS => {
		// For now, we'll stub this out
		Ok(0)
	    },
	    ioctl::IoCtl::TCSETS => {
		let termios = memory::copy_value_from_user::<Termios>(VirtAddr::new(buf)).unwrap();

		// Input flags
		let mut crnl = self.crnl.write();
		let mut nlcr = self.nlcr.write();
		*crnl = termios.iflag & 2 != 0;
		*nlcr = termios.iflag & 0x20 != 0;

		// Local flags
		let mut canonical = self.canonical.write();
		let mut local_loopback = self.local_loopback.write();
		*canonical = termios.lflag & 0x10 != 0;
		*local_loopback = termios.lflag & 0x01 != 0;

		Err(())
	    },
	    ioctl::IoCtl::TIOCGWINSZ => {
		let printk = crate::PRINTK.get().expect("Unable to get printk");

		let read_buf = Bytes::copy_from_slice(&[
		    printk.get_rows(),
		    printk.get_cols(),
		    0, 0]);
		memory::copy_to_user(VirtAddr::new(buf), read_buf.as_ref()).unwrap();
		Ok(0)
	    },
	    ioctl::IoCtl::TIOCGPGRP => {
		let pgrp = self.pgrp.read();
		Ok(*pgrp)
	    },
	    ioctl::IoCtl::TIOCSPGRP => {
		let mut pgrp = self.pgrp.write();
		*pgrp = memory::copy_value_from_user::<c_int>(VirtAddr::new(buf)).unwrap() as u64;

		Ok(0)
	    },
	}
    }

    fn poll(self: Arc<Self>, events: syscall::PollEvents) -> BoxFuture<'static, syscall::PollEvents> {
	struct Wait {
	    events: syscall::PollEvents
	}

	impl Future for Wait {
	    type Output = syscall::PollEvents;

	    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
		let mut revents = syscall::PollEvents::empty();

		// We can always write
		if self.events.contains(syscall::PollEvents::Out) {
                    revents |= syscall::PollEvents::Out;
		}

		if self.events.contains(syscall::PollEvents::In) {
		    let console = CONSOLE.get().unwrap();
		    let key_buffer = console.key_buffer.read();

		    let canonical = console.canonical.read();

		    let _return_buf = if *canonical {
			// Check for a complete line (canonical mode)
			if let Some(_pos) = (*key_buffer).iter().position(|&b| b == b'\r') {
			    revents |= syscall::PollEvents::In;
			}
		    } else {
			// TODO - this should handle VMIN, and doesn't yet - right now, VMIN is assumed as 1
			let l = (*key_buffer).len();
			if l > 0 {
			    revents |= syscall::PollEvents::In;
			}
		    };
		}

		if revents.is_empty() {
		    return Poll::Pending;
		}

		Poll::Ready(revents)
	    }
	}

	Box::pin(Wait { events })
    }
}

static CONSOLE: Once<Arc<ConsoleDevice>> = Once::new();

pub fn init() {
    let device = Arc::new(ConsoleDevice {
	key_buffer: RwLock::new(BytesMut::new()),
	local_loopback: RwLock::new(true),
	pgrp: RwLock::new(0),
	canonical: RwLock::new(true),
	crnl: RwLock::new(false),
	nlcr: RwLock::new(false),
    });
    CONSOLE.call_once(|| device.clone());
    let devid = driver::register_device(device);
    driver::register_devfs(String::from("console"), devid);
}

pub fn register_keypress(k: char) {
    CONSOLE.get().expect("Attempted to register keypress before Console subsystem initialised").register_key(k);
}
