use core::any::Any;
use core::fmt;
use spin::{Once, RwLock};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::boxed::Box;
use alloc::format;

use aml::{AmlName, LevelType, value::{AmlValue, Args}};

use crate::sys::acpi;
use crate::memory;

pub trait Driver {
    fn init(&self, info: &Box<dyn DeviceTypeIdentifier>);
    fn check_device(&self, info: &Box<dyn DeviceTypeIdentifier>) -> bool;
    fn check_new_device(&self, info: &Box<dyn DeviceTypeIdentifier>) -> bool;
}

pub trait DeviceTypeIdentifier: fmt::Display {
    fn as_any(&self) -> &dyn Any;
}

#[derive(PartialEq, Eq, Clone)]
pub struct SystemBusDeviceIdentifier {
    pub hid: String,
    pub uid: u32,
    pub path: AmlName,
}

impl DeviceTypeIdentifier for SystemBusDeviceIdentifier {
    fn as_any(&self) -> &dyn Any {
	self
    }
}

impl fmt::Display for SystemBusDeviceIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
	write!(f, "{}/{}:{}", self.path.as_string(), self.hid, self.uid)
    }
}

pub trait Bus {
    fn name(&self) -> String;
    fn enumerate(&self) -> Vec<Box<dyn DeviceTypeIdentifier>>;
}

pub trait Device {
    fn read(&self, offset: u64, size: u64, access_restriction: memory::MemoryAccessRestriction) -> Result<*const u8, ()>;
}

static DRIVER_TABLE: Once<RwLock<Vec<Box<dyn Driver + Send + Sync>>>> = Once::new();
static DEVICE_TABLE: Once<RwLock<Vec<Arc<dyn Device + Send + Sync>>>> = Once::new();
static BUS_TABLE: Once<RwLock<Vec<Box<dyn Bus + Send + Sync>>>> = Once::new();

struct SystemBus { }
unsafe impl Send for SystemBus { }
unsafe impl Sync for SystemBus { }
impl Bus for SystemBus {
    fn name(&self) -> String {
	String::from("System Bus")
    }

    fn enumerate(&self) -> Vec<Box<dyn DeviceTypeIdentifier>> {
	let mut namespace = {
	    let aml = acpi::AML.get().expect("Attempted to access ACPI tables before ACPI is initialised").read();
	    aml.namespace.clone()
	};

	let mut found_devices = Vec::<Box<dyn DeviceTypeIdentifier>>::new();

	namespace.traverse(|name, ns_lvl| {
	    match ns_lvl.typ {
		LevelType::Scope => Ok(true),
		LevelType::Processor => Ok(false),
		LevelType::PowerResource => Ok(false),
		LevelType::ThermalZone => Ok(false),
		LevelType::MethodLocals => Ok(false),
		LevelType::Device => {
		    let hid_path = name.as_string() + "._HID";
		    let hid = {
			let mut aml = acpi::AML.get().expect("Attempted to access ACPI tables before ACPI is initialised").write();
			match aml.invoke_method(
			    &AmlName::from_str(&hid_path).expect(&format!("Unable to construct AmlName {}", &hid_path)),
			    Args::EMPTY,
			) {
			    Ok(AmlValue::String(s)) => s,
			    Ok(AmlValue::Integer(eisa_id)) => acpi::eisa_id_to_string(eisa_id),
			    Err(_) => return Ok(true),
			    _ => { panic!("Malformed _HID for device {}", hid_path) },
			}
		    };

		    let uid_path = name.as_string() + "._UID";
		    let uid = {
			let mut aml = acpi::AML.get().expect("Attempted to access ACPI tables before ACPI is initialised").write();
			match aml.invoke_method(
			    &AmlName::from_str(&uid_path).expect(&format!("Unable to construct AmlName {}", &uid_path)),
			    Args::EMPTY,
			) {
			    Ok(AmlValue::Integer(v)) => v as u32,
			    _ => 0,
			}
		    };

		    found_devices.push(Box::new(SystemBusDeviceIdentifier {
			hid: hid,
			uid: uid,
			path: name.clone(),
		    }));

		    Ok(true)
		},
	    }
	}).expect("Driver configuration failed.");

	found_devices
    }
}

pub fn init() {
    DRIVER_TABLE.call_once(|| RwLock::new(Vec::new()));
    DEVICE_TABLE.call_once(|| RwLock::new(Vec::new()));
    BUS_TABLE.call_once(|| RwLock::new(Vec::new()));
}

pub fn configure_drivers() {    
    register_bus_and_enumerate(Box::new(SystemBus { }));
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

pub fn register_bus_and_enumerate(bus: Box<dyn Bus + Send + Sync>) {
    for found_device in bus.enumerate().iter() {
	let driver_tbl = DRIVER_TABLE.get().expect("Attempted to access driver table before it is initialised").read();

	let driver = match driver_tbl.iter()
	    .find(|d| d.check_device(found_device) &&
		  d.check_new_device(found_device)) {
		Some(d) => d,
		None => {
		    log::info!("No driver installed or attempted to init twice for {}", found_device);
		    continue;
		},
	    };

	log::info!("Found new device {}", found_device);
	driver.init(found_device);
    }

    let mut bus_tbl = BUS_TABLE.get().expect("Attempted to access bus table before it is initialised").write();
    bus_tbl.push(bus);
}

pub fn read(device_id: u64, offset: u64, size: u64, access_restriction: memory::MemoryAccessRestriction) -> Result<*const u8, ()> {
    let device_tbl = DEVICE_TABLE.get().expect("Attempted to access device table before it is initialised").write();
    let device = device_tbl.get(device_id as usize).expect("Attempted to access device that does not exist");

    device.read(offset, size, access_restriction)
}
