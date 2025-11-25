use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;
use bytes::Bytes;
use futures_util::future::BoxFuture;

use crate::console;
use crate::driver;
use crate::drivers::usb::protocol as usb_protocol;
use crate::drivers::usb::usb;
use crate::sys::syscall;
use crate::sys::ioctl;

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
    poll_interval: u8,
    endpoint_num: u8,

    // Current state
    current_active_key: RwLock<Option<protocol::Key>>,
}

impl Keyboard {
    pub fn new(device_info: usb::UsbDevice, protocol: HidProtocol, hid_descriptor: protocol::HidDescriptor) -> Self {
	if protocol == HidProtocol::Report {
	    unimplemented!()
	}

	let (endpoint_num, endpoint) = device_info.interface_descriptor.endpoints.clone().into_iter()
	    .filter(|(_, endpoint)|
		    endpoint.direction == usb_protocol::EndpointDirection::In &&
		    endpoint.transfer_type == usb_protocol::EndpointTransferType::Interrupt)
	    .nth(0)
	    .unwrap();

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
	    callback: None,
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
	    callback: None,
	};
	{
	    device_info.hci.lock().transfer(device_info.address, set_report);
	}

	Keyboard {
	    device_info,
	    protocol,
	    hid_descriptor,
	    poll_interval: endpoint.interval,
	    endpoint_num: endpoint_num.clone(),
	    current_active_key: RwLock::new(None),
	}
    }

    pub fn start_with_callback(&self, callback: Arc<(dyn Fn(bytes::Bytes) + Send + Sync)>) {
	let xfer_config_descriptor = usb::UsbTransfer {
	    transfer_type: usb::TransferType::InterruptIn(usb::InterruptTransferDescriptor {
		frequency_in_ms: self.poll_interval,
		length: 8,
	    }),
	    endpoint: self.endpoint_num,
	    speed: self.device_info.speed,
	    poll: false,
	    callback: Some(callback),
	};
	{
	    self.device_info.hci.lock().transfer(self.device_info.address, xfer_config_descriptor);
	}
    }

    pub fn keypresses(&self, kp: protocol::BootKeyPresses) {
	let most_recent_key: Option<protocol::Key> = kp.keys.into_iter()
	    .filter(|key| *key != protocol::Key::Unknown)
	    .collect::<Vec<_>>()
	    .last()
	    .copied();

	let mut current_active_key = self.current_active_key.write();
	if most_recent_key != *current_active_key {
	    match most_recent_key {
		Some(protocol::Key::AsciiKey(mrk)) => console::register_keypress(mrk),
		_ => (),
	    }
	    *current_active_key = most_recent_key;
	}
    }
}

unsafe impl Send for Keyboard {}
unsafe impl Sync for Keyboard {}

impl driver::Device for Keyboard {
    fn read(self: Arc<Self>, _offset: u64, _size: u64) -> BoxFuture<'static, Result<Bytes, syscall::CanonicalError>> {
	unimplemented!();
    }
    fn write(&self, _buf: *const u8, _size: u64) -> Result<u64, ()> {
	unimplemented!();
    }
    fn ioctl(self: Arc<Self>, _ioctl: ioctl::IoCtl, _buf: u64) -> Result<u64, ()> {
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
		let dc = device.clone();
		device.clone().start_with_callback(Arc::new(move |buf| {
		    let (_, keypresses) = protocol::parse_boot_buffer(buf.as_ref()).unwrap();
		    dc.keypresses(keypresses);
		}));
		driver::register_device(device.clone());
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
