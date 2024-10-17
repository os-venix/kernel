use spin::{Once, RwLock};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::boxed::Box;
use alloc::format;
use aml::{AmlName, LevelType, value::{AmlValue, Args}};

use crate::sys::acpi;

#[derive(PartialEq, Eq, Clone)]
pub struct DriverInfo {
    // Identifying information
    pub hid: String,
    pub init: fn(u64, &AmlName, u32),
}

struct DeviceIdentifier {
    pub hid: String,
    pub uid: u32,
    pub path: AmlName,
}

pub trait Bus {
    fn name(&self) -> String;
    fn enumerate(&self) -> Vec<DeviceIdentifier>;
}

#[derive(Copy, Clone)]
pub struct DeviceInfo {
    // Identifying information
    pub driver_id: u64,

    // Tracking information
    pub uid: u32,
    pub is_loaded: bool,
}

static DRIVER_TABLE: Once<RwLock<Vec<DriverInfo>>> = Once::new();
static DEVICE_TABLE: Once<RwLock<Vec<DeviceInfo>>> = Once::new();
static BUS_TABLE: Once<RwLock<Vec<Box<dyn Bus + Send + Sync>>>> = Once::new();

struct SystemBus { }
unsafe impl Send for SystemBus { }
unsafe impl Sync for SystemBus { }
impl Bus for SystemBus {
    fn name(&self) -> String {
	String::from("System Bus")
    }

    fn enumerate(&self) -> Vec<DeviceIdentifier> {
	let mut namespace = {
	    let aml = acpi::AML.get().expect("Attempted to access ACPI tables before ACPI is initialised").read();
	    aml.namespace.clone()
	};

	let mut found_devices = Vec::<DeviceIdentifier>::new();

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

		    found_devices.push(DeviceIdentifier {
			hid: hid,
			uid: uid,
			path: name.clone(),
		    });

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

pub fn register_driver(driver: DriverInfo) {
    let mut driver_table = DRIVER_TABLE.get().expect("Driver table is not yet initialised").write();
    driver_table.push(driver);
}

pub fn register_device(device: DeviceInfo) -> u64 {
    let mut device_tbl = DEVICE_TABLE.get().expect("Attempted to access device table before it is initialised").write();
    device_tbl.push(device);
    (device_tbl.len() - 1) as u64
}

pub fn register_bus_and_enumerate(bus: Box<dyn Bus + Send + Sync>) {
    for found_device in bus.enumerate().iter() {
	let driver_tbl = DRIVER_TABLE.get().expect("Attempted to access driver table before it is initialised").read();

	let driver_id = match driver_tbl.iter()
	    .position(|d| d.hid == found_device.hid) {
		Some(d) => d,
		None => {
		    log::info!("No driver installed for {}", found_device.hid);
		    continue;
		},
	    };
	let device_found = {
	    let device_tbl = DEVICE_TABLE.get().expect("Attempted to access device table before it is initialised").read();
	    device_tbl.iter()
		.any(|d| d.driver_id == driver_id as u64 &&
		     d.uid == found_device.uid &&
		     d.is_loaded)
	};

	if !device_found {
	    log::info!("Found new device {}:{}", found_device.hid, found_device.uid);
	    (driver_tbl[driver_id].init)(driver_id as u64, &found_device.path, found_device.uid);
	}	
    }

    let mut bus_tbl = BUS_TABLE.get().expect("Attempted to access bus table before it is initialised").write();
    bus_tbl.push(bus);
}
