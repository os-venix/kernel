use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::dma::arena;
use crate::driver;
use crate::drivers::usb::protocol as usb_protocol;
use crate::drivers::usb::usb;
use crate::memory;

mod protocol;

#[derive(PartialEq, Eq)]
enum HidProtocol {
    Boot,
    Report,
}

#[allow(dead_code)]
struct Keyboard {
    arena: arena::Arena,
    device_info: usb::UsbDevice,
    protocol: HidProtocol,
    hid_descriptor: protocol::HidDescriptor,
}

impl Keyboard {
    pub fn new(device_info: usb::UsbDevice, protocol: HidProtocol, hid_descriptor: protocol::HidDescriptor) -> Self {
	if protocol == HidProtocol::Report {
	    unimplemented!()
	}

	let arena = arena::Arena::new();
	let (report_slice, report_phys) = arena.acquire_slice(0, 8).unwrap();

	let (endpoint_num, endpoint) = device_info.interface_descriptor.endpoints.iter()
	    .filter(|(_, endpoint)|
		    endpoint.direction == usb_protocol::EndpointDirection::In &&
		    endpoint.transfer_type == usb_protocol::EndpointTransferType::Interrupt)
	    .nth(0)
	    .unwrap();

	let xfer_config_descriptor = usb::UsbTransfer {
	    transfer_type: usb::TransferType::InterruptIn(usb::InterruptTransferDescriptor {
		endpoint: *endpoint_num,
		frequency_in_ms: endpoint.interval,
		length: 8,
	    }),
	    speed: device_info.speed,
	    buffer_phys_ptr: report_phys,
	    poll: false,
	};
	{
	    device_info.hci.lock().transfer(device_info.address, xfer_config_descriptor, &arena);
	}

	Keyboard {
	    arena,
	    device_info,
	    protocol,
	    hid_descriptor,
	}
    }
}

unsafe impl Send for Keyboard {}
unsafe impl Sync for Keyboard {}

impl driver::Device for Keyboard {
    fn read(&self, _offset: u64, _size: u64, _access_restriction: memory::MemoryAccessRestriction) -> Result<*const u8, ()> {
	unimplemented!();
    }
    fn write(&self, _buf: *const u8, _size: u64) -> Result<u64, ()> {
	unimplemented!();
    }
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
	    let protocol = if usb_info.interface_descriptor.subclass == 1 { HidProtocol::Boot } else { HidProtocol::Report };

	    let mut hid_descriptor: protocol::HidDescriptor = Default::default();
	    for descriptor in usb_info.interface_descriptor.other_descriptors.iter() {
		if descriptor.descriptor_type == 0x21 {
		    hid_descriptor = protocol::parse_hid_descriptor(&descriptor.remaining_bytes).unwrap().1;
		}
	    }

	    if usb_info.interface_descriptor.protocol == 1 {
		let device = Arc::new(Keyboard::new(
		    usb_info.clone(),
		    protocol,
		    hid_descriptor,
		));
		driver::register_device(device);
	    } else if usb_info.interface_descriptor.protocol == 2 {
		log::info!("  Mouse");
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
