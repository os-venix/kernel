use spin::{Once, RwLock};
use alloc::string::{String, ToString};
use alloc::boxed::Box;
use alloc::collections::btree_map::BTreeMap;

pub trait FileSystem {
    fn read(&self, path: String) -> Result<(*const u8, usize), ()>;
}

static MOUNT_TABLE: Once<RwLock<BTreeMap<String, Box<dyn FileSystem + Send + Sync>>>> = Once::new();

pub fn init() {
    MOUNT_TABLE.call_once(|| RwLock::new(BTreeMap::new()));
}

pub fn mount(mount_point: String, fs: Box<dyn FileSystem + Send + Sync>) {
    let mut mount_table = MOUNT_TABLE.get().expect("Attempted to access mount table before it is initialised").write();
    mount_table.insert(mount_point, fs);
}

pub fn read(file: String) -> Result<(*const u8, usize), ()> {
    let mount_table = MOUNT_TABLE.get().expect("Attempted to access mount table before it is initialised").read();
    for (mount_point, fs) in mount_table.iter() {
	if file.starts_with(mount_point) {
	    return fs.read(
		file.strip_prefix(mount_point)
		    .expect("Attempted to strip off mount point unsuccessfully")
		    .to_string());
	}
    }

    Err(())
}
