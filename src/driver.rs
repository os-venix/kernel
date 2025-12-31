use core::any::Any;
use core::fmt;
use spin::{Once, RwLock, Mutex};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use futures_util::future::BoxFuture;
use futures_util::FutureExt;

use crate::sys::acpi;
use crate::sys::syscall::CanonicalError;
use crate::vfs;
use crate::sys::syscall::SyscallResult;

pub trait Driver {
    fn init(&self, info: &dyn DeviceTypeIdentifier);
    fn check_device(&self, info: &dyn DeviceTypeIdentifier) -> bool;
    fn check_new_device(&self, info: &dyn DeviceTypeIdentifier) -> bool;
}

pub trait DeviceTypeIdentifier: fmt::Display {
    fn as_any(&self) -> &dyn Any;
}

#[allow(dead_code)]
pub trait Bus {
    fn name(&self) -> String;
    fn enumerate(&mut self) -> Vec<Box<dyn DeviceTypeIdentifier>>;
}

struct DevFSRootVNode {
    fs: Arc<DevFS>,
    fsi: Mutex<vfs::filesystem::FileSystemInstance>,
}
impl vfs::filesystem::VNode for DevFSRootVNode {
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
	unimplemented!();
    }
    
    fn filesystem(&self) -> Arc<dyn vfs::filesystem::FileSystem> {
	self.fs.clone()
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

pub struct DevFS {
    file_table: RwLock<BTreeMap<String, Arc<dyn vfs::filesystem::VNode>>>,
}
impl DevFS {
    pub fn new() -> DevFS {
	DevFS {
	    file_table: RwLock::new(BTreeMap::new())
	}
    }

    pub fn add_device(&self, vnode: Arc<dyn vfs::filesystem::VNode>, mount: String) {
	self.file_table.write().insert(mount, vnode);
    }
}
impl vfs::filesystem::FileSystem for DevFS {
    fn root(self: Arc<Self>, fsi: vfs::filesystem::FileSystemInstance) -> Arc<dyn vfs::filesystem::VNode> {
	Arc::new(DevFSRootVNode {
	    fs: self.clone(),
	    fsi: Mutex::new(fsi),
	})
    }

    fn lookup(self: Arc<Self>, fsi: vfs::filesystem::FileSystemInstance, _parent: &Arc<dyn vfs::filesystem::VNode>, name: &str) -> BoxFuture<'static, Result<Arc<dyn vfs::filesystem::VNode>, CanonicalError>> {
	let device_vnode = {
	    match self.file_table.read().get(name) {
		Some(vnode) => vnode.clone(),
		None => return async move {
		    Err(CanonicalError::Access)
		}.boxed(),
	    }
	};

	device_vnode.clone().set_fsi(fsi);

	async move {
	    Ok(device_vnode)
	}.boxed()
    }
}

static DRIVER_TABLE: Once<RwLock<Vec<Box<dyn Driver + Send + Sync>>>> = Once::new();
static BUS_TABLE: Once<RwLock<Vec<Arc<Mutex<dyn Bus + Send + Sync>>>>> = Once::new();
static DEVFS: Once<Arc<DevFS>> = Once::new();

pub fn init() {
    DRIVER_TABLE.call_once(|| RwLock::new(Vec::new()));
    BUS_TABLE.call_once(|| RwLock::new(Vec::new()));

    let devfs = Arc::new(DevFS::new());
    DEVFS.call_once(|| devfs.clone());
}

pub async fn mount_devfs() -> SyscallResult {
    let devfs = DEVFS.get().unwrap();
    syscall_try!(vfs::mount("/dev", devfs.clone()).await);
    syscall_success!(0)
}

pub fn configure_drivers() {
    acpi::namespace::enumerate().expect("Could not enumerate ACPI devices");
}

pub fn register_devfs(mount_point: String, vnode: Arc<dyn vfs::filesystem::VNode>) {
    DEVFS.get().expect("Attempted to access devfs table before it is initialised").add_device(vnode, mount_point);
}

pub fn get_devfs() -> Arc<DevFS> {
    DEVFS.get().expect("Attempted to access devfs before it is initialised").clone()
}

pub fn register_driver(driver: Box<dyn Driver + Send + Sync>) {
    let mut driver_table = DRIVER_TABLE.get().expect("Driver table is not yet initialised").write();
    driver_table.push(driver);
}

pub fn register_bus_and_enumerate(bus: Arc<Mutex<dyn Bus + Send + Sync>>) {
    let enumerated_bus_devices = {
	let mut locked_bus = bus.lock();
	locked_bus.enumerate()
    };

    for found_device in enumerated_bus_devices.iter() {
	let driver_tbl = DRIVER_TABLE.get().expect("Attempted to access driver table before it is initialised").read();

	let driver = match driver_tbl.iter()
	    .find(|d| d.check_device(found_device.as_ref()) &&
		  d.check_new_device(found_device.as_ref())) {
		Some(d) => d,
		None => {
//		    log::info!("No driver installed or attempted to init twice for {}", found_device);
		    continue;
		},
	    };

	log::info!("Found new device {}", found_device);
	driver.init(found_device.as_ref());
    }

    let mut bus_tbl = BUS_TABLE.get().expect("Attempted to access bus table before it is initialised").write();
    bus_tbl.push(bus);
}

pub fn enumerate_device(device_identifier: Box<dyn DeviceTypeIdentifier>) {
    let driver_tbl = DRIVER_TABLE.get().expect("Attempted to access driver table before it is initialised").read();

    let driver = match driver_tbl.iter()
	.find(|d| d.check_device(device_identifier.as_ref()) &&
	      d.check_new_device(device_identifier.as_ref())) {
	    Some(d) => d,
	    None => {
//		log::info!("No driver installed or attempted to init twice for {}", device_identifier);
		return;
	    },
	};

    log::info!("Found new device {}", device_identifier);
    driver.init(device_identifier.as_ref());
}
