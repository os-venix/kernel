use core::any::Any;
use core::fmt;
use spin::{Once, RwLock, Mutex};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use bytes;
use futures_util::future::BoxFuture;

use crate::sys::acpi;
use crate::sys::syscall;
use crate::sys::vfs;
use crate::memory;

pub trait Driver {
    fn init(&self, info: &Box<dyn DeviceTypeIdentifier>);
    fn check_device(&self, info: &Box<dyn DeviceTypeIdentifier>) -> bool;
    fn check_new_device(&self, info: &Box<dyn DeviceTypeIdentifier>) -> bool;
}

pub trait DeviceTypeIdentifier: fmt::Display {
    fn as_any(&self) -> &dyn Any;
}

pub trait Bus {
    fn name(&self) -> String;
    fn enumerate(&mut self) -> Vec<Box<dyn DeviceTypeIdentifier>>;
}

pub trait Device {
    fn read(self: Arc<Self>, offset: u64, size: u64, access_restriction: memory::MemoryAccessRestriction) -> BoxFuture<'static, Result<bytes::Bytes, syscall::CanonicalError>>;
    fn write(&self, buf: *const u8, size: u64) -> Result<u64, ()>;
    fn ioctl(&self, ioctl: u64) -> Result<(bytes::Bytes, usize, u64), ()>;
}

struct DevFS {
    file_table: RwLock<BTreeMap<String, u64>>,
}
impl DevFS {
    pub fn new() -> DevFS {
	DevFS {
	    file_table: RwLock::new(BTreeMap::new())
	}
    }

    pub fn add_device(&self, dev_id: u64, mount: String) {
	self.file_table.write().insert(mount, dev_id);
    }
}
impl vfs::FileSystem for DevFS {
    fn read(self: Arc<Self>, path: String, offset: u64, len: u64) -> BoxFuture<'static, Result<bytes::Bytes, syscall::CanonicalError>> {
	// TODO: Set the values here after seeking is supported
	let parts = path.split("/")
	    .filter(|s| s.len() != 0)
	    .collect::<Vec<&str>>();
	if parts.len() != 1 {
	    return Box::pin(async move {
		Err(syscall::CanonicalError::EINVAL)
	    });
	}

	let device_id = {
	    match self.file_table.read().get(parts[0]) {
		Some(id) => id.clone(),
		None => return Box::pin(async move {
		    Err(syscall::CanonicalError::EACCESS)
		}),
	    }
	};

	Box::pin(async move {
	    {
		let device_tbl = DEVICE_TABLE.get()
		    .expect("Attempted to access device table before it is initialised").write();
		let device = device_tbl.get(device_id as usize)
		    .expect("Attempted to access device that does not exist");
		device.clone().read(offset, len, memory::MemoryAccessRestriction::User)
	    }.await
	})
    }
    fn write(&self, path: String, buf: *const u8, len: usize) -> Result<u64, ()> {
	let parts = path.split("/")
	    .filter(|s| s.len() != 0)
	    .collect::<Vec<&str>>();
	if parts.len() != 1 {
	    return Err(());
	}

	let device_id = {
	    match self.file_table.read().get(parts[0]) {
		Some(id) => id.clone(),
		None => return Err(()),
	    }
	};

	let device_tbl = DEVICE_TABLE.get().expect("Attempted to access device table before it is initialised").write();
	let device = device_tbl.get(device_id as usize).expect("Attempted to access device that does not exist");

	device.write(buf, len as u64)
    }
    fn stat(self: Arc<Self>, path: String) -> BoxFuture<'static, Result<vfs::Stat, ()>> {
	let parts = path.split("/")
	    .filter(|s| s.len() != 0)
	    .collect::<Vec<&str>>();
	if parts.len() != 1 {
	    return Box::pin(async move {
		Err(())
	    });
	}

	Box::pin(async move {
	    Ok(vfs::Stat {
		file_name: path,
		size: None,
	    })
	})
    }
    fn ioctl(&self, path: String, ioctl: u64) -> Result<(bytes::Bytes, usize, u64), ()> {
	let parts = path.split("/")
	    .filter(|s| s.len() != 0)
	    .collect::<Vec<&str>>();
	if parts.len() != 1 {
	    return Err(());
	}

	let device_id = {
	    match self.file_table.read().get(parts[0]) {
		Some(id) => id.clone(),
		None => return Err(()),
	    }
	};

	let device_tbl = DEVICE_TABLE.get().expect("Attempted to access device table before it is initialised").write();
	let device = device_tbl.get(device_id as usize).expect("Attempted to access device that does not exist");

	device.ioctl(ioctl)
    }
}

static DRIVER_TABLE: Once<RwLock<Vec<Box<dyn Driver + Send + Sync>>>> = Once::new();
static DEVICE_TABLE: Once<RwLock<Vec<Arc<dyn Device + Send + Sync>>>> = Once::new();
static BUS_TABLE: Once<RwLock<Vec<Arc<Mutex<dyn Bus + Send + Sync>>>>> = Once::new();
static DEVFS: Once<Arc<DevFS>> = Once::new();

pub fn init() {
    DRIVER_TABLE.call_once(|| RwLock::new(Vec::new()));
    DEVICE_TABLE.call_once(|| RwLock::new(Vec::new()));
    BUS_TABLE.call_once(|| RwLock::new(Vec::new()));

    let devfs = Arc::new(DevFS::new());
    DEVFS.call_once(|| devfs.clone());
    vfs::mount(String::from("/dev"), devfs);
}

pub fn configure_drivers() {
    acpi::namespace::enumerate().expect("Could not enumerate ACPI devices");
}

pub fn register_devfs(mount_point: String, dev_id: u64) {
    DEVFS.get().expect("Attempted to access devfs table before it is initialised").add_device(dev_id, mount_point);
}

pub fn register_driver(driver: Box<dyn Driver + Send + Sync>) {
    let mut driver_table = DRIVER_TABLE.get().expect("Driver table is not yet initialised").write();
    driver_table.push(driver);
}

pub fn register_device(device: Arc<dyn Device + Send + Sync>) -> u64 {
    let mut device_tbl = DEVICE_TABLE.get().expect("Attempted to access device table before it is initialised").write();
    device_tbl.push(device);
    (device_tbl.len() - 1) as u64
}

pub fn register_bus_and_enumerate(bus: Arc<Mutex<dyn Bus + Send + Sync>>) {
    let enumerated_bus_devices = {
	let mut locked_bus = bus.lock();
	locked_bus.enumerate()
    };

    for found_device in enumerated_bus_devices.iter() {
	let driver_tbl = DRIVER_TABLE.get().expect("Attempted to access driver table before it is initialised").read();

	let driver = match driver_tbl.iter()
	    .find(|d| d.check_device(found_device) &&
		  d.check_new_device(found_device)) {
		Some(d) => d,
		None => {
//		    log::info!("No driver installed or attempted to init twice for {}", found_device);
		    continue;
		},
	    };

	log::info!("Found new device {}", found_device);
	driver.init(found_device);
    }

    let mut bus_tbl = BUS_TABLE.get().expect("Attempted to access bus table before it is initialised").write();
    bus_tbl.push(bus);
}

pub fn enumerate_device(device_identifier: Box<dyn DeviceTypeIdentifier>) {
    let driver_tbl = DRIVER_TABLE.get().expect("Attempted to access driver table before it is initialised").read();

    let driver = match driver_tbl.iter()
	.find(|d| d.check_device(&device_identifier) &&
	      d.check_new_device(&device_identifier)) {
	    Some(d) => d,
	    None => {
//		log::info!("No driver installed or attempted to init twice for {}", device_identifier);
		return;
	    },
	};

    log::info!("Found new device {}", device_identifier);
    driver.init(&device_identifier);    
}
