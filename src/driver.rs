use spin::{Once, RwLock};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use aml::{AmlName, LevelType, value::{AmlValue, Args}};

use crate::sys::acpi;

#[derive(PartialEq, Eq)]
pub struct DriverInfo {
    // Identifying information
    pub hid: String,
    pub init: for<'a> fn(&'a DriverInfo, &AmlName, u32) -> DeviceInfo<'a>,
}

pub struct DeviceInfo<'a> {
    // Identifying information
    pub driver: &'a DriverInfo,

    // Tracking information
    pub uid: u32,
    pub is_loaded: bool,
}

static DRIVER_TABLE: Once<RwLock<Vec<DriverInfo>>> = Once::new();
static DEVICE_TABLE: Once<RwLock<Vec<DeviceInfo>>> = Once::new();

pub fn init() {
    DRIVER_TABLE.call_once(|| RwLock::new(Vec::new()));
    DEVICE_TABLE.call_once(|| RwLock::new(Vec::new()));
}

pub fn register_driver(driver: DriverInfo) {
    let mut driver_table = DRIVER_TABLE.get().expect("Driver table is not yet initialised").write();
    driver_table.push(driver);
}

pub fn configure_drivers() {
    let mut namespace = {
	let aml = acpi::AML.get().expect("Attempted to access ACPI tables before ACPI is initialised").read();
	aml.namespace.clone()
    };

    namespace.traverse(|name, ns_lvl| {
	match ns_lvl.typ {
	    LevelType::Scope => Ok(true),
	    LevelType::Processor => Ok(false),
	    LevelType::PowerResource => Ok(false),
	    LevelType::ThermalZone => Ok(false),
	    LevelType::MethodLocals => Ok(false),
	    LevelType::Device => {
		let driver_tbl = DRIVER_TABLE.get().expect("Attempted to access driver table before it is initialised").read();

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

		let driver = match driver_tbl.iter()
		    .find(|d| d.hid == hid) {
			Some(d) => d,
			None => {
			    log::info!("No driver installed for {}", hid);
			    return Ok(true);
			},
		    };
		let device_found = {
		    let device_tbl = DEVICE_TABLE.get().expect("Attempted to access device table before it is initialised").read();
		    device_tbl.iter()
			.any(|d| d.driver == driver &&
			     d.uid == uid &&
			     d.is_loaded)
		};

		if !device_found {
		    log::info!("Found new device {}:{}", hid, uid);
		    (driver.init)(&driver, name, uid);
		}

		Ok(true)
	    },
	}
    }).expect("Driver configuration failed.");
}
