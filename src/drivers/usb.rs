use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::asm;
use core::ops::BitOr;
use core::slice;
use pci_types::{ConfigRegionAccess, PciAddress, PciHeader, CommandRegister};
use x86_64::instructions::port::Port;

use crate::drivers::pcie;
use crate::driver;
use crate::memory;

const USBCMD: u16 = 0x00;
const USBINTR: u16 = 0x04;
const FRBASEADD: u16 = 0x08;
const PORTSC1: u16 = 0x10;
const PORTSC2: u16 = 0x12;

const HOST_CONTROLLER_RUN: u16 = 1 << 0;
const HOST_CONTROLLER_RESET: u16 = 1 << 1;
const GLOBAL_RESET: u16 = 1 << 2;
const MAX_PACKET_SIZE_64: u16 = 1 << 7;

const SHORT_PACKET_INTERRUPT: u16 = 1 << 3;
const INTERRUPT_ON_COMPLETE: u16 = 1 << 2;
const RESUME_INTERRUPT: u16 = 1 << 1;
const TIMEOUT_CRC_INTERRUPT: u16 = 1 << 0;

pub struct UhciBus<'a> {
    pci_address: PciAddress,
    base_io: u16,

    frame_list: &'a mut [u32],
}

unsafe impl Send for UhciBus<'_> { }
unsafe impl Sync for UhciBus<'_> { }

impl<'a> UhciBus<'a> {
    pub fn new(pci_address: PciAddress, uhci_base: u16) -> UhciBus<'a> {
	let pci_config_access = pcie::PciConfigAccess::new();
	let mut device_header = PciHeader::new(pci_address);

	// Disable BIOS emulation
	unsafe {
	    pci_config_access.write(pci_address, 0xC0, 0x2000);
	}

	// Enable busmaster
	device_header.update_command(pci_config_access, |command_reg| command_reg.bitor(CommandRegister::BUS_MASTER_ENABLE));

	// Reset the controller
	unsafe {
	    let mut cmd_reg = Port::<u16>::new(uhci_base + USBCMD);
	    cmd_reg.write(HOST_CONTROLLER_RESET | GLOBAL_RESET);

	    for _ in 0 .. 1000000 { unsafe { asm!("nop"); } }

	    cmd_reg.write(0);
	}

	// Create a transfer frame page in DMA memory, and configure the controller with the physical address
	let (frame_list_virt, frame_list_phys) = memory::kernel_allocate(
	    4096, memory::MemoryAllocationType::DMA,
	    memory::MemoryAllocationOptions::Arbitrary,
	    memory::MemoryAccessRestriction::Kernel)
	    .expect("Unable to allocate a frame list memory region");
	let frame_list_phys_addr = frame_list_phys[0].as_u64();
	if frame_list_phys_addr >= 1 << 32 {
	    panic!("Frame list region is out of bounds, in higher half of physical memory");
	}

	let frame_list = unsafe {
	    slice::from_raw_parts_mut (frame_list_virt.as_mut_ptr::<u32>(), 1024)
	};
	frame_list.fill_with(Default::default);

	// Set transfer frame base address
	unsafe {
	    let mut frbase_reg = Port::<u32>::new(uhci_base + FRBASEADD);
	    frbase_reg.write(frame_list_phys_addr as u32);

	    // Receive all interrupts
	    let mut usbintr_reg = Port::<u16>::new(uhci_base + USBINTR);
	    usbintr_reg.write(SHORT_PACKET_INTERRUPT | INTERRUPT_ON_COMPLETE | RESUME_INTERRUPT | TIMEOUT_CRC_INTERRUPT);
	}

	// Check which ports are present
	unsafe {
	    let mut port_sc1 = Port::<u16>::new(uhci_base + PORTSC1);
	    let mut port_sc2 = Port::<u16>::new(uhci_base + PORTSC2);

	    let sc1_val = port_sc1.read();
	    let sc2_val = port_sc2.read();

	    if sc1_val != 0xFFFF && sc1_val & (1 << 7) != 0 {
		log::info!("Port 1 valid");
	    }
	    if sc2_val != 0xFFFF && sc2_val & (1 << 7) != 0 {
		log::info!("Port 2 valid");
	    }
	}

	// Start the controller
	unsafe {
	    let mut cmd_reg = Port::<u16>::new(uhci_base + USBCMD);
	    cmd_reg.write(HOST_CONTROLLER_RUN | MAX_PACKET_SIZE_64);
	}

	// Check which ports have devices
	unsafe {
	    let mut port_sc1 = Port::<u16>::new(uhci_base + PORTSC1);
	    let mut port_sc2 = Port::<u16>::new(uhci_base + PORTSC2);

	    let sc1_val = port_sc1.read();
	    let sc2_val = port_sc2.read();

	    if sc1_val & (1 << 0) != 0 {
		log::info!("Port 1 has device(s)");
	    }
	    if sc2_val & (1 << 0) != 0 {
		log::info!("Port 2 has device(s)");
	    }
	}

	UhciBus {
	    pci_address: pci_address,
	    base_io: uhci_base,

	    frame_list: frame_list,
	}
    }
}

impl driver::Bus for UhciBus<'_> {
    fn name(&self) -> String {
	String::from("UHCI")
    }

    fn enumerate(&self) -> Vec<Box<dyn driver::DeviceTypeIdentifier>> {
	Vec::new()
    }
}

pub fn init() {
    let uhci_driver = UhciDriver {};
    driver::register_driver(Box::new(uhci_driver));
}

pub struct UhciDriver {}
impl driver::Driver for UhciDriver {
    fn init(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) {
	log::info!("Initialising UHCI controller");

	let pci_info = if let Some(pci_info) = info.as_any().downcast_ref::<pcie::PciDeviceType>() {
	    pci_info
	} else {
	    return;
	};

	let bar = pcie::get_bar(*pci_info, 4).expect("Unable to find UHCI BAR");

	// UHCI is guaranteed to be I/O space
	let uhci_base = bar.unwrap_io();

	driver::register_bus_and_enumerate(Box::new(UhciBus::new(pci_info.address, uhci_base as u16)));
    }

    fn check_device(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	if let Some(pci_info) = info.as_any().downcast_ref::<pcie::PciDeviceType>() {
	    pci_info.base_class == 0x0C &&
		pci_info.sub_class == 0x03 &&
		pci_info.interface == 0x00
	} else {
	    false
	}
    }

    fn check_new_device(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	true // Not yet implemented
    }
}
