use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use anyhow::{anyhow, Result};
use bitfield::bitfield;
use core::arch::asm;
use core::ptr;
use core::ops::BitOr;
use core::slice;
use core::sync::atomic::{fence, Ordering};
use pci_types::{ConfigRegionAccess, PciAddress, PciHeader, CommandRegister};
use spin::Mutex;
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
    #[derive(Default, Debug)]
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
    status_stalled, set_status_stalled: 54;
    status_buffer_error, set_status_buffer_error: 53;
    status_babble, set_status_babble: 52;
    status_nak, set_status_nak: 51;
    status_crc_timeout, set_status_crc_timeout: 50;
    status_bitstuff_error, set_status_bitstuff_error: 49;
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

#[derive(Debug, Eq, PartialEq)]
enum TransferStatus {
    Active,
    Done,
    Stalled,
    DataBufferError,
    Babble,
    Nak,
    CrcTimeout,
    Bitstuff,
}

#[derive(Debug, Eq, PartialEq)]
enum TransferDescriptorType {
    Setup,
    Status,
    Data,
}

struct UhciTransfer {
    pub arena: arena::Arena,
    queue_head: arena::ArenaTag,
    queue_head_phys: PhysAddr,
    transfer_descriptors: Vec<arena::ArenaTag>,
    buffer: Option<arena::ArenaTag>,
    buf_length: usize,
}

unsafe impl Send for UhciTransfer { }
unsafe impl Sync for UhciTransfer { }

impl UhciTransfer {
    pub fn new() -> Self {
	let arena = arena::Arena::new();
	let (_, queue_head_tag, queue_head_phys) = arena.acquire_default_by_tag::<QueueHead>(0x10).unwrap();
	UhciTransfer {
	    arena,
	    queue_head: queue_head_tag,
	    queue_head_phys,
	    transfer_descriptors: Vec::new(),
	    buffer: None,
	    buf_length: 0,
	}
    }

    pub fn is_complete(&self) -> bool {
	self.transfer_descriptors.iter()
	    .all(|td| {
		!self.arena.tag_to_ptr::<TransferDescriptor>(*td).status_active()
	    })
    }

    pub fn get_status(&self) -> (TransferStatus, usize) {
	let mut any_active = false;

	for (n, td_tag) in self.transfer_descriptors.iter().enumerate() {
	    let td = self.arena.tag_to_ptr::<TransferDescriptor>(*td_tag);
	    if td.status_active() {
		any_active = true;
	    }

	    if td.status_buffer_error() {
		return (TransferStatus::DataBufferError, n);
	    } else if td.status_babble() {
		return (TransferStatus::Babble, n);
	    } else if td.status_nak() && !td.status_active() {
		return (TransferStatus::Nak, n);
	    } else if td.status_crc_timeout() {
		return (TransferStatus::CrcTimeout, n);
	    } else if td.status_bitstuff_error() {
		return (TransferStatus::Bitstuff, n);
	    } else if td.status_stalled() {
		return (TransferStatus::Stalled, n);
	    }
	}

	if any_active {
	    (TransferStatus::Active, 0 as usize)
	} else {
	    (TransferStatus::Done, 0 as usize)
	}
    }

    pub fn reset_tds(&mut self) {
	for td in self.transfer_descriptors.iter_mut() {
	    self.arena.tag_to_ptr::<TransferDescriptor>(*td).set_status_stalled(false);
	    self.arena.tag_to_ptr::<TransferDescriptor>(*td).set_status_buffer_error(false);
	    self.arena.tag_to_ptr::<TransferDescriptor>(*td).set_status_babble(false);
	    self.arena.tag_to_ptr::<TransferDescriptor>(*td).set_status_nak(false);
	    self.arena.tag_to_ptr::<TransferDescriptor>(*td).set_status_crc_timeout(false);
	    self.arena.tag_to_ptr::<TransferDescriptor>(*td).set_status_bitstuff_error(false);
	}

	fence(Ordering::SeqCst);

	// Do this in reverse order, so that the last TD to be marked active is the first in the queue.
	// This will mean the controller still sees the queue as inactive, and won't try to execute it,
	// until the last step, when the first TD is marked as active, at which point absolutely everything
	// else has already been reset.
	for td in self.transfer_descriptors.iter_mut().rev() {
	    self.arena.tag_to_ptr::<TransferDescriptor>(*td).set_status_active(true);
	}
    }

    pub fn create_transfer_buffer(&mut self, size: usize) -> PhysAddr {
	let (_, buffer_tag, buffer_phys) = self.arena.acquire_slice_by_tag(0, size).unwrap();

	self.buffer = Some(buffer_tag);
	self.buf_length = size;

	buffer_phys
    }

    pub fn create_transfer_descriptor(&mut self, is_low_speed: bool, len: u8, endpoint: u8, address: u8, packet_id: u8, buf: PhysAddr, td_type: TransferDescriptorType) {
	let (td, td_tag, td_phys) = self.arena.acquire_default_by_tag::<TransferDescriptor>(0x10).unwrap();

	if self.transfer_descriptors.len() == 0 {
	    let mut el_pointer = Pointer::default();	    
	    el_pointer.set_link_pointer_phys(td_phys);
	    el_pointer.set_qh_td_select(false);
	    el_pointer.set_terminate(false);

	    self.arena.tag_to_ptr::<QueueHead>(self.queue_head).set_el_pointer(el_pointer);
	} else {
	    self.arena.tag_to_ptr::<TransferDescriptor>(*self.transfer_descriptors.iter_mut().rev().nth(0).unwrap())
		.set_link_pointer_phys_addr(td_phys);
	};

	let toggle = match td_type {
	    TransferDescriptorType::Setup => false,
	    TransferDescriptorType::Status => true,
	    TransferDescriptorType::Data => if self.transfer_descriptors.len() == 0 {
		false
	    } else {
		!self.arena.tag_to_ptr::<TransferDescriptor>(
		    *self.transfer_descriptors.iter_mut().rev().nth(0).unwrap())
		    .toggle()
	    },
	};
	let length: u16 = if len != 0 { (len - 1).into() } else { 0x7FF };

	td.set_depth_breadth_select(true);  // Depth first
	td.set_qh_td_select(false);  // Next one is a transfer descriptor
	td.set_terminate(false);
	td.set_error_count(3);
	td.set_low_speed(is_low_speed);  // Low speed, as we don't know if it can do high speed yet
	td.set_status_active(true);
	td.set_max_length(length.into());
	td.set_toggle(toggle);
	td.set_endpoint(endpoint.into());
	td.set_address(address.into());
	td.set_packet_identification(packet_id.into());
	td.set_buffer_pointer_phys_addr(buf);

	self.transfer_descriptors.push(td_tag);
    }

    pub fn finalise_and_get_qh(&mut self, poll: bool) -> PhysAddr {
	self.arena.tag_to_ptr::<TransferDescriptor>(*self.transfer_descriptors.iter_mut().rev().nth(0).unwrap())
	    .set_terminate(true);
	self.arena.tag_to_ptr::<TransferDescriptor>(*self.transfer_descriptors.iter_mut().rev().nth(0).unwrap())
	    .set_interrupt_on_complete(!poll);

	self.queue_head_phys
    }

    pub fn get_owned_buf(&mut self) -> Option<Box<[u8]>> {
	if let Some(buf_tag) = self.buffer {
	    Some(self.arena.tag_to_slice(buf_tag, self.buf_length).to_vec().into_boxed_slice())
	} else {
	    None
	}
    }

    pub fn output_tds(&self) {
	for td in self.transfer_descriptors.iter() {
	    log::info!("  {:#?}", self.arena.tag_to_ptr::<TransferDescriptor>(*td));
	}
    }
}

pub struct UhciBus<'a> {
    base_io: u16,

    frame_list: &'a mut [FrameListPointer],

    port1: bool,
    port2: bool,

    next_address: u8,

    recurring_transfers: Vec<UhciTransfer>,
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
	    base_io: uhci_base,

	    frame_list: frame_list,
	    port1: port1,
	    port2: port2,

	    next_address: 1,

	    recurring_transfers: Vec::new(),
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

    fn build_transfer(&self, address: u8, transfer: usb::UsbTransfer) -> UhciTransfer {
	let mut uhci_transfer = UhciTransfer::new();

	let (data_per_xfer, is_low_speed) = match transfer.speed {
	    usb::PortSpeed::LowSpeed => (8, true),
	    usb::PortSpeed::FullSpeed => (1023, false),
	};

	match transfer.transfer_type {
	    usb::TransferType::ControlRead(ref setup_packet) => {
		let (_, packet_phys) = uhci_transfer.arena.acquire::<usb::SetupPacket>(0, &setup_packet).unwrap();
		let buffer_phys = uhci_transfer.create_transfer_buffer(setup_packet.length as usize);

		uhci_transfer.create_transfer_descriptor(
		    is_low_speed, 8 /* len */, transfer.endpoint, address, 0x2d /* packet_id = SETUP */, packet_phys, TransferDescriptorType::Setup);

		for offset in (0 .. setup_packet.length).step_by(data_per_xfer as usize) {
		    let length = if setup_packet.length - offset < data_per_xfer {
			setup_packet.length - offset
		    } else {
			data_per_xfer
		    };

		    uhci_transfer.create_transfer_descriptor(
			is_low_speed, length.try_into().unwrap(), transfer.endpoint, address,
			0x69 /* packet_id = IN */, buffer_phys + offset.into(), TransferDescriptorType::Data);
		}

		uhci_transfer.create_transfer_descriptor(
		    is_low_speed, 0 /* len */, transfer.endpoint, address,
		    0xe1 /* packet_id = OUT */, PhysAddr::new(0), TransferDescriptorType::Status);
	    },
	    usb::TransferType::ControlWrite(ref write_setup_packet) => {
		let (_, packet_phys) = uhci_transfer.arena.acquire::<usb::SetupPacket>(0, &write_setup_packet.setup_packet).unwrap();
		let (_, data_phys) = uhci_transfer.arena.acquire_slice_buffer(
		    0, &write_setup_packet.buf.as_slice(), write_setup_packet.setup_packet.length as usize).unwrap();

		uhci_transfer.create_transfer_descriptor(
		    is_low_speed, 8 /* len */, transfer.endpoint, address, 0x2d /* packet_id = SETUP */, packet_phys, TransferDescriptorType::Setup);

		for offset in (0 .. write_setup_packet.setup_packet.length).step_by(data_per_xfer as usize) {
		    let length = if write_setup_packet.setup_packet.length - offset < data_per_xfer {
			write_setup_packet.setup_packet.length - offset
		    } else {
			data_per_xfer
		    };

		    uhci_transfer.create_transfer_descriptor(
			is_low_speed, length.try_into().unwrap(), transfer.endpoint, address,
			0xe1 /* packet_id = OUT */, data_phys + offset.into(), TransferDescriptorType::Data);
		}

		uhci_transfer.create_transfer_descriptor(
		    is_low_speed, 0 /* len */, transfer.endpoint, address,
		    0x69 /* packet_id = IN */, PhysAddr::new(0), TransferDescriptorType::Status);
	    },
	    usb::TransferType::ControlNoData(ref setup_packet) => {
		let (_, packet_phys) = uhci_transfer.arena.acquire::<usb::SetupPacket>(0, &setup_packet).unwrap();
		log::info!("{:x}", packet_phys.as_u64());
		uhci_transfer.create_transfer_descriptor(
		    is_low_speed, 8 /* len */, transfer.endpoint.into(), address, 0x2d /* packet_id = SETUP */, packet_phys, TransferDescriptorType::Setup);
		uhci_transfer.create_transfer_descriptor(
		    is_low_speed, 0 /* len */, transfer.endpoint.into(), address, 0x69 /* packet_id = IN */, PhysAddr::new(0), TransferDescriptorType::Status);
	    },
	    usb::TransferType::InterruptIn(ref interrupt_transfer_descriptor) => {
		let buffer_phys = uhci_transfer.create_transfer_buffer(interrupt_transfer_descriptor.length as usize);
		uhci_transfer.create_transfer_descriptor(
		    is_low_speed, interrupt_transfer_descriptor.length.into(), transfer.endpoint.into(),
		    address, 0x69 /* packet_id = IN */, buffer_phys, TransferDescriptorType::Data);
		// uhci_transfer.create_transfer_descriptor(
		//     is_low_speed, 0, transfer.endpoint.into(),
		//     address, 0xe1 /* packet_id = OUT */, PhysAddr::new(0), TransferDescriptorType::Status);
	    },
	    _ => unimplemented!(),
	}

	uhci_transfer
    }

    fn handle_oneshot(&mut self, mut uhci_transfer: UhciTransfer, transfer: usb::UsbTransfer) -> Option<Box<[u8]>> {
	// One shots should always be polling (for now, anyway)
	if !transfer.poll {
	    return None;
	}

	// Clear any lingering interrupts
	unsafe {
	    let mut port_sts = Port::<u16>::new(self.base_io + USBSTS);
	    port_sts.write(STATUS_INT);
	}

	let queue_head_phys = uhci_transfer.finalise_and_get_qh(transfer.poll);

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

	while !uhci_transfer.is_complete() {
	    let (ts, n) = uhci_transfer.get_status();
	    if ts != TransferStatus::Done && ts != TransferStatus::Active {
		log::info!("  TD {} - Transfer status {:?}", n, ts);
	    }

	    unsafe {
		asm!("pause");
	    }
	}
	
	let (ts, n) = uhci_transfer.get_status();
	if ts != TransferStatus::Done && ts != TransferStatus::Active {
	    log::info!("  TD {} - Transfer status {:?}", n, ts);
	}

	let halted = unsafe {
	    let mut port_sts = Port::<u16>::new(self.base_io + USBSTS);
	    (port_sts.read() & STATUS_HALTED) != 0
	};

	if halted {
	    log::info!("A fatal error occurred");
	    panic!("Status is halted");
	}

	self.frame_list.fill_with(Default::default);
	return uhci_transfer.get_owned_buf();
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

    fn transfer(&mut self, address: u8, transfer: usb::UsbTransfer) -> Option<Box<[u8]>> {
	match transfer.transfer_type {
	    usb::TransferType::InterruptIn(ref interrupt_transfer_descriptor) => {
		// Clear li'ngering interrupts ready for transfer
		unsafe {
		    let mut port_sts = Port::<u16>::new(self.base_io + USBSTS);
		    port_sts.write(STATUS_INT);
		}

		for i in (0 .. 1024).step_by(interrupt_transfer_descriptor.frequency_in_ms as usize) {
		    let mut uhci_transfer = self.build_transfer(address, transfer.clone());
		    let queue_head_phys = uhci_transfer.finalise_and_get_qh(transfer.poll);
		    self.recurring_transfers.push(uhci_transfer);

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

		None
	    },
	    _ => {
		let uhci_transfer = self.build_transfer(address, transfer.clone());
		self.handle_oneshot(uhci_transfer, transfer)
	    },
	}
    }

    fn get_free_address(&mut self) -> u8 {
	let addr = self.next_address;
	self.next_address += 1;
	addr
    }
    
    fn interrupt(&mut self) {
	let (usbint, usberr) = unsafe {
	    let mut port_sts = Port::<u16>::new(self.base_io + USBSTS);
	    let current_status = port_sts.read();

	    ((current_status & 1) == 1, (current_status & 2) == 2)
	};

	if usbint && !usberr {
	    for transfer in self.recurring_transfers.iter_mut() {
		if transfer.is_complete() {
		    transfer.reset_tds();
		}
	    }
	} else if usbint && usberr {
	    for transfer in self.recurring_transfers.iter_mut() {
		let (ts, n) = transfer.get_status();
		if ts != TransferStatus::Done && ts != TransferStatus::Active {
		    log::info!("  TD {} - Transfer status {:?}", n, ts);
		    transfer.output_tds();
		}
	    }

	    loop {}
	} else if usberr && !usbint {
	    log::info!("USB Error - not set on transaction");
	    loop {}
	}

	unsafe {
	    let mut port_sts = Port::<u16>::new(self.base_io + USBSTS);
	    port_sts.write(STATUS_INT);
	};
	log::info!("UHCI Interrupt!");
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

	// UHCI is guaranteed to be I/O space
	let uhci_base = bar.unwrap_io();
	let uhci: Arc<Mutex<Box<dyn usb::UsbHCI>>> = Arc::new(Mutex::new(Box::new(UhciBus::new(pci_info.address, uhci_base as u16))));

	if let Some(interrupt_route) = &pci_info.interrupt_mapping {
	    pcie::enable_interrupts(pci_info.clone());

	    let uhci_interrupt = uhci.clone();
	    interrupt_route.register_handler(Box::new(move || {
		handle_uhci_interrupts(&uhci_interrupt);
	    }));
	}

	usb::register_hci(uhci);
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

fn handle_uhci_interrupts(hci: &Arc<Mutex<Box<dyn usb::UsbHCI>>>) {
    hci.lock().interrupt();
}
