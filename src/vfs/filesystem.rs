use alloc::sync::Arc;
use alloc::string::String;
use futures_util::future::BoxFuture;

use crate::sys::syscall::{CanonicalError, PollEvents};
use crate::sys::ioctl;

#[allow(dead_code)]
pub struct Stat {
    pub file_name: String,
    pub size: Option<u64>,
}

#[derive(Copy, Clone, Ord, PartialOrd, PartialEq, Eq)]
pub struct FileSystemInstance(pub u64);

pub enum SeekFrom {
    Set(i64),
    Cur(i64),
    End(i64),
}

pub trait FileSystem: Send + Sync {
    fn root(self: Arc<Self>, fsi: FileSystemInstance) -> Arc<dyn VNode>;

    fn lookup(self: Arc<Self>, fsi: FileSystemInstance, parent: &Arc<dyn VNode>, name: &str) -> BoxFuture<'static, Result<Arc<dyn VNode>, CanonicalError>>;
}

pub trait VNode: Send + Sync {
    fn inode(&self) -> u64;
    fn kind(&self) -> VNodeKind;

    fn stat(&self) -> Result<Stat, CanonicalError>;

    fn open(self: Arc<Self>/*, flags: OpenFlags */) -> Result<Arc<dyn FileHandle>, CanonicalError>;

    fn filesystem(&self) -> Arc<dyn FileSystem>;
    fn fsi(&self) -> FileSystemInstance;

    fn parent(&self) -> Result<Arc<dyn VNode>, CanonicalError>;

    // For use with procedurally generated filesystems
    fn set_fsi(self: Arc<Self>, fsi: FileSystemInstance);
}

#[allow(dead_code)]
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum VNodeKind {
    Regular,
    Directory,
    Symlink,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
}

pub trait FileHandle: Send + Sync {
    fn read(self: Arc<Self>, len: u64) -> BoxFuture<'static, Result<bytes::Bytes, CanonicalError>>;
    fn write(self: Arc<Self>, buf: bytes::Bytes) -> BoxFuture<'static, Result<u64, CanonicalError>>;
    fn poll(self: Arc<Self>, events: PollEvents) -> BoxFuture<'static, Result<PollEvents, CanonicalError>>;

    fn stat(self: Arc<Self>) -> Result<Stat, CanonicalError>;
    fn ioctl(self: Arc<Self>, ioctl: ioctl::IoCtl, arg: u64) -> BoxFuture<'static, Result<u64, CanonicalError>>;
    fn seek(&self, offset: SeekFrom) -> Result<u64, CanonicalError>;
}
