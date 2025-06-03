use crate::driver;
use crate::memory;
use alloc::sync::Arc;
use core::ascii;
use core::slice;
use alloc::string::String;
use spin::{Once, Mutex};
use bytes::Bytes;

use crate::sys::ioctl;

// Although strictly speaking a subsystem, not a device, this is implemented as a device. This allows us to make use of devfs,
// allowing for reading and writing from usermode.
pub struct ConsoleDevice {
    key_buffer: Bytes,
    local_loopback: bool,
}
unsafe impl Send for ConsoleDevice { }
unsafe impl Sync for ConsoleDevice { }

impl ConsoleDevice {
    fn register_key(&mut self, k: char) {
	self.key_buffer = [self.key_buffer.clone(), Bytes::copy_from_slice(&[k as u8])].concat().into();

	if self.local_loopback {
	    let printk = crate::PRINTK.get().expect("Unable to get printk");
	    printk.write_char(k);
	}
    }
}

impl driver::Device for ConsoleDevice {
    fn read(&mut self, offset: u64, size: u64, access_restriction: memory::MemoryAccessRestriction) -> Result<Bytes, ()> {
	if self.key_buffer.len() < size as usize {
	    Ok(self.key_buffer.split_to(self.key_buffer.len()))
	} else {
	    Ok(self.key_buffer.split_to(size as usize))
	}
    }

    fn write(&mut self, buf: *const u8, size: u64) -> Result<u64, ()> {
	let s = unsafe {
	    slice::from_raw_parts(buf as *const ascii::Char, size as usize).as_str()
	};

	let printk = crate::PRINTK.get().expect("Unable to get printk");
	printk.write_str(s);

	Ok(size)
    }

    fn ioctl(&self, ioctl: u64) -> Result<(Bytes, usize, u64), ()> {
	let (rows, cols) = {
	    let printk = crate::PRINTK.get().expect("Unable to get printk");
	    (printk.get_rows(),
	     printk.get_cols())
	};

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

static CONSOLE: Once<Arc<Mutex<ConsoleDevice>>> = Once::new();

pub fn init() {
    let device = Arc::new(Mutex::new(ConsoleDevice {
	key_buffer: Bytes::new(),
	local_loopback: true,
    }));
    CONSOLE.call_once(|| device.clone());
    let devid = driver::register_device(device);
    driver::register_devfs(String::from("console"), devid);
}

pub fn register_keypress(k: char) {
    let mut console = CONSOLE.get().expect("Attempted to register keypress before Console subsystem initialised").lock();
    console.register_key(k);
}
