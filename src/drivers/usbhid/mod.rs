use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

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
    device_info: usb::UsbDevice,
    protocol: HidProtocol,
    hid_descriptor: protocol::HidDescriptor,
}

impl Keyboard {
    pub fn new(device_info: usb::UsbDevice, protocol: HidProtocol, hid_descriptor: protocol::HidDescriptor) -> Self {
	if protocol == HidProtocol::Report {
	    unimplemented!()
	}

	let (endpoint_num, endpoint) = device_info.interface_descriptor.endpoints.iter()
	    .filter(|(_, endpoint)|
		    endpoint.direction == usb_protocol::EndpointDirection::In &&
		    endpoint.transfer_type == usb_protocol::EndpointTransferType::Interrupt)
	    .nth(0)
	    .unwrap();

	let mut write_request_type = usb::SetupPacketRequestType::default();
	write_request_type.set_direction_from_enum(usb::SetupPacketRequestTypeDirection::HostToDevice);
	write_request_type.set_request_type_from_enum(usb::SetupPacketRequestTypeRequestType::Standard);
	write_request_type.set_recipient_from_enum(usb::SetupPacketRequestTypeRecipient::Endpoint);

	// let unhalt_endpoint_descriptor = usb::UsbTransfer {
	//     transfer_type: usb::TransferType::ControlNoData(usb::SetupPacket {
	// 	request_type: write_request_type.clone(),
	// 	request: usb::RequestCode::ClearFeature,
	// 	value: 0,  // ENDPOINT_HALT
	// 	index: 0x80 | (*endpoint_num) as u16,
	// 	length: 0,
	//     }),
	//     endpoint: 0,
	//     speed: device_info.speed,
	//     poll: true,
	// };
	// {
	//     device_info.hci.lock().transfer(device_info.address, unhalt_endpoint_descriptor);
	// }

	let set_protocol = usb::UsbTransfer {
	    transfer_type: usb::TransferType::ControlNoData(usb::SetupPacket {
		request_type: {
		    let mut t = usb::SetupPacketRequestType::default();
		    t.set_direction_from_enum(usb::SetupPacketRequestTypeDirection::HostToDevice);
		    t.set_request_type_from_enum(usb::SetupPacketRequestTypeRequestType::Class);
		    t.set_recipient_from_enum(usb::SetupPacketRequestTypeRecipient::Interface);
		    t
		},
		request: 0x0b,  // SET_PROTOCOL
		value: 0,  // Boot protocol
		index: device_info.interface_descriptor.interface_number as u16,
		length: 0,
	    }),
	    endpoint: 0,
	    speed: device_info.speed,
	    poll: true,
	};
	{
	    device_info.hci.lock().transfer(device_info.address, set_protocol);
	}

	let set_report = usb::UsbTransfer {
	    transfer_type: usb::TransferType::ControlWrite(usb::WriteSetupPacket {
		setup_packet: usb::SetupPacket {
		    request_type: {
			let mut t = usb::SetupPacketRequestType::default();
			t.set_direction_from_enum(usb::SetupPacketRequestTypeDirection::HostToDevice);
			t.set_request_type_from_enum(usb::SetupPacketRequestTypeRequestType::Class);
			t.set_recipient_from_enum(usb::SetupPacketRequestTypeRecipient::Interface);
			t
		    },
		    request: 0x09,  // SET_REPORT
		    value: 0x0200,  // Output report (set LEDs)
		    index: device_info.interface_descriptor.interface_number as u16,
		    length: 1,
		},
		buf: Vec::from([0x00]),
	    }),
	    endpoint: 0,
	    speed: device_info.speed,
	    poll: true,
	};
	{
	    device_info.hci.lock().transfer(device_info.address, set_report);
	}

	let xfer_config_descriptor = usb::UsbTransfer {
	    transfer_type: usb::TransferType::InterruptIn(usb::InterruptTransferDescriptor {
		frequency_in_ms: endpoint.interval,
		length: 8,
	    }),
	    endpoint: *endpoint_num,
	    speed: device_info.speed,
	    poll: false,
	};
	{
	    device_info.hci.lock().transfer(device_info.address, xfer_config_descriptor);
	}

	loop {}
	Keyboard {
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
