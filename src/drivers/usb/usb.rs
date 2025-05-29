use alloc::boxed::Box;
use alloc::fmt;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitfield::bitfield;
use core::any::Any;
use spin::Mutex;

use crate::driver;
use crate::drivers::usb::protocol;

#[derive(PartialEq, Eq)]
pub enum PortStatus {
    Connected,
    Disconnected,
}

#[allow(dead_code)]
#[derive(Copy, Clone)]
pub enum PortSpeed {
    LowSpeed,
    FullSpeed,
}

#[allow(dead_code)]
pub struct Port {
    pub num: u32,
    pub status: PortStatus,
    pub speed: PortSpeed,
}

#[repr(u8)]
#[derive(PartialEq, Eq)]
pub enum SetupPacketRequestTypeDirection {
    HostToDevice,
    DeviceToHost,
}

#[repr(u8)]
#[allow(dead_code)]
pub enum SetupPacketRequestTypeRequestType {
    Standard,
    Class,
    Vendor,
}

#[repr(u8)]
#[allow(dead_code)]
pub enum SetupPacketRequestTypeRecipient {
    Device,
    Interface,
    Endpoint,
    Other,
}

bitfield! {
    #[derive(Clone, Copy, Default)]
    pub struct SetupPacketRequestType(u8);

    direction, set_direction: 7;
    request_type, set_request_type: 6, 5;
    recipient, set_recipient: 4, 0;
}

impl SetupPacketRequestType {
    fn set_direction_from_enum(&mut self, direction: SetupPacketRequestTypeDirection) {
	self.set_direction(direction == SetupPacketRequestTypeDirection::DeviceToHost);
    }

    fn set_request_type_from_enum(&mut self, request_type: SetupPacketRequestTypeRequestType) {
	self.set_request_type(request_type as u8);
    }

    fn set_recipient_from_enum(&mut self, recipient: SetupPacketRequestTypeRecipient) {
	self.set_recipient(recipient as u8);
    }
}

#[repr(u8)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub enum RequestCode {
    GetStatus,
    ClearFeature,
    SetFeature = 3,
    SetAddress = 5,
    GetDescriptor,
    SetDescriptor,
    GetConfiguration,
    SetConfiguration,
    GetInterface,
    SetInterface,
    SyncFrame,
}

#[repr(C, packed)]
#[derive(Clone)]
pub struct SetupPacket {
    pub request_type: SetupPacketRequestType,
    pub request: RequestCode,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default)]
#[allow(dead_code)]
enum Descriptor {
    #[default]
    Device = 1,
    Configuration,
    String,
    Interface,
    Endpoint,
    DeviceQualifier,
    OtherSpeedConfiguration,
    InterfacePower,
}

#[allow(dead_code)]
pub struct InterruptTransferDescriptor {
    pub endpoint: u8,
    pub frequency_in_ms: u8,
    pub length: u8,
}

#[allow(dead_code)]
pub enum TransferType {
    ControlRead(SetupPacket),
    ControlWrite(SetupPacket),
    ControlNoData(SetupPacket),
    BulkWrite,
    BulkRead,
    InterruptOut,
    InterruptIn(InterruptTransferDescriptor),
}

pub struct UsbTransfer {
    pub transfer_type: TransferType,
    pub speed: PortSpeed,
    pub poll: bool,
}

pub trait UsbHCI: Send + Sync {
    fn get_ports(&self) -> Vec<Port>;
    fn transfer(&mut self, address: u8, transfer: UsbTransfer) -> Option<Box<[u8]>>;
    fn get_free_address(&mut self) -> u8;
    fn interrupt(&mut self);
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct UsbDevice {
    pub configuration_descriptor: protocol::ConfigurationDescriptor,
    pub interface_descriptor: protocol::InterfaceDescriptor,
    pub address: u8,
    pub hci: Arc<Mutex<Box<dyn UsbHCI>>>,
    pub speed: PortSpeed,
}

impl driver::DeviceTypeIdentifier for UsbDevice {
    fn as_any(&self) -> &dyn Any {
	self
    }
}

impl fmt::Display for UsbDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
	// Todo: this only supports devices with one ID
	write!(f, "usb/{}:{}:{}",
	       self.interface_descriptor.class,
	       self.interface_descriptor.subclass,
	       self.interface_descriptor.protocol)
    }
}

pub fn register_hci(locked_hci: Arc<Mutex<Box<dyn UsbHCI>>>) {
    let mut devices: Vec<UsbDevice> = Vec::new();
    {
	let mut hci = locked_hci.lock();
	for port in hci.get_ports() {
	    if port.status == PortStatus::Disconnected {
		continue;
	    }

	    let mut read_request_type = SetupPacketRequestType::default();
	    read_request_type.set_direction_from_enum(SetupPacketRequestTypeDirection::DeviceToHost);
	    read_request_type.set_request_type_from_enum(SetupPacketRequestTypeRequestType::Standard);
	    read_request_type.set_recipient_from_enum(SetupPacketRequestTypeRecipient::Device);

	    let mut write_request_type = SetupPacketRequestType::default();
	    write_request_type.set_direction_from_enum(SetupPacketRequestTypeDirection::HostToDevice);
	    write_request_type.set_request_type_from_enum(SetupPacketRequestTypeRequestType::Standard);
	    write_request_type.set_recipient_from_enum(SetupPacketRequestTypeRecipient::Device);

	    let xfer_config_descriptor = UsbTransfer {
		transfer_type: TransferType::ControlRead(SetupPacket {
		    request_type: read_request_type.clone(),
		    request: RequestCode::GetDescriptor,
		    value: 0x0200,
		    index: 0,
		    length: 9,
		}),
		speed: port.speed,
		poll: true,
	    };
	    let configuration_descriptor_slice = hci.transfer(0, xfer_config_descriptor).unwrap();
	    let (_, configuration_descriptor) = protocol::parse_configuration_descriptor(&configuration_descriptor_slice).unwrap();

	    let device_address = hci.get_free_address();

	    let set_addr = UsbTransfer {
		transfer_type: TransferType::ControlNoData(SetupPacket {
		    request_type: write_request_type,
		    request: RequestCode::SetAddress,
		    value: device_address.into(),
		    index: 0,
		    length: 0,
		}),
		speed: port.speed,
		poll: true,
	    };
	    hci.transfer(0, set_addr);

	    let xfer_descriptors = UsbTransfer {
		transfer_type: TransferType::ControlRead(SetupPacket {
		    request_type: read_request_type,
		    request: RequestCode::GetDescriptor,
		    value: 0x0200,
		    index: 0,
		    length: configuration_descriptor.total_length,
		}),
		speed: port.speed,
		poll: true,
	    };
	    let descriptors = hci.transfer(device_address, xfer_descriptors).unwrap();

	    // Effectively treat each interface as its own device, which it more or less is
	    let (_, (configuration_descriptor, interface_descriptors)) = protocol::parse_configuration_descriptors(&descriptors).unwrap();
	    for interface_descriptor in interface_descriptors {
		let device = UsbDevice {
		    configuration_descriptor: configuration_descriptor.clone(),
		    interface_descriptor,
		    address: device_address,
		    hci: locked_hci.clone(),
		    speed: port.speed,
		};

		devices.push(device);
	    }
	}
    }

    for device in devices {
	driver::enumerate_device(Box::new(device));
    }
}
