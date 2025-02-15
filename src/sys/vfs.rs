use spin::{Once, RwLock};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::collections::btree_map::BTreeMap;
use anyhow::{anyhow, Result};
use alloc::slice;

const SEEK_SET: u64 = 3;
const SEEK_CUR: u64 = 1;
const SEEK_END: u64 = 2;

pub struct Stat {
    pub file_name: String,
    pub size: u64,
}

pub trait FileSystem {
    fn read(&self, path: String) -> Result<(*const u8, usize), ()>;
    fn write(&self, path: String, buf: *const u8, len: usize) -> Result<u64, ()>;
    fn stat(&self, path: String) -> Result<Stat, ()>;
}

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

static MOUNT_TABLE: Once<RwLock<BTreeMap<String, Arc<RwLock<dyn FileSystem + Send + Sync>>>>> = Once::new();

pub fn init() {
    MOUNT_TABLE.call_once(|| RwLock::new(BTreeMap::new()));
}

pub fn mount(mount_point: String, fs: Arc<RwLock<dyn FileSystem + Send + Sync>>) {
    let mut mount_table = MOUNT_TABLE.get().expect("Attempted to access mount table before it is initialised").write();
    mount_table.insert(mount_point, fs);
}

pub fn read(file: String) -> Result<(*const u8, usize)> {
    let (fs, file_name) = {
	let mut fs: Option<Arc<RwLock<dyn FileSystem + Send + Sync>>> = None;
	let mut file_name: String = String::new();
	let mount_table = MOUNT_TABLE.get().expect("Attempted to access mount table before it is initialised").read();
	for (mount_point, filesystem) in mount_table.iter() {
	    if file.starts_with(mount_point) {
		fs = Some(filesystem.clone());
		file_name = file.strip_prefix(mount_point)
		    .expect("Attempted to strip off mount point unsuccessfully")
		    .to_string();
		break;
	    }
	}

	match fs {
	    Some(filesystem) => (filesystem, file_name),
	    None => return Err(anyhow!("No mount point found for {}", file)),
	}
    };

    {
	let fs_to = fs.read();
	return match fs_to.read(file_name) {
	    Ok(f) => Ok(f),
	    Err(_) => Err(anyhow!("Unable to load {}", file)),
	};
    }
}

pub fn write(file: String, buf: *const u8, len: usize) -> Result<u64> {
    let (fs, file_name) = {
	let mut fs: Option<Arc<RwLock<dyn FileSystem + Send + Sync>>> = None;
	let mut file_name: String = String::new();
	let mut current_mount_point = String::new();

	let mount_table = MOUNT_TABLE.get().expect("Attempted to access mount table before it is initialised").read();
	for (mount_point, filesystem) in mount_table.iter() {
	    if file.starts_with(mount_point) && mount_point.len() > current_mount_point.len() {
		fs = Some(filesystem.clone());
		file_name = file.strip_prefix(mount_point)
		    .expect("Attempted to strip off mount point unsuccessfully")
		    .to_string();
		current_mount_point = mount_point.clone();
	    }
	}

	match fs {
	    Some(filesystem) => (filesystem, file_name),
	    None => return Err(anyhow!("No mount point found for {}", file)),
	}
    };

    {
	let fs_to = fs.read();
	return match fs_to.write(file_name, buf, len) {
	    Ok(l) => Ok(l),
	    Err(_) => Err(anyhow!("Unable to write {}", file)),
	};
    }
}

pub fn stat(file: String) -> Result<Stat> {
    let (fs, file_name) = {
	let mut fs: Option<Arc<RwLock<dyn FileSystem + Send + Sync>>> = None;
	let mut file_name: String = String::new();
	let mut current_mount_point = String::new();

	let mount_table = MOUNT_TABLE.get().expect("Attempted to access mount table before it is initialised").read();
	for (mount_point, filesystem) in mount_table.iter() {
	    if file.starts_with(mount_point) && mount_point.len() > current_mount_point.len() {
		fs = Some(filesystem.clone());
		file_name = file.strip_prefix(mount_point)
		    .expect("Attempted to strip off mount point unsuccessfully")
		    .to_string();
		current_mount_point = mount_point.clone();
	    }
	}

	match fs {
	    Some(filesystem) => (filesystem, file_name),
	    None => return Err(anyhow!("No mount point found for {}", file)),
	}
    };

    {
	let fs_to = fs.read();
	return match fs_to.stat(file_name) {
	    Ok(l) => Ok(l),
	    Err(_) => Err(anyhow!("Unable to stat {}", file)),
	};
    }
}

pub fn write_by_fd(fd: Arc<RwLock<FileDescriptor>>, buf: u64, len: u64) -> Result<u64> {
    let r = fd.read();
    let file = r.get_file_name();
    write(file, buf as *const u8, len as usize)
}

pub fn read_by_fd(fd: Arc<RwLock<FileDescriptor>>, buf: u64, len: u64) -> Result<u64> {
    let mut w = fd.write();
    let file = w.get_file_name();

    let (read_buf, size) = read(file)?;
    if (size as u64) < w.current_offset + len {
	return Err(anyhow!("Requested more data than file contains"))
    }

    let data_from = unsafe {
	slice::from_raw_parts(read_buf.wrapping_add(w.current_offset as usize), len as usize)
    };
    let data_to = unsafe {
	slice::from_raw_parts_mut(buf as *mut u8, len as usize)
    };
    data_to.copy_from_slice(data_from);

    w.current_offset += len;
    Ok(len)
}

pub fn seek_fd(fd: Arc<RwLock<FileDescriptor>>, offset: u64, whence: u64) -> Result<u64> {
    let offset_signed = offset as i64;

    let mut w = fd.write();
    let file = w.get_file_name();

    let stat = stat(file)?;

    if whence == SEEK_SET {
	if offset >= stat.size || offset_signed < 0 {
	    return Err(anyhow!("Invalid size"));
	}

	w.current_offset = offset_signed as u64;
    } else if whence == SEEK_CUR {
	let (result, overflow) = w.current_offset.overflowing_add_signed(offset_signed);
	if result >= stat.size || overflow {
	    return Err(anyhow!("Invalid size"));
	}

	w.current_offset = w.current_offset.wrapping_add_signed(offset_signed);
    } else if whence == SEEK_END {
	let (result, overflow) = stat.size.overflowing_add_signed(offset_signed);
	if result >= stat.size || overflow {
	    return Err(anyhow!("Invalid size"));
	}

	w.current_offset = stat.size.wrapping_add_signed(offset_signed);
    }

    Ok(w.current_offset)
}
