use crate::driver;
use crate::memory;
use alloc::sync::Arc;
use core::ascii;
use core::slice;
use alloc::string::String;
use spin::Mutex;

pub struct ConsoleDevice {}
unsafe impl Send for ConsoleDevice { }
unsafe impl Sync for ConsoleDevice { }
impl driver::Device for ConsoleDevice {
    fn read(&self, offset: u64, size: u64, access_restriction: memory::MemoryAccessRestriction) -> Result<*const u8, ()> {
	panic!("User input isn't supported yet");
    }
    fn write(&self, buf: *const u8, size: u64) -> Result<u64, ()> {
	let s = unsafe {
	    slice::from_raw_parts(buf as *const ascii::Char, size as usize).as_str()
	};

	let printk = crate::PRINTK.get().expect("Unable to get printk");
	printk.write_str(s);

	Ok(size)
    }
}

pub fn init() {
    let device = Arc::new(Mutex::new(ConsoleDevice {}));
    let devid = driver::register_device(device);
    driver::register_devfs(String::from("console"), devid);
}
