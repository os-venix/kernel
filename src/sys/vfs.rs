use spin::{Once, RwLock};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::collections::btree_map::BTreeMap;
use alloc::vec::Vec;
use anyhow::{anyhow, Result};

pub trait FileSystem {
    fn read(&self, path: String) -> Result<(*const u8, usize), ()>;
    fn write(&self, path: String, buf: *const u8, len: usize) -> Result<u64, ()>;
}

pub struct FileDescriptor {
    file_name: String,
}

impl FileDescriptor {
    pub fn new(name: String) -> FileDescriptor {
	FileDescriptor {
	    file_name: name,
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

pub fn write_by_fd(fd: Arc<FileDescriptor>, buf: u64, len: u64) -> Result<u64> {
    let file = fd.get_file_name();
    write(file, buf as *const u8, len as usize)
}
