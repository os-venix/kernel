use crate::driver;
use crate::memory;
use alloc::sync::Arc;
use alloc::string::String;
use spin::{Once, RwLock};
use bytes::{Bytes, BytesMut, BufMut};
use core::cmp;
use core::ffi::{c_int, c_uint};
use core::task::Context;
use core::task::Poll;
use alloc::boxed::Box;
use futures_util::future::BoxFuture;
use x86_64::VirtAddr;
use core::task::Waker;
use core::future::poll_fn;
use futures_util::FutureExt;
use spin::Mutex;

use crate::sys::ioctl;
use crate::sys::syscall::{CanonicalError, PollEvents};
use crate::vfs;

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

    read_waker: RwLock<Option<Waker>>,

    fsi: Mutex<vfs::filesystem::FileSystemInstance>,
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

	let mut read_waker = self.read_waker.write();
	if let Some(waker) = read_waker.take() {
	    waker.wake();
	    *read_waker = None;
	}
    }
}

impl vfs::filesystem::VNode for ConsoleDevice {
    fn inode(&self) -> u64 {
	0
    }
    
    fn kind(&self) -> vfs::filesystem::VNodeKind {
	vfs::filesystem::VNodeKind::Directory
    }
	
    fn stat(&self) -> Result<vfs::filesystem::Stat, CanonicalError> {
	Err(CanonicalError::Inval)
    }

    fn open(self: Arc<Self>/*, flags: OpenFlags */) -> Result<Arc<dyn vfs::filesystem::FileHandle>, CanonicalError> {
	Ok(self.clone())
    }
    
    fn filesystem(&self) -> Arc<dyn vfs::filesystem::FileSystem> {
	driver::get_devfs()
    }

    fn fsi(&self) -> vfs::filesystem::FileSystemInstance {
	*self.fsi.lock()
    }

    fn parent(&self) -> Result<Arc<dyn vfs::filesystem::VNode>, CanonicalError> {
	unimplemented!();
    }

    fn set_fsi(self: Arc<Self>, fsi: vfs::filesystem::FileSystemInstance) {
	*(self.fsi.lock()) = fsi;
    }
}

impl vfs::filesystem::FileHandle for ConsoleDevice {
    fn read(self: Arc<Self>, len: u64) -> BoxFuture<'static, Result<bytes::Bytes, CanonicalError>> {
	Box::pin(poll_fn(move |cx: &mut Context<'_>| {
	    let mut key_buffer = self.key_buffer.write();
	    let canonical = self.canonical.read();

	    let mut return_buf = if *canonical {
		// Check for a complete line (canonical mode)
		if let Some(pos) = (*key_buffer).iter().position(|&b| b == b'\r') {
		    // Include the newline character
		    let line_len = pos + 1;
		    let to_read = cmp::min(line_len, len as usize);

		    (*key_buffer).split_to(to_read)
		} else {
		    let mut read_waker = self.read_waker.write();
		    *read_waker = Some(cx.waker().clone());
		    return Poll::Pending;
		}
	    } else {
		// TODO - this should handle VMIN, and doesn't yet - right now, VMIN is assumed as 1
		let l = (*key_buffer).len();
		if l > 0 {
		    (*key_buffer).split_to(l)
		} else {
		    let mut read_waker = self.read_waker.write();
		    *read_waker = Some(cx.waker().clone());
		    return Poll::Pending;
		}
	    };

	    let crnl = self.crnl.read();
	    let nlcr = self.nlcr.read();

	    return_buf.iter_mut()
		.for_each(|c| {
		    if *c == b'\r' && *crnl {
			*c = b'\n';
		    } else if *c == b'\n' && *nlcr {
			*c = b'\r'
		    }
		});

	    Poll::Ready(Ok(return_buf.freeze()))
	}))
    }
    
    fn write(self: Arc<Self>, buf: bytes::Bytes) -> BoxFuture<'static, Result<u64, CanonicalError>> {
	async move {
	    // TODO - this should handle ONLCR and doesn't.
	    // At present, due to the way printk works, all \n implies \r, which isn't correct
	    let printk = crate::PRINTK.get().expect("Unable to get printk");
	    printk.write_str(str::from_utf8(&buf).map_err(|_| CanonicalError::Inval)?);

	    Ok(buf.len() as u64)
	}.boxed()
    }

    fn poll(self: Arc<Self>, events: PollEvents) -> BoxFuture<'static, Result<PollEvents, CanonicalError>> {
	Box::pin(poll_fn(move |cx: &mut Context<'_>| {
	    let mut revents = PollEvents::empty();

	    // We can always write
	    if events.contains(PollEvents::Out) {
                revents |= PollEvents::Out;
	    }

	    if events.contains(PollEvents::In) {
		let key_buffer = self.key_buffer.read();
		let canonical = self.canonical.read();

		if *canonical {
		    // Check for a complete line (canonical mode)
		    if let Some(_pos) = (*key_buffer).iter().position(|&b| b == b'\r') {
			revents |= PollEvents::In;
		    }
		} else {
		    // TODO - this should handle VMIN, and doesn't yet - right now, VMIN is assumed as 1
		    let l = (*key_buffer).len();
		    if l > 0 {
			revents |= PollEvents::In;
		    }
		};
		
		if revents.is_empty() {
		    let mut read_waker = self.read_waker.write();
		    *read_waker = Some(cx.waker().clone());
		    return Poll::Pending;
		}
	    }

	    Poll::Ready(Ok(revents))
	}))
    }

    fn stat(self: Arc<Self>) -> Result<vfs::filesystem::Stat, CanonicalError> {
	unimplemented!()
    }

    fn ioctl(self: Arc<Self>, ioctl: ioctl::IoCtl, arg: u64) -> BoxFuture<'static, Result<u64, CanonicalError>> {
	async move {
	    match ioctl {
		ioctl::IoCtl::TCGETS => {
		    // For now, we'll stub this out
		    Ok(0)
		},
		ioctl::IoCtl::TCSETS => {
		    let termios = memory::copy_value_from_user::<Termios>(VirtAddr::new(arg)).unwrap();

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

		    Ok(0)
		},
		ioctl::IoCtl::TIOCGWINSZ => {
		    let printk = crate::PRINTK.get().expect("Unable to get printk");

		    let read_buf = Bytes::copy_from_slice(&[
			printk.get_rows(),
			printk.get_cols(),
			0, 0]);
		    memory::copy_to_user(VirtAddr::new(arg), read_buf.as_ref()).unwrap();
		    Ok(0)
		},
		ioctl::IoCtl::TIOCGPGRP => {
		    let pgrp = self.pgrp.read();
		    Ok(*pgrp)
		},
		ioctl::IoCtl::TIOCSPGRP => {
		    let mut pgrp = self.pgrp.write();
		    *pgrp = memory::copy_value_from_user::<c_int>(VirtAddr::new(arg)).unwrap() as u64;

		    Ok(0)
		},
	    }
	}.boxed()
    }

    fn seek(&self, _offset: vfs::filesystem::SeekFrom) -> Result<u64, CanonicalError> {
	Err(CanonicalError::SPipe)
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
	read_waker: RwLock::new(None),
	fsi: Mutex::new(vfs::filesystem::FileSystemInstance(0)),
    });
    CONSOLE.call_once(|| device.clone());
    driver::register_devfs(String::from("console"), device);
}

pub fn register_keypress(k: char) {
    CONSOLE.get().expect("Attempted to register keypress before Console subsystem initialised").register_key(k);
}
