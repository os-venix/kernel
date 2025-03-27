use alloc::boxed::Box;
use alloc::vec::Vec;
use nom::{
    IResult,
    Parser,
    multi::many_m_n,
    number::{
	complete::{u8, u16},
	Endianness,
    },
};

use crate::driver;
use crate::drivers::usb::usb;

#[derive(Debug)]
struct HidDescriptorDescriptor {
    pub descriptor_type: u8,
    pub length: u16,
}

#[derive(Debug)]
struct HidDescriptor {
    pub version: u16,
    pub country_code: u8,
    pub descriptors: Vec<HidDescriptorDescriptor>,
}

fn parse_hid_descriptor_descriptor(input: &[u8]) -> IResult<&[u8], HidDescriptorDescriptor> {
    let (input, descriptor_type) = u8(input)?;
    let (input, length) = u16(Endianness::Little)(input)?;

    Ok((input, HidDescriptorDescriptor {
	descriptor_type,
	length,
    }))
}

fn parse_hid_descriptor(input: &[u8]) -> IResult<&[u8], HidDescriptor> {
    let (input, version) = u16(Endianness::Little)(input)?;
    let (input, country_code) = u8(input)?;
    let (input, num_descriptors) = u8(input)?;
    let (input, descriptors) = many_m_n(num_descriptors as usize, num_descriptors as usize, parse_hid_descriptor_descriptor).parse(input)?;

    Ok((input, HidDescriptor {
	version,
	country_code,
	descriptors,
    }))
}

pub fn init() {
    let usb_hid_driver = HidDriver {};
    driver::register_driver(Box::new(usb_hid_driver));
}

pub struct HidDriver {}
impl driver::Driver for HidDriver {
    fn init(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) {
	log::info!("Initialising HID device");

	if let Some(usb_info) = info.as_any().downcast_ref::<usb::UsbDevice>() {
	    if usb_info.interface_descriptor.subclass == 1 {
		log::info!("  Boot protocol");
	    } else {
		log::info!("  Report protocol");
	    }

	    if usb_info.interface_descriptor.protocol == 1 {
		log::info!("  Keyboard");
	    } else if usb_info.interface_descriptor.protocol == 2 {
		log::info!("  Mouse");
	    }

	    for descriptor in usb_info.interface_descriptor.other_descriptors.iter() {
		if descriptor.descriptor_type == 0x21 {
		    let (_, hid_descriptor) = parse_hid_descriptor(&descriptor.remaining_bytes).unwrap();
		    log::info!("{:?}", hid_descriptor);
		}
	    }
	}
    }

    fn check_device(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	if let Some(usb_info) = info.as_any().downcast_ref::<usb::UsbDevice>() {
	    usb_info.interface_descriptor.class == 3
	} else {
	    false
	}
    }

    fn check_new_device(&self, _info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	true // Not yet implemented
    }
}
