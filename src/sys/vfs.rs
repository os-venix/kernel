use spin::{Once, RwLock};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::collections::btree_map::BTreeMap;
use anyhow::{anyhow, Result};
use alloc::slice;
use bytes;
use futures_util::future::BoxFuture;

use crate::sys::syscall;

const SEEK_SET: u64 = 3;
const SEEK_CUR: u64 = 1;
const SEEK_END: u64 = 2;

pub struct Stat {
    pub file_name: String,
    pub size: Option<u64>,
}

pub trait FileSystem {
    fn read(self: Arc<Self>, path: String, offset: u64, len: u64) -> BoxFuture<'static, Result<bytes::Bytes, syscall::CanonicalError>>;
    fn write(&self, path: String, buf: *const u8, len: usize) -> Result<u64, ()>;
    fn stat(self: Arc<Self>, path: String) -> BoxFuture<'static, Result<Stat, ()>>;
    fn ioctl(&self, path: String, ioctl: u64) -> Result<(bytes::Bytes, usize, u64), ()>;
}

#[derive(Debug)]
pub struct FileDescriptor {
    file_name: String,
    current_offset: u64,
}

impl FileDescriptor {
    pub fn new(name: String) -> FileDescriptor {
	FileDescriptor {
	    file_name: name,
	    current_offset: 0,
	}
    }

    pub fn get_file_name(&self) -> String {
	self.file_name.clone()
    }
}

static MOUNT_TABLE: Once<RwLock<BTreeMap<String, Arc<dyn FileSystem + Send + Sync>>>> = Once::new();

pub fn init() {
    MOUNT_TABLE.call_once(|| RwLock::new(BTreeMap::new()));
}

pub fn mount(mount_point: String, fs: Arc<dyn FileSystem + Send + Sync>) {
    let mut mount_table = MOUNT_TABLE.get().expect("Attempted to access mount table before it is initialised").write();
    mount_table.insert(mount_point, fs);
}

fn get_mount_point(path: &String) -> Result<(Arc<dyn FileSystem + Send + Sync>, String), syscall::CanonicalError> {
    let mut fs: Option<Arc<dyn FileSystem + Send + Sync>> = None;
    let mut file_name: String = String::new();
    let mut current_mount_point = String::new();

    let mount_table = MOUNT_TABLE.get().expect("Attempted to access mount table before it is initialised").read();
    for (mount_point, filesystem) in mount_table.iter() {
	if path.starts_with(mount_point) && mount_point.len() > current_mount_point.len() {
	    fs = Some(filesystem.clone());
	    file_name = path.strip_prefix(mount_point)
		.expect("Attempted to strip off mount point unsuccessfully")
		.to_string();
	    current_mount_point = mount_point.clone();
	}
    }

    if let Some(filesystem) = fs {
	Ok((filesystem, file_name))
    } else {
	Err(syscall::CanonicalError::ENOENT)
    }
}

pub async fn read(file: String, offset: u64, len: u64) -> Result<bytes::Bytes, syscall::CanonicalError> {
    let (fs, file_name) = get_mount_point(&file)?;
    fs.read(file_name.clone(), offset, len).await
}

pub fn write(file: String, buf: *const u8, len: usize) -> Result<u64> {
    let (fs, file_name) = get_mount_point(&file)?;

    {
	return match fs.write(file_name, buf, len) {
	    Ok(l) => Ok(l),
	    Err(_) => Err(anyhow!("Unable to write {}", file)),
	};
    }
}

pub async fn stat(file: String) -> Result<Stat, syscall::CanonicalError> {
    let (fs, file_name) = get_mount_point(&file)?;

    {
	return match fs.stat(file_name).await {
	    Ok(l) => Ok(l),
	    Err(_) => panic!("Unable to stat {}", file),
	};
    }
}

pub fn ioctl(file: String, ioctl: u64) -> Result<(bytes::Bytes, usize, u64)> {
    let (fs, file_name) = get_mount_point(&file)?;

    {
	return match fs.ioctl(file_name, ioctl) {
	    Ok(l) => Ok(l),
	    Err(_) => Err(anyhow!("Unable to write {}", file)),
	};
    }
}

pub fn write_by_fd(fd: Arc<RwLock<FileDescriptor>>, buf: u64, len: u64) -> Result<u64> {
    let r = fd.read();
    let file = r.get_file_name();
    write(file, buf as *const u8, len as usize)
}

pub async fn read_by_fd(fd: Arc<RwLock<FileDescriptor>>, buf: u64, len: u64) -> Result<u64, syscall::CanonicalError> {
    let read_buffer = {
	let w = fd.read();
	let file = w.get_file_name();

	read(file, w.current_offset, len)
    }.await?;

    let data_to = unsafe {
	slice::from_raw_parts_mut(buf as *mut u8, read_buffer.len())
    };
    data_to.copy_from_slice(read_buffer.as_ref());

    {
	let mut w = fd.write();
	w.current_offset += len;
    }
    Ok(read_buffer.len() as u64)
}

pub fn ioctl_by_fd(fd: Arc<RwLock<FileDescriptor>>, ioctl_num: u64, buf: u64) -> Result<u64> {
    let r = fd.read();
    let file = r.get_file_name();

    let (read_buf, size, ret) = ioctl(file, ioctl_num)?;

    let data_to = unsafe {
	slice::from_raw_parts_mut(buf as *mut u8, size as usize)
    };
    data_to.copy_from_slice(read_buf.as_ref());
    Ok(ret)
}

pub async fn seek_fd(fd: Arc<RwLock<FileDescriptor>>, offset: u64, whence: u64) -> Result<u64> {
    let offset_signed = offset as i64;

    let stat = {
	let w = fd.read();
	let file = w.get_file_name();

	stat(file)
    }.await?;
    let size = if let Some(s) = stat.size { s } else {
	return Ok(0);
    };

    let mut w = fd.write();
    if whence == SEEK_SET {
	if offset >= size || offset_signed < 0 {
	    return Err(anyhow!("Invalid size"));
	}

	w.current_offset = offset_signed as u64;
    } else if whence == SEEK_CUR {
	let (result, overflow) = w.current_offset.overflowing_add_signed(offset_signed);
	if result >= size || overflow {
	    return Err(anyhow!("Invalid size"));
	}

	w.current_offset = w.current_offset.wrapping_add_signed(offset_signed);
    } else if whence == SEEK_END {
	let (result, overflow) = size.overflowing_add_signed(offset_signed);
	if result >= size || overflow {
	    return Err(anyhow!("Invalid size"));
	}

	w.current_offset = size.wrapping_add_signed(offset_signed);
    }

    Ok(w.current_offset)
}
