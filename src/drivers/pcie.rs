use alloc::string::String;
use aml::AmlName;
use pci_types::{ConfigRegionAccess, PciAddress, PciHeader};
use x86_64::instructions::port::{PortGeneric, ReadWriteAccess, WriteOnlyAccess};

use crate::driver;

#[derive(Copy, Clone)]
struct PciConfigAccess { }

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

pub fn init() {
    // TODO: PNP0A08 is PCIe
    let pci_driver = driver::DriverInfo {
	hid: String::from("PNP0A03"),
	init: init_pci,
    };
    driver::register_driver(pci_driver);
}

fn init_pci(driver_id: u64, acpi_device: &AmlName, uid: u32) {
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
	for device in 0 .. 32 {
	    let device_header = PciHeader::new(PciAddress::new(0, 0, device, 0));
	    let (device_vendor_id, device_device_id) = device_header.id(pci_config_access);

	    if device_vendor_id == 0xFFFF {
		continue;
	    }

	    log::info!("Found PCI device, vendor = {:X}, device = {:X}", device_vendor_id, device_device_id);
	}
    }
}
