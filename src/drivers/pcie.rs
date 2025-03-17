use alloc::vec::Vec;
use alloc::string::String;
use alloc::boxed::Box;
use alloc::fmt;
use core::any::Any;
use pci_types::{ConfigRegionAccess, PciAddress, PciHeader, HeaderType, EndpointHeader, Bar, VendorId, DeviceId, BaseClass, SubClass, Interface};
use x86_64::instructions::port::{PortGeneric, ReadWriteAccess, WriteOnlyAccess};
use spin::{Mutex, Once};
use alloc::sync::Arc;

use crate::driver;
use crate::interrupts;
use crate::sys::acpi::{Namespace, namespace};
use crate::utils::vector_map::VecMap;

#[derive(Eq, PartialEq, Ord, PartialOrd, Debug)]
pub enum InterruptPin {
    IntA,
    IntB,
    IntC,
    IntD,
}

#[derive(Copy, Clone)]
pub struct PciConfigAccess { }

impl PciConfigAccess {
    pub fn new() -> PciConfigAccess {
	PciConfigAccess { }
    }

    fn get_address(&self, address: PciAddress, offset: u16) -> u32 {
	let enable_bit: u32 = 1 << 31;
	let bus_number: u32 = (address.bus() as u32) << 16;
	let device_number: u32 = (address.device() as u32) << 11;
	let function: u32 = (address.function() as u32) << 8;

	enable_bit | bus_number | device_number | function | (offset as u32)
    }
}

impl ConfigRegionAccess for PciConfigAccess {
    unsafe fn read(&self, address: PciAddress, offset: u16) -> u32 {
	let mut config_address_port = PortGeneric::<u32, WriteOnlyAccess>::new(0xCF8);
	let mut config_data_port = PortGeneric::<u32, ReadWriteAccess>::new(0xCFC);

	let address = self.get_address(address, offset);
	config_address_port.write(address);
	config_data_port.read()
    }

    unsafe fn write(&self, address: PciAddress, offset: u16, value: u32) {
	let mut config_address_port = PortGeneric::<u32, WriteOnlyAccess>::new(0xCF8);
	let mut config_data_port = PortGeneric::<u32, ReadWriteAccess>::new(0xCFC);

	let address = self.get_address(address, offset);
	config_address_port.write(address);
	config_data_port.write(value);
    }
}

pub static PCI_ACCESS: Once<Mutex<PciConfigAccess>> = Once::new();

pub fn init_pci_subsystem_for_acpi() {
    PCI_ACCESS.call_once(|| Mutex::new(PciConfigAccess::new()));
}

pub fn init() {
    // TODO: PNP0A08 is PCIe
    let pci_driver = PciDriver {};
    driver::register_driver(Box::new(pci_driver));
}

#[derive(Clone)]
pub struct PciDeviceType {
    // Location
    pub address: PciAddress,

    // ID
    pub vendor_id: VendorId,
    pub device_id: DeviceId,

    // Type
    pub base_class: BaseClass,
    pub sub_class: SubClass,
    pub interface: Interface,

    // Interrupt mapping
    pub interrupt_mapping: Option<interrupts::InterruptRoute>,
}

impl driver::DeviceTypeIdentifier for PciDeviceType {
    fn as_any(&self) -> &dyn Any {
	self
    }
}

impl fmt::Display for PciDeviceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
	write!(f, "{}/{:X}:{:X}/{:X}:{:X}:{:X}",
	       self.address,
	       self.vendor_id, self.device_id,
	       self.base_class, self.sub_class, self.interface)
    }
}

pub struct PciBus {
    acpi_namespace: Namespace,
    routing_table: VecMap<namespace::PciInterruptFunction, interrupts::InterruptRoute>,
    segment: u16,
    bus: u8,
}
unsafe impl Send for PciBus { }
unsafe impl Sync for PciBus { }

impl PciBus {
    pub fn new(acpi_namespace: Namespace, segment: u16, bus: u8) -> PciBus {
	let routing_table = namespace::get_pci_routing_table(acpi_namespace).unwrap();
	PciBus {
	    acpi_namespace: acpi_namespace,
	    routing_table: routing_table,
	    segment: segment,
	    bus: bus,
	}
    }
}

impl driver::Bus for PciBus {
    fn name(&self) -> String {
	String::from("PCI")
    }

    fn enumerate(&mut self) -> Vec<Box<dyn driver::DeviceTypeIdentifier>> {
	let pci_config_access = PciConfigAccess::new();
	let mut found_devices = Vec::<Box<dyn driver::DeviceTypeIdentifier>>::new();
	for device in 0 .. 32 {
	    for function in 0 .. 8 {
		let device_header = PciHeader::new(PciAddress::new(0, 0, device, function));
		let (vendor_id, device_id) = device_header.id(pci_config_access);

		if vendor_id == 0xFFFF {
		    continue;
		}

		let (_, base_class, sub_class, interface) = device_header.revision_and_class(pci_config_access);

		let mut interrupt_mapping: Option<interrupts::InterruptRoute> = None;

		if device_header.header_type(pci_config_access) == HeaderType::Endpoint {
		    let endpoint_header = EndpointHeader::from_header(device_header, pci_config_access).expect("Creating endpoint header failed");
		    
		    let (device_pin, device_irq) = endpoint_header.interrupt(pci_config_access);
		    if device_pin != 0 {
			// Device may use GSIs, consult with ACPI
			let interrupt_function = namespace::PciInterruptFunction {
			    device,
			    function: Some(function),
			    pin: match device_pin {
				1 => InterruptPin::IntA,
				2 => InterruptPin::IntB,
				3 => InterruptPin::IntC,
				4 => InterruptPin::IntD,
				_ => panic!("Malformed interrupt pin"),
			    },
			};

			interrupt_mapping = self.routing_table.get(&interrupt_function).cloned();
		    } else if device_irq != 0xFF {
			interrupt_mapping = Some(interrupts::InterruptRoute::Irq(device_irq));
		    }
		}

		found_devices.push(Box::new(PciDeviceType {
		    address: PciAddress::new(self.segment, self.bus, device, function),

		    vendor_id,
		    device_id,

		    base_class,
		    sub_class,
		    interface,

		    interrupt_mapping,
		}));
	    }
	}

	found_devices
    }
}

pub struct PciDriver {}
impl driver::Driver for PciDriver {
    fn init(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) {
	let system_bus_identifier = if let Some(sb_info) = info.as_any().downcast_ref::<namespace::SystemBusDeviceIdentifier>() {
	    sb_info
	} else {
	    panic!("Attempted to get SB identifier from a not SB device");
	};

	let pci_config_access = PciConfigAccess::new();
	let root_bus_header = PciHeader::new(PciAddress::new(0, 0, 0, 0));

	if root_bus_header.has_multiple_functions(pci_config_access) {
	    for function in 0 .. 8 {
		let bus_function_header = PciHeader::new(PciAddress::new(0, 0, 0, function));
		let (bus_vendor_id, _) = bus_function_header.id(pci_config_access);

		// No vendor ID means no bus on this segment
		if bus_vendor_id == 0xFFFF {
		    continue;
		}

		for device in 0 .. 32 {
		    let device_header = PciHeader::new(PciAddress::new(0, function, device, 0));
		    let (device_vendor_id, device_device_id) = device_header.id(pci_config_access);

		    if device_vendor_id == 0xFFFF {
			continue;
		    }

		    log::info!("Found PCI device, vendor = {:X}, device = {:X}", device_vendor_id, device_device_id);
		}
	    }
	} else {
	    driver::register_bus_and_enumerate(Arc::new(Mutex::new(PciBus::new(system_bus_identifier.namespace, 0, 0))));
	}
    }

    fn check_device(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	if let Some(sb_info) = info.as_any().downcast_ref::<namespace::SystemBusDeviceIdentifier>() {
	    if let Some(hid) = &sb_info.hid {
		*hid == String::from("PNP0A03")
	    } else {
		false
	    }
	} else {
	    false
	}
    }

    fn check_new_device(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	true // Not yet implemented
    }
}

pub fn get_bar(info: PciDeviceType, slot: u8) -> Option<Bar> {
    let pci_config_access = PciConfigAccess::new();
    let device_header = PciHeader::new(info.address);

    let endpoint_header = match device_header.header_type(pci_config_access) {
	HeaderType::Endpoint => EndpointHeader::from_header(device_header, pci_config_access).expect("Creating endpoint header failed"),
	HeaderType::PciPciBridge => panic!("Attempted to access BAR of PciPciBridge"),
	HeaderType::CardBusBridge => panic!("Attempted to access BAR of CardBusBridge"),
	_ => panic!("Unknown header type"),
    };

    endpoint_header.bar(slot, pci_config_access)
}
