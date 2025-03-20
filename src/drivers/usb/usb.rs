use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::fmt;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitfield::bitfield;
use core::any::Any;
use spin::{Once, Mutex};
use x86_64::PhysAddr;

use crate::dma::arena;
use crate::driver;

#[derive(PartialEq, Eq)]
pub enum PortStatus {
    Connected,
    Disconnected,
}

#[derive(Copy, Clone)]
pub enum PortSpeed {
    LowSpeed,
    FullSpeed,
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

#[repr(C, packed)]
struct GenericDescriptor {
    length: u8,
    descriptor_type: Descriptor,
}

#[repr(C, packed)]
#[derive(Clone, Debug, Default)]
pub struct ConfigurationDescriptor {
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
#[derive(Clone, Debug, Default)]
pub struct InterfaceDescriptor {
    length: u8,
    descriptor_type: Descriptor,
    interface_number: u8,
    alternate_setting: u8,
    num_endpoints: u8,
    pub interface_class: u8,
    pub interface_subclass: u8,
    pub protocol: u8,
    interface_string: u8,
}

#[repr(C, packed)]
#[derive(Clone, Debug, Default)]
pub struct EndpointDescriptor {
    length: u8,
    descriptor_type: Descriptor,
    endpoint_address: u8,
    attributes: u8,
    max_packet_size: u16,
    interval: u8,
}

#[allow(dead_code)]
pub enum TransferType {
    ControlRead(SetupPacket),
    ControlWrite(SetupPacket),
    ControlNoData(SetupPacket),
    BulkWrite,
    BulkRead,
    InterruptOut,
    InterruptIn,
}

pub struct UsbTransfer {
    pub transfer_type: TransferType,
    pub speed: PortSpeed,
    pub buffer_phys_ptr: PhysAddr,
    pub poll: bool,
}

pub trait UsbHCI {
    fn get_ports(&self) -> Vec<Port>;
    fn transfer(&mut self, address: u8, transfer: UsbTransfer, arena: &arena::Arena);
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct UsbDevice {
    pub configuration_descriptor: ConfigurationDescriptor,
    pub interface_descriptors: Vec<InterfaceDescriptor>,
    pub endpoint_descriptors: Vec<EndpointDescriptor>,
    pub address: u8,
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
	       self.interface_descriptors[0].interface_class,
	       self.interface_descriptors[0].interface_subclass,
	       self.interface_descriptors[0].protocol)
    }
}

#[allow(dead_code)]
struct Usb {
    hcis: Vec<Box<dyn UsbHCI>>,
    devices: BTreeMap<u8, UsbDevice>,
    next_address: u8,
}

impl Usb {
    fn new() -> Self {
	Usb {
	    hcis: Vec::new(),
	    devices: BTreeMap::new(),
	    next_address: 1,
	}
    }

    fn get_next_address(&mut self) -> u8 {
	let addr = self.next_address;
	self.next_address += 1;

	// TODO: handle address recycling
	// TODO: error here, rather than panicking
	if addr > 127 {
	    panic!("Ran out of USB addresses!");
	}

	addr
    }

    fn register_hci(&mut self, mut hci: Box<dyn UsbHCI>) {
	for port in hci.get_ports() {
	    let arena = arena::Arena::new();

	    if port.status == PortStatus::Disconnected {
		continue;
	    }

	    let (configuration_descriptor, configuration_descriptor_phys) = arena.acquire_default::<ConfigurationDescriptor>(0).unwrap();

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
		    length: size_of::<ConfigurationDescriptor>() as u16,
		}),
		speed: port.speed,
		buffer_phys_ptr: configuration_descriptor_phys,
		poll: true,
	    };
	    hci.transfer(0, xfer_config_descriptor, &arena);

	    let device_address = self.get_next_address();

	    let set_addr = UsbTransfer {
		transfer_type: TransferType::ControlNoData(SetupPacket {
		    request_type: write_request_type,
		    request: RequestCode::SetAddress,
		    value: device_address.into(),
		    index: 0,
		    length: 0,
		}),
		speed: port.speed,
		buffer_phys_ptr: PhysAddr::new(0),
		poll: true,
	    };
	    hci.transfer(0, set_addr, &arena);

	    let (descriptors, descriptors_phys) = arena.acquire_slice(
		0, configuration_descriptor.total_length as usize).unwrap();
	    let xfer_descriptors = UsbTransfer {
		transfer_type: TransferType::ControlRead(SetupPacket {
		    request_type: read_request_type,
		    request: RequestCode::GetDescriptor,
		    value: 0x0200,
		    index: 0,
		    length: configuration_descriptor.total_length,
		}),
		speed: port.speed,
		buffer_phys_ptr: descriptors_phys,
		poll: true,
	    };
	    hci.transfer(device_address, xfer_descriptors, &arena);
	    
	    let mut interfaces = Vec::<InterfaceDescriptor>::new();
	    let mut endpoints = Vec::<EndpointDescriptor>::new();

	    // This is horrible, needs cleaning
	    {
		let mut i = 0;
		let ptr = descriptors.as_ptr();
		while i < configuration_descriptor.total_length {
		    match descriptors[i as usize + 1] {
			4 => {  // Interface
			    let interface = unsafe {
				(ptr.add(i as usize) as *const InterfaceDescriptor).as_ref().unwrap()
			    };
			    interfaces.push(interface.clone());
			},
			5 => {  // Endpoint
			    let endpoint = unsafe { (
				ptr.add(i as usize) as *const EndpointDescriptor).as_ref().unwrap()
			    };
			    endpoints.push(endpoint.clone());
			},
			_ => (),
		    }

		    i += descriptors[i as usize] as u16;
		}
	    }

	    let device = UsbDevice {
		configuration_descriptor: configuration_descriptor.clone(),
		interface_descriptors: interfaces,
		endpoint_descriptors: endpoints,
		address: device_address,
	    };

	    self.devices.insert(device_address, device.clone());
	    driver::enumerate_device(Box::new(device));
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

pub fn register_hci(hci: Box<dyn UsbHCI>) {
    let mut usb = USB_BUS.get().unwrap().lock();
    usb.register_hci(hci);
}
