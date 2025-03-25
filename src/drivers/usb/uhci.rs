use alloc::boxed::Box;
use alloc::vec::Vec;
use anyhow::{anyhow, Result};
use bitfield::bitfield;
use core::arch::asm;
use core::ptr;
use core::ops::BitOr;
use core::slice;
use core::sync::atomic::{fence, Ordering};
use pci_types::{ConfigRegionAccess, PciAddress, PciHeader, CommandRegister};
use x86_64::instructions::port::Port;
use x86_64::PhysAddr;

use crate::dma::arena;
use crate::drivers::pcie;
use crate::driver;
use crate::memory;
use crate::drivers::usb::usb;

const USBCMD: u16 = 0x00;
const USBSTS: u16 = 0x02;
const USBINTR: u16 = 0x04;
const FRBASEADD: u16 = 0x08;
const PORTSC1: u16 = 0x10;
const PORTSC2: u16 = 0x12;

const HOST_CONTROLLER_RUN: u16 = 1 << 0;
const HOST_CONTROLLER_RESET: u16 = 1 << 1;
const GLOBAL_RESET: u16 = 1 << 2;
const MAX_PACKET_SIZE_64: u16 = 1 << 7;

const STATUS_HALTED: u16 = 1 << 5;
const STATUS_INT: u16 = 1 << 0;

const SHORT_PACKET_INTERRUPT: u16 = 1 << 3;
const INTERRUPT_ON_COMPLETE: u16 = 1 << 2;
const RESUME_INTERRUPT: u16 = 1 << 1;
const TIMEOUT_CRC_INTERRUPT: u16 = 1 << 0;

const PORT_RESET: u16 = 1 << 9;
const PORT_ALWAYS_1: u16 = 1 << 7;
const PORT_ENABLE_CHANGE: u16 = 1 << 3;
const PORT_ENABLE: u16 = 1 << 2;
const PORT_STATUS_CHANGE: u16 = 1 << 1;
const PORT_CONNECTION_STATUS: u16 = 1 << 0;

bitfield! {
    pub struct Pointer(u32);

    link_pointer, set_link_pointer: 31, 4;
    qh_td_select, set_qh_td_select: 1;
    terminate, set_terminate: 0;
}

impl Pointer {
    fn set_link_pointer_phys(&mut self, link_pointer: PhysAddr) {
	self.set_link_pointer(link_pointer.as_u64() as u32 >> 4);
    }
}

impl Default for Pointer {
    fn default() -> Self { Self(1) }  // Terminate
}

#[repr(packed)]
#[derive(Default)]
#[allow(dead_code)]
struct QueueHead {
    queue_head_pointer: Pointer,
    element_link_pointer: Pointer,
}

impl QueueHead {
    #[allow(dead_code)]
    fn set_qh_pointer(&mut self, ptr: Pointer) {
	let self_ptr = &raw mut self.queue_head_pointer;
	unsafe { ptr::write_unaligned(self_ptr, ptr); }
    }
    fn set_el_pointer(&mut self, ptr: Pointer) {
	let self_ptr = &raw mut self.element_link_pointer;
	unsafe { ptr::write_unaligned(self_ptr, ptr); }
    }
}

bitfield! {
    pub struct FrameListPointer(u32);

    frame_list_pointer, set_frame_list_pointer: 31, 4;
    qh_td_select, set_qh_td_select: 1;
    terminate, set_terminate: 0;
}

impl FrameListPointer {
    fn set_frame_list_pointer_phys(&mut self, link_pointer: PhysAddr) {
	self.set_frame_list_pointer(link_pointer.as_u64() as u32 >> 4);
    }
}

impl Default for FrameListPointer {
    fn default() -> Self { Self(1) }
}

bitfield! {
    #[derive(Default)]
    pub struct TransferDescriptor(u128);

    link_pointer, set_link_pointer: 31, 4;
    depth_breadth_select, set_depth_breadth_select: 2;
    qh_td_select, set_qh_td_select: 1;
    terminate, set_terminate: 0;

    short_packet_detect, set_short_packet_detect: 61;
    error_count, set_error_count: 60, 59;
    low_speed, set_low_speed: 58;
    isochronous, set_isochronous: 57;
    interrupt_on_complete, set_interrupt_on_complete: 56;
    status_active, set_status_active: 55;
    status_stalled, _: 54;
    status_buffer_error, _: 53;
    status_babble, _: 52;
    status_nak, _: 51;
    status_crc_timeout, _: 50;
    status_bitstuff_error, _: 49;
    actual_length, _: 42, 32;

    max_length, set_max_length: 95, 85;
    toggle, set_toggle: 83;
    endpoint, set_endpoint: 82, 79;
    address, set_address: 78, 72;
    packet_identification, set_packet_identification: 71, 64;

    buffer_pointer, set_buffer_pointer: 127, 96;
}

impl TransferDescriptor {
    fn set_link_pointer_phys_addr(&mut self, link_pointer: PhysAddr) {
	self.set_link_pointer((link_pointer.as_u64() >> 4).into());
    }

    fn set_buffer_pointer_phys_addr(&mut self, buffer_pointer: PhysAddr) {
	self.set_buffer_pointer(buffer_pointer.as_u64().into());
    }
}

pub struct UhciBus<'a> {
    pci_address: PciAddress,
    base_io: u16,

    frame_list: &'a mut [FrameListPointer],

    port1: bool,
    port2: bool,
}

unsafe impl Send for UhciBus<'_> { }
unsafe impl Sync for UhciBus<'_> { }

impl<'a> UhciBus<'a> {
    pub fn new(pci_address: PciAddress, uhci_base: u16) -> UhciBus<'a> {
	let pci_config_access = pcie::PciConfigAccess::new();
	let mut device_header = PciHeader::new(pci_address);

	// Disable BIOS emulation, enable interrutps
	unsafe {
	    pci_config_access.write(pci_address, 0xC0, 0x8F00);
	    pci_config_access.write(pci_address, 0xC0, 0x2000);
	}

	// Enable busmaster
	device_header.update_command(pci_config_access, |command_reg| command_reg.bitor(
	    CommandRegister::BUS_MASTER_ENABLE | CommandRegister::MEMORY_ENABLE | CommandRegister::IO_ENABLE));

	// Reset the controller
	unsafe {
	    let mut cmd_reg = Port::<u16>::new(uhci_base + USBCMD);
	    let mut port_sts = Port::<u16>::new(uhci_base + USBSTS);

	    cmd_reg.write(GLOBAL_RESET);

	    for _ in 0 .. 1000000 { asm!("nop"); }

	    cmd_reg.write(0);

	    if cmd_reg.read() != 0 {
		panic!("Command reg is not 0");
	    }

	    if port_sts.read() != STATUS_HALTED {
		panic!("Status is not halted");
	    }

	    // Clear status by writing 1s
	    port_sts.write(0xFF);

	    cmd_reg.write(HOST_CONTROLLER_RESET);
	    for _ in 0 .. 1000000 { asm!("nop"); }
	    if (cmd_reg.read() & HOST_CONTROLLER_RESET) != 0 {
		panic!("UHCI controller did not properly reset");
	    }
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
	    slice::from_raw_parts_mut (frame_list_virt.as_mut_ptr::<FrameListPointer>(), 1024)
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
	let (port1_present, port2_present) = unsafe {
	    let mut port_sc1 = Port::<u16>::new(uhci_base + PORTSC1);
	    let mut port_sc2 = Port::<u16>::new(uhci_base + PORTSC2);

	    let sc1_val = port_sc1.read();
	    let sc2_val = port_sc2.read();

	    (sc1_val != 0xFFFF && sc1_val & PORT_ALWAYS_1 != 0, sc2_val != 0xFFFF && sc2_val & PORT_ALWAYS_1 != 0)
	};

	// Start the controller
	unsafe {
	    let mut cmd_reg = Port::<u16>::new(uhci_base + USBCMD);
	    let mut port_sts = Port::<u16>::new(uhci_base + USBSTS);

	    cmd_reg.write(HOST_CONTROLLER_RUN | MAX_PACKET_SIZE_64);
	    for _ in 0 .. 1000000 { asm!("nop"); }

	    if (port_sts.read() & STATUS_HALTED) != 0 {
		panic!("Status is halted");
	    }
	}

	// Check which ports have devices
	let (port1, port2) = unsafe {
	    let mut port_sc1 = Port::<u16>::new(uhci_base + PORTSC1);
	    let mut port_sc2 = Port::<u16>::new(uhci_base + PORTSC2);

	    let sc1_val = port_sc1.read();
	    let sc2_val = port_sc2.read();

	    (sc1_val & PORT_CONNECTION_STATUS != 0 && port1_present, sc2_val & PORT_CONNECTION_STATUS != 0 && port2_present)
	};

	UhciBus {
	    pci_address: pci_address,
	    base_io: uhci_base,

	    frame_list: frame_list,
	    port1: port1,
	    port2: port2,
	}
    }

    fn reset_port(&self, port: u8) -> Result<()> {
	if port > 2 {
	    return Err(anyhow!("Invalid port number: {}", port));
	}

	if (port == 1 && !self.port1) || (port == 2 && !self.port2) {
	    return Err(anyhow!("Port {} is not attached", port));
	}

	unsafe {
	    let port_offset = if port == 1 { PORTSC1 } else { PORTSC2 };
	    let mut port_sc = Port::<u16>::new(self.base_io + port_offset);

	    // Reset the port
	    let mut current = port_sc.read();
	    port_sc.write(current | PORT_RESET);
	    for _ in 0 .. 1000000 { asm!("nop"); }
	    current = port_sc.read();
	    port_sc.write(current & !PORT_RESET);
	    for _ in 0 .. 1000000 { asm!("nop"); }

	    // Clear any status change and enable
	    current = port_sc.read();
	    port_sc.write(current | PORT_STATUS_CHANGE);
	    current = port_sc.read();
	    port_sc.write(current | PORT_ENABLE);
	    for _ in 0 .. 1000000 { asm!("nop"); }

	    // Clear status change, and make sure we're still enabled
	    current = port_sc.read();
	    port_sc.write(current | PORT_STATUS_CHANGE | PORT_ENABLE_CHANGE | PORT_ENABLE);
	    for _ in 0 .. 1000000 { asm!("nop"); }

	    if port_sc.read() & PORT_ENABLE == 0 {
		return Err(anyhow!("Port not enabled after reset"));
	    }
	    if port_sc.read() & PORT_CONNECTION_STATUS == 0 {
		return Err(anyhow!("Port not connected after reset"));
	    }
	}

	Ok(())
    }
}
    
impl<'a> usb::UsbHCI for UhciBus<'a> {
    fn get_ports(&self) -> Vec<usb::Port> {
	let mut ports: Vec<usb::Port> = Vec::new();
	if self.port1 {
	    if let Err(e) = self.reset_port(1) {
		log::info!("{}", e);
	    } else {
		ports.push(usb::Port {
		    num: 1,
		    status: usb::PortStatus::Connected,
		    speed: usb::PortSpeed::LowSpeed,  // TODO (here and below): actually check the speed of the port
		});
	    }
	}
	if self.port2 {
	    if let Err(e) = self.reset_port(2) {
		log::info!("{}", e);
	    } else {
		ports.push(usb::Port {
		    num: 2,
		    status: usb::PortStatus::Disconnected,
		    speed: usb::PortSpeed::LowSpeed,
		});
	    }
	}

	ports
    }

    fn transfer(&mut self, address: u8, transfer: usb::UsbTransfer, arena: &arena::Arena) {
	let (queue_head, queue_head_phys) = arena.acquire_default::<QueueHead>(0x10).unwrap();
	let mut transfer_descriptors_to_check: Vec<&mut TransferDescriptor> = Vec::new();

	let (data_per_xfer, is_low_speed) = match transfer.speed {
	    usb::PortSpeed::LowSpeed => (8, true),
	    usb::PortSpeed::FullSpeed => (1023, false),
	};

	match transfer.transfer_type {
	    usb::TransferType::ControlRead(setup_packet) => {
		let (_, packet_phys) = arena.acquire::<usb::SetupPacket>(0, &setup_packet).unwrap();

		// These two are of fixed size, so we can preallocate. This makes linking everything together much easier
		let (setup_td, setup_td_phys) = arena.acquire_default::<TransferDescriptor>(0x10).unwrap();
		let (data_out_td, data_out_td_phys) = arena.acquire_default::<TransferDescriptor>(0x10).unwrap();

		setup_td.set_depth_breadth_select(true);  // Depth first
		setup_td.set_qh_td_select(false);  // Next one is a transfer descriptor
		setup_td.set_terminate(false);
		setup_td.set_error_count(3);
		setup_td.set_low_speed(is_low_speed);  // Low speed, as we don't know if it can do high speed yet
		setup_td.set_status_active(true);
		setup_td.set_max_length(7);  // 8 bytes
		setup_td.set_toggle(true);
		setup_td.set_endpoint(0);
		setup_td.set_address(address.into());
		setup_td.set_packet_identification(0x2d);  // SETUP
		setup_td.set_buffer_pointer_phys_addr(packet_phys);

		transfer_descriptors_to_check.push(setup_td);

		let mut data_in_tds: Vec<PhysAddr> = Vec::new();
		let mut current_toggle = true;
		for offset in (0 .. setup_packet.length).step_by(data_per_xfer as usize) {
		    let length = if setup_packet.length - offset < data_per_xfer {
			setup_packet.length - offset
		    } else {
			data_per_xfer
		    };

		    let (data_in_td, data_in_td_phys) = arena.acquire_default::<TransferDescriptor>(0x10).unwrap();
		    data_in_td.set_link_pointer_phys_addr(data_out_td_phys);  // Fine as a default, as this is what's needed for the last element anyway
		    data_in_td.set_depth_breadth_select(true);
		    data_in_td.set_qh_td_select(false);
		    data_in_td.set_terminate(false);
		    data_in_td.set_error_count(3);
		    data_in_td.set_low_speed(is_low_speed);
		    data_in_td.set_status_active(true);
		    data_in_td.set_max_length((length - 1) as u128);
		    data_in_td.set_toggle(current_toggle);
		    data_in_td.set_endpoint(0);
		    data_in_td.set_address(address.into());
		    data_in_td.set_packet_identification(0x69);  // IN
		    data_in_td.set_buffer_pointer_phys_addr(transfer.buffer_phys_ptr + offset.into());

		    data_in_tds.push(data_in_td_phys);
		    transfer_descriptors_to_check.push(data_in_td);
		    current_toggle = !current_toggle;
		}

		// Skip 1 td, as the first element is setup
		transfer_descriptors_to_check.iter_mut().skip(1)
		    .zip(data_in_tds.iter().skip(1))
		    .for_each(|(data_in_td, data_in_td_phys)| data_in_td.set_link_pointer_phys_addr(*data_in_td_phys));

		// Link setup to data in
		transfer_descriptors_to_check[0].set_link_pointer_phys_addr(data_in_tds[0]);

		// NULL transfer to finish
		data_out_td.set_link_pointer(0);
		data_out_td.set_depth_breadth_select(true);
		data_out_td.set_qh_td_select(false);
		data_out_td.set_terminate(true);
		data_out_td.set_error_count(3);
		data_out_td.set_low_speed(is_low_speed);
		data_out_td.set_interrupt_on_complete(!(transfer.poll));
		data_out_td.set_status_active(true);
		data_out_td.set_max_length(0x7FF);
		data_out_td.set_endpoint(0);
		data_out_td.set_address(address.into());
		data_out_td.set_packet_identification(0xE1);  // OUT
		data_out_td.set_buffer_pointer(0);

		transfer_descriptors_to_check.push(data_out_td);

		let mut el_pointer = Pointer::default();
		el_pointer.set_link_pointer_phys(setup_td_phys);
		el_pointer.set_qh_td_select(false);
		el_pointer.set_terminate(false);
		queue_head.set_el_pointer(el_pointer);
	    },
	    usb::TransferType::ControlNoData(setup_packet) => {
		let (_, packet_phys) = arena.acquire::<usb::SetupPacket>(0, &setup_packet).unwrap();

		let (data_in_td, data_in_td_phys) = arena.acquire_default::<TransferDescriptor>(0x10).unwrap();
		data_in_td.set_link_pointer(0);
		data_in_td.set_depth_breadth_select(true);
		data_in_td.set_qh_td_select(false);
		data_in_td.set_terminate(true);
		data_in_td.set_error_count(3);
		data_in_td.set_low_speed(is_low_speed);
		data_in_td.set_interrupt_on_complete(!(transfer.poll));
		data_in_td.set_status_active(true);
		data_in_td.set_max_length(0x7FF);
		data_in_td.set_toggle(true);
		data_in_td.set_endpoint(0);
		data_in_td.set_address(address.into());
		data_in_td.set_packet_identification(0x69);  // IN
		data_in_td.set_buffer_pointer(0);
		
		let (setup_td, setup_td_phys) = arena.acquire_default::<TransferDescriptor>(0x10).unwrap();
		setup_td.set_link_pointer_phys_addr(data_in_td_phys);
		setup_td.set_depth_breadth_select(true);  // Depth first
		setup_td.set_qh_td_select(false);  // Next one is a transfer descriptor
		setup_td.set_terminate(false);
		setup_td.set_error_count(3);
		setup_td.set_low_speed(is_low_speed);  // Low speed, as we don't know if it can do high speed yet
		setup_td.set_status_active(true);
		setup_td.set_max_length(7);  // 8 bytes
		setup_td.set_toggle(false);
		setup_td.set_endpoint(0);
		setup_td.set_address(address.into());
		setup_td.set_packet_identification(0x2d);  // SETUP
		setup_td.set_buffer_pointer_phys_addr(packet_phys);

		let mut el_pointer = Pointer::default();
		el_pointer.set_link_pointer_phys(setup_td_phys);
		el_pointer.set_qh_td_select(false);
		el_pointer.set_terminate(false);
		queue_head.set_el_pointer(el_pointer);

		transfer_descriptors_to_check.push(setup_td);
		transfer_descriptors_to_check.push(data_in_td);
	    },
	    _ => unimplemented!(),
	}
	
	// Clear any lingering interrupts
	unsafe {
	    let mut port_sts = Port::<u16>::new(self.base_io + USBSTS);
	    port_sts.write(STATUS_INT);
	}

	for i in 0 .. 1024 {
	    // First, ensure the frame pointer is terminated, so that the controller will not attempt to run
	    // a partially updated frame pointer
	    self.frame_list[i].set_terminate(true);
	    fence(Ordering::SeqCst);

	    // Next, update the pointer to the Queue Head
	    self.frame_list[i].set_qh_td_select(true);  // Entry is a QH
	    self.frame_list[i].set_frame_list_pointer_phys(queue_head_phys);

	    // Lastly, allow the controller to go ahead and execute
	    fence(Ordering::SeqCst);
	    self.frame_list[i].set_terminate(false);
	}

	if transfer.poll {
	    while transfer_descriptors_to_check.iter().any(|td| td.status_active()) {
		unsafe {
		    asm!("pause");
		}
	    }

	    let halted = unsafe {
		let mut port_sts = Port::<u16>::new(self.base_io + USBSTS);
		(port_sts.read() & STATUS_HALTED) != 0
	    };

	    if halted {
		log::info!("A fatal error occurred");

		for (i, td) in transfer_descriptors_to_check.iter().enumerate() {
		    log::info!("{} - length {:x}, pid {:x}, active = {}", i, td.max_length(), td.packet_identification(), td.status_active());
		    if td.status_stalled() {
			log::info!("  Stalled");
		    }
		    if td.status_buffer_error() {
			log::info!("  Buffer error");
		    }
		    if td.status_babble() {
			log::info!("  Babble");
		    }
		    if td.status_nak() {
			log::info!("  Nak");
		    }
		    if td.status_crc_timeout() {
			log::info!("  CRC/Timeout");
		    }
		    if td.status_bitstuff_error() {
			log::info!("  Bitstuff error");
		    }
		}
		panic!("Status is halted");
	    }

	    for td in transfer_descriptors_to_check.iter_mut() {
		if td.interrupt_on_complete() {
		    td.set_interrupt_on_complete(false);
		}
	    }

	    self.frame_list.fill_with(Default::default);
	} else {
	    unimplemented!();
	}
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

	let bar = pcie::get_bar(pci_info.clone(), 4).expect("Unable to find UHCI BAR");

	if let Some(interrupt_route) = &pci_info.interrupt_mapping {
	    pcie::enable_interrupts(pci_info.clone());
	    interrupt_route.register_handler(Box::new(handle_uhci_interrupts));
	}

	// UHCI is guaranteed to be I/O space
	let uhci_base = bar.unwrap_io();
	usb::register_hci(Box::new(UhciBus::new(pci_info.address, uhci_base as u16)));
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

    fn check_new_device(&self, _info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	true // Not yet implemented
    }
}

fn handle_uhci_interrupts() {
    log::info!("UHCI Interrupt!");
}
