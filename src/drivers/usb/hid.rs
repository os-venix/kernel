use alloc::boxed::Box;

use crate::driver;
use crate::drivers::usb::usb;

pub fn init() {
    let usb_hid_driver = HidDriver {};
    driver::register_driver(Box::new(usb_hid_driver));
}

pub struct HidDriver {}
impl driver::Driver for HidDriver {
    fn init(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) {
	log::info!("Initialising HID device");

	if let Some(usb_info) = info.as_any().downcast_ref::<usb::UsbDevice>() {
	    if usb_info.interface_descriptors.len() != 1 {
		unimplemented!();
	    }

	    if usb_info.interface_descriptors[0].subclass == 1 {
		log::info!("  Boot protocol");
	    } else {
		log::info!("  Report protocol");
	    }

	    if usb_info.interface_descriptors[0].protocol == 1 {
		log::info!("  Keyboard");
	    } else if usb_info.interface_descriptors[0].protocol == 2 {
		log::info!("  Mouse");
	    }
	}
    }

    fn check_device(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	if let Some(usb_info) = info.as_any().downcast_ref::<usb::UsbDevice>() {
	    if usb_info.interface_descriptors.len() != 1 {
		unimplemented!();
	    }

	    usb_info.interface_descriptors[0].class == 3
	} else {
	    false
	}
    }

    fn check_new_device(&self, _info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	true // Not yet implemented
    }
}
