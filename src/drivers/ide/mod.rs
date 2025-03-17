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
	let pci_info = if let Some(pci_info) = info.as_any().downcast_ref::<pcie::PciDeviceType>() {
	    pci_info
	} else {
	    return;
	};

	let interface = pci_info.interface;

	// For now, we only support compatibility mode
	if (interface & 1) == 1 {
	    unimplemented!();
	}

	let control_primary_base = 0x3F6;
	let control_secondary_base = 0x376;
	let io_primary_base = 0x1F0;
	let io_secondary_base = 0x170;

	// This device supports bus mastering
	let (busmaster_primary_base, busmaster_secondary_base) = if (interface & 0x80) != 0 {
	    log::info!("Busmastering (DMA) IDE controller found");
	    let bar = pcie::get_bar(pci_info.clone(), 4).expect("Unable to find Busmaster BAR");

	    // Assume I/O space for now. It might not be, but assume it is
	    let busmaster_base = bar.unwrap_io();
	    (Some(busmaster_base), Some(busmaster_base + 0x08))
	} else {
	    (None, None)
	};

	log::info!("Primary IDE Bus:");
	ide::detect_drives(control_primary_base, io_primary_base, busmaster_primary_base);
	log::info!("Secondary IDE Bus:");
	ide::detect_drives(control_secondary_base, io_secondary_base, busmaster_secondary_base);
    }

    fn check_device(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	if let Some(pci_info) = info.as_any().downcast_ref::<pcie::PciDeviceType>() {
	    pci_info.base_class == 1 &&
		pci_info.sub_class == 1
	} else {
	    false
	}
    }

    fn check_new_device(&self, _info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	true // Not yet implemented
    }
}
