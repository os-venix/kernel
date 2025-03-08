use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitfield::bitfield;
use spin::{Once, Mutex};
use x86_64::PhysAddr;

use crate::dma::arena;
use crate::driver;

#[derive(PartialEq, Eq)]
pub enum PortStatus {
    CONNECTED,
    DISCONNECTED,
}

pub enum PortSpeed {
    LOW_SPEED,
    FULL_SPEED,
}

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
pub enum SetupPacketRequestTypeRequestType {
    STANDARD,
    CLASS,
    VENDOR,
}

#[repr(u8)]
pub enum SetupPacketRequestTypeRecipient {
    DEVICE,
    INTERFACE,
    ENDPOINT,
    OTHER,
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
pub enum RequestCode {
    GET_STATUS,
    CLEAR_FEATURE,
    SET_FEATURE = 3,
    SET_ADDRESS = 5,
    GET_DESCRIPTOR,
    SET_DESCRIPTOR,
    GET_CONFIGURATION,
    SET_CONFIGURATION,
    GET_INTERFACE,
    SET_INTERFACE,
    SYNC_FRAME,
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
enum Descriptor {
    #[default]
    DEVICE = 1,
    CONFIGURATION,
    STRING,
    INTERFACE,
    ENDPOINT,
    DEVICE_QUALIFIER,
    OTHER_SPEED_CONFIGURATION,
    INTERFACE_POWER,
}

#[repr(C, packed)]
struct GenericDescriptor {
    length: u8,
    descriptor_type: Descriptor,
}

#[repr(C, packed)]
#[derive(Debug, Default)]
struct ConfigurationDescriptor {
    length: u8,
    descriptor_type: Descriptor,
    total_length: u16,
    num_interfaces: u8,
    configuration_value: u8,
    configuration_index: u8,
    attributes: u8,
    max_power: u8,
}

#[repr(C, packed)]
#[derive(Debug, Default)]
struct InterfaceDescriptor {
    length: u8,
    descriptor_type: Descriptor,
    interface_number: u8,
    alternate_setting: u8,
    num_endpoints: u8,
    interface_class: u8,
    interface_subclass: u8,
    protocol: u8,
    interface_string: u8,
}

#[repr(C, packed)]
#[derive(Debug, Default)]
struct EndpointDescriptor {
    length: u8,
    descriptor_type: Descriptor,
    endpoint_address: u8,
    attributes: u8,
    max_packet_size: u16,
    interval: u8,
}

pub enum TransferType {
    ControlRead(SetupPacket),
    ControlWrite(SetupPacket),
    BulkWrite,
    BulkRead,
    InterruptOut,
    InterruptIn,
}

pub struct UsbTransfer {
    pub transfer_type: TransferType,
    pub buffer_phys_ptr: PhysAddr,
    pub poll: bool,
}

pub trait UsbHCI {
    fn get_ports(&self) -> Vec<Port>;
    fn transfer(&mut self, transfer: UsbTransfer, arena: &arena::Arena);
}

struct Usb {
    hcis: Vec<Box<dyn UsbHCI>>,
}

impl Usb {
    fn new() -> Self {
	Usb {
	    hcis: Vec::new(),
	}
    }

    fn register_hci(&mut self, mut hci: Box<dyn UsbHCI>) {
	for port in hci.get_ports() {
	    let arena = arena::Arena::new();

	    if port.status == PortStatus::DISCONNECTED {
		continue;
	    }

	    let (configuration_descriptor, configuration_descriptor_phys) = arena.acquire_default::<ConfigurationDescriptor>(0).unwrap();

	    let mut request_type = SetupPacketRequestType::default();
	    request_type.set_direction_from_enum(SetupPacketRequestTypeDirection::DeviceToHost);
	    request_type.set_request_type_from_enum(SetupPacketRequestTypeRequestType::STANDARD);
	    request_type.set_recipient_from_enum(SetupPacketRequestTypeRecipient::DEVICE);

	    let xfer_config_descriptor = UsbTransfer {
		transfer_type: TransferType::ControlRead(SetupPacket {
		    request_type: request_type.clone(),
		    request: RequestCode::GET_DESCRIPTOR,
		    value: 0x0200,
		    index: 0,
		    length: size_of::<ConfigurationDescriptor>() as u16,
		}),
		buffer_phys_ptr: configuration_descriptor_phys,
		poll: true,
	    };

	    hci.transfer(xfer_config_descriptor, &arena);

	    let (descriptors, descriptors_phys) = arena.acquire_slice(
		0, configuration_descriptor.total_length as usize).unwrap();
	    let xfer_descriptors = UsbTransfer {
		transfer_type: TransferType::ControlRead(SetupPacket {
		    request_type: request_type,
		    request: RequestCode::GET_DESCRIPTOR,
		    value: 0x0200,
		    index: 0,
		    length: configuration_descriptor.total_length,
		}),
		buffer_phys_ptr: descriptors_phys,
		poll: true,
	    };

	    hci.transfer(xfer_descriptors, &arena);
	    
	    let mut interfaces = Vec::<&InterfaceDescriptor>::new();
	    let mut endpoints = Vec::<&EndpointDescriptor>::new();

	    // This is horrible, needs cleaning
	    {
		let mut i = 0;
		let mut ptr = descriptors.as_ptr();
		while i < configuration_descriptor.total_length {
		    match descriptors[i as usize + 1] {
			4 => {  // Interface
			    let interface = unsafe {
				(ptr.add(i as usize) as *const InterfaceDescriptor).as_ref().unwrap()
			    };
			    interfaces.push(interface);
			},
			5 => {  // Endpoint
			    let endpoint = unsafe { (
				ptr.add(i as usize) as *const EndpointDescriptor).as_ref().unwrap()
			    };
			    endpoints.push(endpoint);
			},
			_ => (),
		    }

		    i += descriptors[i as usize] as u16;
		}
	    }

	    log::info!("{:?}", configuration_descriptor);
	    log::info!("{:?}", interfaces);
	    log::info!("{:?}", endpoints);
	}
	
	self.hcis.push(hci);
    }
}

unsafe impl Send for Usb { }
unsafe impl Sync for Usb { }

impl driver::Bus for Usb {
    fn name(&self) -> String {
	String::from("USB")
    }

    fn enumerate(&mut self) -> Vec<Box<dyn driver::DeviceTypeIdentifier>> {
	Vec::new()
    }
}

static USB_BUS: Once<Arc<Mutex<Usb>>> = Once::new();

pub fn init() {
    let usb_bus = Arc::new(Mutex::new(Usb::new()));
    driver::register_bus_and_enumerate(usb_bus.clone());
    USB_BUS.call_once(|| usb_bus);
}

pub fn register_hci(mut hci: Box<dyn UsbHCI>) {
    let mut usb = USB_BUS.get().unwrap().lock();
    usb.register_hci(hci);
}
