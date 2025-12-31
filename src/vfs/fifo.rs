use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::string::String;
use bytes::BytesMut;
use core::cmp;
use core::future::poll_fn;
use core::task::{Context, Poll, Waker};
use futures_util::future::BoxFuture;
use spin::Mutex;
use futures_util::FutureExt;

use crate::sys::ioctl;
use crate::syscall::{CanonicalError, PollEvents};
use crate::vfs::filesystem::{SeekFrom, Stat, VNode, VNodeKind, FileHandle, FileSystemInstance, FileSystem};

pub struct Fifo {
    buffer: Mutex<BytesMut>,
    read_waker: Mutex<Option<Waker>>,
}

impl Fifo {
    pub fn new() -> Self {
	Self {
	    buffer: Mutex::new(BytesMut::new()),
	    read_waker: Mutex::new(None),
	}
    }

    fn read(self: Arc<Self>, len: u64) -> BoxFuture<'static, Result<bytes::Bytes, CanonicalError>> {
	let this = self.clone();

	async move {
	    let mut buffer = this.buffer.lock();

            let to_read = cmp::min(len as usize, buffer.len());
            Ok(buffer.split_to(to_read).freeze())
	}.boxed()
    }

    fn write(self: Arc<Self>, buf: bytes::Bytes) -> BoxFuture<'static, Result<u64, CanonicalError>> {
	let this = self.clone();
	async move {
	    let len = buf.len() as u64;
	    {
		let mut buffer = this.buffer.lock();
		buffer.extend(buf);
	    }

	    {
		let mut read_waker = this.read_waker.lock();
		if let Some(waker) = read_waker.take() {
		    waker.wake();
		    *read_waker = None;
		}
	    }

            Ok(len)
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
		let b = self.buffer.lock();
		let l = (*b).len();

		if l > 0 {
		    revents |= PollEvents::In;
		}
		
		if revents.is_empty() {
		    let mut read_waker = self.read_waker.lock();
		    *read_waker = Some(cx.waker().clone());
		    return Poll::Pending;
		}
	    }

	    Poll::Ready(Ok(revents))
	}))
    }
}

impl VNode for Fifo {
    fn inode(&self) -> u64 {
	0
    }

    fn kind(&self) -> VNodeKind {
	VNodeKind::Fifo
    }

    fn stat(&self) -> Result<Stat, CanonicalError> {
	Ok(Stat {
	    file_name: String::new(),
	    size: Some(self.buffer.lock().len() as u64),
	})
    }

    fn open(self: Arc<Self>) -> Result<Arc<dyn FileHandle>, CanonicalError> {
	Ok(Arc::new(FifoHandle::new(self.clone())))
    }

    fn filesystem(&self) -> Arc<dyn FileSystem> {
	// It's very unclear what goes here
	unimplemented!();
    }

    fn fsi(&self) -> FileSystemInstance {
	unimplemented!();
    }

    fn parent(&self) -> Result<Arc<dyn VNode>, CanonicalError> {
	unimplemented!();
    }

    fn set_fsi(self: Arc<Self>, _fsi: FileSystemInstance) {
	unimplemented!();
    }
}

pub struct FifoHandle {
    fifo: Arc<Fifo>,
}

impl FifoHandle {
    pub fn new(fifo: Arc<Fifo>) -> Self {
	Self {
	    fifo
	}
    }
}

impl FileHandle for FifoHandle {
    fn read(self: Arc<Self>, len: u64) -> BoxFuture<'static, Result<bytes::Bytes, CanonicalError>> {
	// Todo: check for read/write ends here
	self.fifo.clone().read(len)
    }

    fn write(self: Arc<Self>, buf: bytes::Bytes) -> BoxFuture<'static, Result<u64, CanonicalError>> {
	// Todo: check for read/write ends here
	self.fifo.clone().write(buf)
    }

    fn poll(self: Arc<Self>, events: PollEvents) -> BoxFuture<'static, Result<PollEvents, CanonicalError>> {
	// Todo: check for read/write ends here
	self.fifo.clone().poll(events)
    }

    fn stat(self: Arc<Self>) -> Result<Stat, CanonicalError> {
	self.fifo.stat()
    }

    fn ioctl(self: Arc<Self>, _ioctl: ioctl::IoCtl, _arg: u64) -> BoxFuture<'static, Result<u64, CanonicalError>> {
	// TODO: This should return a canonical error
	unimplemented!();
    }

    fn seek(&self, _offset: SeekFrom) -> Result<u64, CanonicalError> {
	// TODO: This should return a canonical error
	unimplemented!();
    }
}
