use crate::driver;
use aml::{AmlName, AmlValue, value::Args, resource::resource_descriptor_list};
use alloc::string::String;
use alloc::format;

use crate::sys::acpi;

pub fn init() {
    let hpet_driver = driver::DriverInfo {
	hid: String::from("PNP0103"),
	init: init_driver,
    };
    driver::register_driver(hpet_driver);
}

fn init_driver<'a>(driver: &'a driver::DriverInfo, acpi_device: &AmlName, uid: u32) -> driver::DeviceInfo<'a> {
    let crs_path = acpi_device.as_string() + "._CRS";
    let crs = {
	let mut aml = acpi::AML.get().expect("Attempted to access ACPI tables before ACPI is initialised").write();
	match aml.invoke_method(
	    &AmlName::from_str(&crs_path).expect(&format!("Unable to construct AmlName {}", &crs_path)),
	    Args::EMPTY,
	) {
	    Ok(AmlValue::Buffer(v)) => AmlValue::Buffer(v),
	    _ => panic!("CRS expected for HPET"),
	}
    };

    let resources = match resource_descriptor_list(&crs) {
	Ok(v) => v,
	Err(e) => panic!("Malformed CRS for HPET: {:#?}", e),
    };

    log::info!("{:#?}", resources);

    driver::DeviceInfo {
	driver: driver,
	uid: uid,
	is_loaded: true
    }
}
