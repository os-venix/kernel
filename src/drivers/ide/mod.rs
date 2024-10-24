use alloc::boxed::Box;

use crate::drivers::pcie;
use crate::driver;

mod ide;

pub fn init() {
    let ide_driver = IdeDriver {};
    driver::register_driver(Box::new(ide_driver));
}

pub struct IdeDriver {}
impl driver::Driver for IdeDriver {
    fn init(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) {
	let (address, interface) = if let Some(pci_info) = info.as_any().downcast_ref::<pcie::PciDeviceType>() {
	    (pci_info.address, pci_info.interface)
	} else {
	    return;
	};

	// For now, we only support compatibility mode
	if (interface & 1) == 1 {
	    unimplemented!();
	}

	let control_primary_base = 0x3F6;
	let control_secondary_base = 0x376;
	let io_primary_base = 0x1F0;
	let io_secondary_base = 0x170;

	log::info!("Primary IDE Bus:");
	ide::detect_drives(control_primary_base, io_primary_base);
	log::info!("Secondary IDE Bus:");
	ide::detect_drives(control_secondary_base, io_secondary_base);
    }

    fn check_device(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	if let Some(pci_info) = info.as_any().downcast_ref::<pcie::PciDeviceType>() {
	    pci_info.base_class == 1 &&
		pci_info.sub_class == 1
	} else {
	    false
	}
    }

    fn check_new_device(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	true // Not yet implemented
    }
}
