use spin::{Once, RwLock};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::collections::btree_map::BTreeMap;
use anyhow::{anyhow, Result};
use alloc::slice;
use bytes;
use core::cmp;
use futures_util::future::BoxFuture;
use alloc::vec::Vec;

use crate::sys::syscall;
use crate::sys::ioctl;

const SEEK_SET: u64 = 3;
const SEEK_CUR: u64 = 1;
const SEEK_END: u64 = 2;

#[allow(dead_code)]
pub struct Stat {
    pub file_name: String,
    pub size: Option<u64>,
}

pub trait FileSystem {
    fn read(self: Arc<Self>, path: String, offset: u64, len: u64) -> BoxFuture<'static, Result<bytes::Bytes, syscall::CanonicalError>>;
    fn write(&self, path: String, buf: *const u8, len: usize) -> Result<u64, ()>;
    fn stat(self: Arc<Self>, path: String) -> BoxFuture<'static, Result<Stat, ()>>;
    fn ioctl(&self, path: String, ioctl: ioctl::IoCtl, buf: u64) -> Result<u64, ()>;
}

pub enum FileDescriptor {
    File {
	file_name: String,
	current_offset: u64,

	file_system: Arc<dyn FileSystem + Send + Sync>,
	local_name: String,
    },
    Pipe {
	buffer: bytes::BytesMut,
    },
}

impl FileDescriptor {
    pub fn new(name: String) -> FileDescriptor {
	let (fs, local_name) = get_mount_point(&name).unwrap();
	FileDescriptor::File {
	    file_name: name,
	    current_offset: 0,
	    file_system: fs,
	    local_name,
	}
    }

    pub fn new_pipe() -> FileDescriptor {
	FileDescriptor::Pipe {
	    buffer: bytes::BytesMut::new()
	}
    }

    pub async fn read(&mut self, len: u64) -> Result<bytes::Bytes, syscall::CanonicalError> {
	match self {
	    FileDescriptor::File { file_name: _, current_offset, file_system, local_name } => {
		let read_buffer = file_system.clone().read(local_name.clone(), *current_offset, len).await?;
		*current_offset += len;
		Ok(read_buffer)
	    },
	    FileDescriptor::Pipe { buffer } => {
		let to_read = cmp::min(len as usize, buffer.len());
		Ok(buffer.split_to(to_read).freeze())
	    },
	}
    }

    pub fn write(&mut self, buf: Vec<u8>, len: u64) -> Result<u64> {
	match self {
	    FileDescriptor::File { file_name, current_offset: _, file_system, local_name } => {
		match file_system.write(local_name.clone(), buf.as_ptr(), len as usize) {
		    Ok(l) => Ok(l),
		    Err(_) => Err(anyhow!("Unable to write {}", file_name)),
		}
	    },
	    FileDescriptor::Pipe { buffer } => {
		let user_buf = unsafe {
		    slice::from_raw_parts(buf.as_ptr(), len as usize)
		};
		buffer.extend_from_slice(user_buf);
		Ok(len)
	    },
	}
    }

    pub fn ioctl(&self, operation: ioctl::IoCtl, buf: u64) -> Result<u64> {
	match self {
	    FileDescriptor::File { file_name, current_offset: _, file_system, local_name } => {
		match file_system.ioctl(local_name.clone(), operation, buf) {
		    Ok(l) => Ok(l),
		    Err(_) => Err(anyhow!("Unable to ioctl {}", file_name)),
		}
	    },
	    _ => Err(anyhow!("Unable to ioctl")),
	}
    }

    pub async fn seek(&mut self, offset: u64, whence: u64) -> Result<u64> {
	match self {
	    FileDescriptor::File { file_name, current_offset, file_system: _, local_name: _ } => {
		let offset_signed = offset as i64;

		let stat = stat(file_name.clone()).await?;
		let size = if let Some(s) = stat.size { s } else {
		    return Ok(0);
		};

		if whence == SEEK_SET {
		    if offset >= size || offset_signed < 0 {
			return Err(anyhow!("Invalid size"));
		    }

		    *current_offset = offset_signed as u64;
		} else if whence == SEEK_CUR {
		    let (result, overflow) = current_offset.overflowing_add_signed(offset_signed);
		    if result >= size || overflow {
			return Err(anyhow!("Invalid size"));
		    }

		    *current_offset = current_offset.wrapping_add_signed(offset_signed);
		} else if whence == SEEK_END {
		    let (result, overflow) = size.overflowing_add_signed(offset_signed);
		    if result >= size || overflow {
			return Err(anyhow!("Invalid size"));
		    }

		    *current_offset = size.wrapping_add_signed(offset_signed);
		}

		Ok(*current_offset)
	    },
	    _ => Err(anyhow!("Unable to seek")),
	}
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

pub async fn stat(file: String) -> Result<Stat, syscall::CanonicalError> {
    let (fs, file_name) = get_mount_point(&file)?;

    return match fs.stat(file_name).await {
	Ok(l) => Ok(l),
	Err(_) => Err(syscall::CanonicalError::ENOENT),
    };
}
