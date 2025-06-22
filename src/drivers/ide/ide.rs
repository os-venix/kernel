use alloc::string::String;
use core::arch::asm;
use core::ascii;
use core::slice;
use alloc::sync::Arc;
use spin::{Mutex, MutexGuard};
use x86_64::instructions::port::Port;
use itertools::Itertools;
use alloc::vec;
use core::ptr;
use bit_field::BitField;
use bytes::Bytes;
use futures_util::future::BoxFuture;
use alloc::boxed::Box;

use crate::dma::arena;
use crate::driver;
use crate::memory;
use crate::sys::block;
use crate::sys::syscall;

const IDE_CTL_REG: u16 = 0;
const IDE_CTL_NIEN: u8 = 1 << 1;
const IDE_CTL_SRST: u8 = 1 << 2;
const IDE_CTL_HOB: u8 = 1 << 7;

const IDE_DRIVE_HEAD_REG: u16 = 6;
const IDE_DRIVE_HEAD_BASE: u8 = 0x40;  // LBA, + always set bits
const IDE_DRIVE_HEAD_DRIVE_SEL_PRIMARY: u8 = 0;
const IDE_DRIVE_HEAD_DRIVE_SEL_SECONDARY: u8 = 1 << 4;

const IDE_CMD_REG: u16 = 7;
const IDE_CMD_IDENTIFY: u8 = 0xEC;
const IDE_CMD_PACKET_IDENTIFY: u8 = 0xA1;
const IDE_CMD_READ_PIO_EXT: u8 = 0x24;
const IDE_CMD_READ_DMA_EXT: u8 = 0x25;

const IDE_STATUS_REG: u16 = 7;
const IDE_STATUS_ERR: u8 = 1;
const IDE_STATUS_RDY: u8 = 1 << 6;
const IDE_STATUS_BSY: u8 = 1 << 7;
const IDE_STATUS_DF: u8 = 1 << 5;
const IDE_STATUS_DRQ: u8 = 1 << 3;

const IDE_REG_CYL_LO: u16 = 4;
const IDE_REG_CYL_HI: u16 = 5;

const IDE_REG_LBA0: u16 = 3;
const IDE_REG_LBA1: u16 = 4;
const IDE_REG_LBA2: u16 = 5;
const IDE_REG_SECCOUNT: u16 = 2;

const IDE_DATA_REG: u16 = 0;
const IDE_ERR_REG: u16 = 1;

const IDE_IDENT_MODEL: usize = 54;

const IDE_BUSMASTER_PRDT_REG: u16 = 0x04;
const IDE_BUSMASTER_COMMAND_REG: u16 = 0x00;
const IDE_BUSMASTER_STATUS_REG: u16 = 0x02;

const IDE_BUSMASTER_COMMAND_READ: u8 = 1 << 3;
const IDE_BUSMASTER_COMMAND_START: u8 = 1 << 0;

const IDE_BUSMASTER_STATUS_ACTIVE: u8 = 1 << 0;
const IDE_BUSMASTER_STATUS_DMA_ERR: u8 = 1 << 1;
const IDE_BUSMASTER_STATUS_INTERRUPT: u8 = 1 << 2;

#[repr(C, packed(1))]
#[derive(Copy, Clone)]
struct ModelNumber([ascii::Char; 40]);
impl Default for ModelNumber {
    fn default() -> Self { ModelNumber([ascii::Char::Null; 40]) }
}

#[repr(C, packed(1))]
#[derive(Copy, Clone)]
struct MediaSerialNumber([ascii::Char; 60]);
impl Default for MediaSerialNumber {
    fn default() -> Self { MediaSerialNumber([ascii::Char::Null; 60]) }
}

#[derive(Debug)]
enum Mode {
    MWDMA2,
    MWDMA1,
    MWDMA0,
    UDMA6,
    UDMA5,
    UDMA4,
    UDMA3,
    UDMA2,
    UDMA1,
    UDMA0,
    PIO,
}

#[repr(C, packed(1))]
#[derive(Copy, Clone, Default)]
struct IdentifyStruct {
    general_configuration: u16,
    obsolete_1: u16,
    specific_configuration: u16,
    obsolete_2: u16,
    retired_1: [u16; 2],
    obsolete_14: u16,
    reserved_1: [u16; 2],
    retired_2: u16,
    serial_number: [ascii::Char; 20],
    reserved_15: [u16; 2],
    obsolete_3: u16,
    firmware_revision: [ascii::Char; 8],
    model_number: ModelNumber,
    max_logical_sectors: u16,
    trusted_computing: u16,
    capabilities: u32,
    obsolete_4: [u16; 2],
    validity: u16,
    obsolete_5: [u16; 5],
    supported_commands_0: u16,
    sectors_28: u32,
    obsolete_7: u16,
    dma_modes: u16,
    pio_modes: u16,
    minimum_multiword_dma_cycle_time: u16,
    mfg_multiword_dma_cycle_time: u16,
    minimum_pio_cycle_time_no_flow_control: u16,
    minimum_pio_cycle_time_flow_control: u16,
    additional_supported: u16,
    reserved_2: [u16; 5],
    queue_depth: u16,
    sata_capabilities: u16,
    sata_additional_capabilities: u16,
    sata_supported_features: u16,
    sata_enabled_features: u16,
    major_version: u16,
    minor_version: u16,
    supported_commands: [u16; 6],
    udma_modes: u16,
    normal_security_erase_time: u16,
    enhanced_security_erase_time: u16,
    apm_level: u16,
    password_identifier: u16,
    hardware_reset_results: u16,
    obsolete_8: u16,
    stream_minimum_request_size: u16,
    stream_transfer_time_dma: u16,
    stream_access_latency: u16,
    stream_performance_granularity: u32,
    addressable_logical_sectors: u64,
    stream_transfer_time_pio: u16,
    blocks_per_dataset_mgmt: u16,
    physical_sector_size: u16,
    inter_seek_delay: u16,
    worldwide_name: u64,
    reserved_4: [u16; 4],
    obsolete_9: u16,
    logical_sector_size: u32,
    supported_commands_3: [u16; 2],
    reserved_5: [u16; 6],
    obsolete_10: u16,
    security_status: u16,
    vendor_specific: [u16; 31],
    reserved_7: [u16; 8],
    device_nominal_form_factor: u16,
    data_set_management_trim_support: u16,
    additional_product_identifier: [ascii::Char; 8],
    reserved_9: [u16; 2],
    current_media_serial_number: MediaSerialNumber,
    sct_command_transport: u16,
    reserved_10: [u16; 2],
    aligment_of_logical_sectors: u16,
    write_read_verify_sector_mode_3_count: u32,
    write_read_verify_sector_mode_2_count: u32,
    obsolete_11: [u16; 3],
    nominal_media_rotation_rate: u16,
    reserved_11: u16,
    obsolete_13: u16,
    write_read_verify_feature_set_current_mode: u16,
    reserved_12: u16,
    transport_major_version_number: u16,
    transport_minor_version_number: u16,
    reserved_13: [u16; 6],
    extended_user_addressable_sectors: u64,
    minimum_blocks_per_microcode_operation: u16,
    maximum_blocks_per_microcode_operation: u16,
    reserved_14: [u16; 19],
    integrity_word: u16,
}

impl IdentifyStruct {
    fn get_model(&self) -> String {
	String::from(
	    self.model_number.0.iter()
		.tuples()
		.flat_map(|(a, b)| vec![b.to_char(), a.to_char()])
		.collect::<String>()
		.as_str()
		.trim())	
    }

    fn get_size_in_sectors(&self) -> u64 {
	if self.is_lba48() {
	    self.addressable_logical_sectors
	} else {
	    self.sectors_28 as u64
	}
    }

    fn is_lba48(&self) -> bool {
	let c = self.supported_commands[1];
	c.get_bit(10)
    }

    fn get_mode(&self) -> Mode {
	let dma_modes = self.dma_modes;
	let udma_modes = self.udma_modes;

	if dma_modes.get_bit(10) {
	    Mode::MWDMA2
	} else if dma_modes.get_bit(9) {
	    Mode::MWDMA1
	} else if dma_modes.get_bit(8) {
	    Mode::MWDMA0
	} else if udma_modes.get_bit(14) {
	    Mode::UDMA6
	} else if udma_modes.get_bit(13) {
	    Mode::UDMA5
	} else if udma_modes.get_bit(12) {
	    Mode::UDMA4
	} else if udma_modes.get_bit(11) {
	    Mode::UDMA3
	} else if udma_modes.get_bit(10) {
	    Mode::UDMA2
	} else if udma_modes.get_bit(9) {
	    Mode::UDMA1
	} else if udma_modes.get_bit(8) {
	    Mode::UDMA0
	} else {
	    Mode::PIO
	}
    }
}

#[derive(Debug, PartialEq, Eq)]
enum DriveType {
    ATA,
    ATAPI,
    SATA,
    SATAPI,
}

struct IdeController {
    arena: arena::Arena,
    prdt: arena::ArenaTag,
    control_base: u16,
    io_base: u16,
    busmaster_base: Option<u32>,
    prdt_phys: u32,
}

impl IdeController {
    pub fn new(control_base: u16, io_base: u16, busmaster_base: Option<u32>) {
	let arena = arena::Arena::new();
	let (_, prdt, prdt_phys_addr) = arena.acquire_slice_by_tag(0, 4096).unwrap();

	let ide_controller = IdeController {
	    arena: arena,
	    prdt: prdt,
	    control_base: control_base,
	    io_base: io_base,
	    busmaster_base: busmaster_base,
	    prdt_phys: prdt_phys_addr.as_u64() as u32,
	};

	ide_controller.reset();

	let locked = Arc::new(Mutex::new(ide_controller));
	if let Some(ide_drive) = IdeDrive::new(locked.clone(), 0) {
	    let model = ide_drive.ident.get_model();
	    let size = ide_drive.ident.get_size_in_sectors();

	    log::info!("Drive 0: {} - {} MiB", model, size / (1024 * 2));
	    let device_arc = Arc::new(ide_drive);
	    driver::register_device(device_arc.clone());
	    block::register_block_device(device_arc);
	}
	if let Some(ide_drive) = IdeDrive::new(locked, 1) {
	    let model = ide_drive.ident.get_model();
	    let size = ide_drive.ident.get_size_in_sectors();
	    log::info!("Drive 1: {} - {} MiB", model, size / (1024 * 2));

	    let device_arc = Arc::new(ide_drive);
	    driver::register_device(device_arc.clone());
	    block::register_block_device(device_arc);
	}
    }

    fn reset(&self) {
	// Reset the drive controller
	unsafe {
	    let mut ctl_reg = Port::<u8>::new(self.control_base + IDE_CTL_REG);
	    ctl_reg.write(IDE_CTL_NIEN | IDE_CTL_SRST);
	}
	for _ in 0 .. 1000000 { unsafe { asm!("nop"); } }
	unsafe {
	    let mut ctl_reg = Port::<u8>::new(self.control_base + IDE_CTL_REG);
	    ctl_reg.write(IDE_CTL_NIEN);
	}
	for _ in 0 .. 1000000 { unsafe { asm!("nop"); } }
    }
}

struct IdeDrive {
    controller: Arc<Mutex<IdeController>>,
    drive_num: u8,
    ident: IdentifyStruct,
    drive_type: DriveType,
}

unsafe impl Send for IdeDrive { }
unsafe impl Sync for IdeDrive { }

impl driver::Device for IdeDrive {
    fn read(self: Arc<Self>, offset: u64, size: u64, access_restriction: memory::MemoryAccessRestriction) -> BoxFuture<'static, Result<Bytes, syscall::CanonicalError>> {
	if self.drive_type != DriveType::ATA {
	    return Box::pin(async move { Err(syscall::CanonicalError::EIO) });
	}
	let mode = self.ident.get_mode();

	match mode {
	    Mode::PIO => Box::pin(async move { self.clone().pio_read(offset, size).await }),
	    _ => Box::pin(async move { self.clone().dma_read(offset, size, access_restriction).await }),
	}
    }

    fn write(&self, _buf: *const u8, _size: u64) -> Result<u64, ()> {
	panic!("Attempted to write to IDE drive. Not yet implemented");
    }

    fn ioctl(&self, ioctl: u64) -> Result<(Bytes, usize, u64), ()> {
	panic!("Shouldn't have attempted to ioctl to the IDE drive. That makes no sense.");
    }
}

impl IdeDrive {
    pub fn new(controller: Arc<Mutex<IdeController>>, drive_num: u8) -> Option<IdeDrive> {
	let mut ide_drive = IdeDrive {
	    controller: controller,
	    drive_num: drive_num,
	    ident: IdentifyStruct {
		..Default::default()
	    },
	    drive_type: DriveType::ATA,
	};

	if !ide_drive.check_exists_and_set_type() {
	    return None;
	}

	ide_drive.set_ident();
	Some(ide_drive)
    }

    fn select(&self, ctl: &MutexGuard<'_, IdeController>) {
	// TODO: make port a shared, locked resource
	let select_cmd = IDE_DRIVE_HEAD_BASE | if self.drive_num == 0 {
	    IDE_DRIVE_HEAD_DRIVE_SEL_PRIMARY
	} else {
	    IDE_DRIVE_HEAD_DRIVE_SEL_SECONDARY
	};
	unsafe {
	    let mut drive_head_reg = Port::<u8>::new(ctl.io_base + IDE_DRIVE_HEAD_REG);
	    drive_head_reg.write(select_cmd);
	}
	for _ in 0 .. 1000000 { unsafe { asm!("nop"); } }
    }

    fn check_exists_and_set_type(&mut self) -> bool {
	let ctl = self.controller.lock();
	self.select(&ctl);
	unsafe {
	    let mut cmd_reg = Port::<u8>::new(ctl.io_base + IDE_CMD_REG);
	    cmd_reg.write(IDE_CMD_IDENTIFY);
	}
	for _ in 0 .. 1000000 { unsafe { asm!("nop"); } }

	unsafe {
	    let mut status_reg = Port::<u8>::new(ctl.io_base + IDE_STATUS_REG);
	    // No drive detected
	    if status_reg.read() == 0 {
		return false;
	    }

	    loop {
		let status = status_reg.read();
		if status & IDE_STATUS_ERR == 1 {

		    let mut reg_cyl_lo = Port::<u8>::new(ctl.io_base + IDE_REG_CYL_LO);
		    let mut reg_cyl_hi = Port::<u8>::new(ctl.io_base + IDE_REG_CYL_HI);

		    let cl = reg_cyl_lo.read();
		    let ch = reg_cyl_hi.read();

		    self.drive_type = match (cl, ch) {
			(0x14, 0xEB) => DriveType::ATAPI,
			(0x69, 0x96) => DriveType::SATAPI,
			(0x00, 0x00) => DriveType::ATA,
			(0x3C, 0xC3) => DriveType::SATA,
			_ => {
			    log::info!("Unrecognised drive type {}:{} for drive {}", cl, ch, self.drive_num);
			    return false;
			},
		    };

		    log::info!("Found drive type {:?}", self.drive_type);

		    return true;
		}

		// This is an ATA drive
		if (status & IDE_STATUS_BSY == 0) &&
		    (status & IDE_STATUS_RDY != 0) {
			self.drive_type = DriveType::ATA;
			return true;
		    }
	    }
	}
    }

    fn set_ident(&mut self) {
	let ctl = self.controller.lock();
	self.select(&ctl);

	let cmd = match self.drive_type {
	    DriveType::ATAPI => IDE_CMD_PACKET_IDENTIFY,
	    DriveType::SATAPI => IDE_CMD_PACKET_IDENTIFY,
	    DriveType::ATA => IDE_CMD_IDENTIFY,
	    DriveType::SATA => IDE_CMD_IDENTIFY,
	};
	unsafe {
	    let mut cmd_reg = Port::<u8>::new(ctl.io_base + IDE_CMD_REG);
	    cmd_reg.write(cmd);
	}

	unsafe {
	    let mut ctl_reg = Port::<u8>::new(ctl.control_base);
	    ctl_reg.read();
	    ctl_reg.read();
	    ctl_reg.read();
	    ctl_reg.read();
	}

	let mut buf: [u32; 128] = [0; 128];
	for i in 0 .. 128 {
	    unsafe {
		let mut data_reg = Port::<u32>::new(ctl.io_base + IDE_DATA_REG);
		buf[i] = data_reg.read();
	    }
	}

	self.ident = unsafe {
	    ptr::read(buf.as_ptr() as *const IdentifyStruct)
	};
    }
    
    async fn pio_read(&self, offset: u64, size: u64) -> Result<Bytes, syscall::CanonicalError> {
	let ctl = self.controller.lock();
	self.select_drive_and_set_xfer_params(&ctl, offset, size);

	unsafe {
	    let mut cmd_reg = Port::<u8>::new(ctl.io_base + IDE_CMD_REG);
	    cmd_reg.write(IDE_CMD_READ_PIO_EXT);
	}

	for _ in 0 .. 1000000 { unsafe { asm!("nop"); } }
	unsafe {
	    let mut status_reg = Port::<u8>::new(ctl.io_base + IDE_STATUS_REG);

	    loop {
		let status = status_reg.read();
		if status & IDE_STATUS_BSY == 0 {
		    break;
		}
	    }

	    let status = status_reg.read();
	    if status & IDE_STATUS_ERR != 0 {
		let mut err_reg = Port::<u8>::new(ctl.io_base + IDE_ERR_REG);
		let err = err_reg.read();
		panic!("Read failure: {:X}", err);
	    }
	    if status & IDE_STATUS_DRQ == 0 {
		panic!("Read failure");
	    }
	    if status & IDE_STATUS_DF != 0 {
		panic!("Read failure");
	    }
	}

	let buf_ptr = memory::kernel_allocate(
	    size,
	    memory::MemoryAllocationType::RAM,
	    memory::MemoryAllocationOptions::Arbitrary,
	    memory::MemoryAccessRestriction::User)
	    .expect("Unable to allocate heap").0.as_mut_ptr::<u16>();

	let buf_u16 = unsafe {
	    slice::from_raw_parts_mut(buf_ptr, (size / 2) as usize)
	};

	let mut data_reg = Port::<u16>::new(ctl.io_base + IDE_DATA_REG);
	for i in 0 .. (size / 2) {
	    buf_u16[i as usize] = unsafe {
		data_reg.read()
	    };
	}

	// Marshall into a Bytes
	let data_from = unsafe {
	    slice::from_raw_parts(buf_ptr as *const u8, size as usize)
	};
	Ok(bytes::Bytes::from(data_from))
    }

    async fn dma_read(&self, offset: u64, size: u64, access_restriction: memory::MemoryAccessRestriction) -> Result<Bytes, syscall::CanonicalError> {
	let ctl = self.controller.lock();

	// We don't (yet) support multiple PRDs per transfer
	if size > 65536 {
	    panic!("Attempted to transfer more than max size");
	}

	let (buf_virt, buf_phys) = memory::kernel_allocate(
	    size * 512, memory::MemoryAllocationType::DMA, memory::MemoryAllocationOptions::Arbitrary, access_restriction)
	    .expect("Unable to allocate a PDRT memory region");

	let mut compacted_phys_addr = {
	    let mut compacted_phys_addr: vec::Vec<memory::MemoryRegion> = vec::Vec::new();
	    let mut current_start: u64 = 0;
	    for (idx, phys_addr) in buf_phys.iter().enumerate() {
		if idx == 0 {
		    current_start = phys_addr.as_u64();
		} else if phys_addr.as_u64() - 4096 != buf_phys[idx - 1].as_u64() {
		    current_start = phys_addr.as_u64();
		}

		// If the last one, or if the next one isn't contiguous
		if idx == buf_phys.len() - 1 || phys_addr.as_u64() + 4096 != buf_phys[idx + 1].as_u64() {
		    compacted_phys_addr.push(memory::MemoryRegion {
			start: current_start,
			end: phys_addr.as_u64() + 4096,
		    });
		}
	    }

	    compacted_phys_addr
	};

	let total_size: u64 = compacted_phys_addr.iter()
	    .map(|e| e.end - e.start)
	    .sum();
	if total_size != size * 512 {
	    compacted_phys_addr.last_mut().unwrap().end -= total_size - (size * 512);
	}

	ctl.arena.tag_to_slice(ctl.prdt, 4096).fill(0);

	{
	    let mut prdt_entries = 0;
	    let (_, prdts, _) = unsafe {
		ctl.arena.tag_to_slice(ctl.prdt, 4096).align_to_mut::<u32>()
	    };
	    for current_region in compacted_phys_addr.iter() {
		if current_region.end >= 1 << 32 {
		    panic!("DMA region is out of bounds, in higher half of physical memory");
		}

		for i in (current_region.start .. current_region.end).step_by(0x10000) {
		    prdts[prdt_entries * 2] = i as u32;
		    let region_size = current_region.end - i;

		    if region_size >= 0x10000 {
			prdts[(prdt_entries * 2) + 1] = 0;
		    } else {
			prdts[(prdt_entries * 2) + 1] = region_size as u32 & 0xFFFF;
		    }

		    prdt_entries += 1;
		}
	    }
	    prdts[(prdt_entries * 2) - 1] |= 1 << 31;
	}

	let busmaster_base = ctl.busmaster_base.expect("Attempted to do DMA xfer to non-DMA controller") as u16;

	unsafe {
	    let mut prdt_addr_reg = Port::<u32>::new(busmaster_base + 0x04);
	    prdt_addr_reg.write(ctl.prdt_phys);
	}

	unsafe {
	    let mut prdt_command_reg = Port::<u8>::new(busmaster_base + IDE_BUSMASTER_COMMAND_REG);
	    prdt_command_reg.write(IDE_BUSMASTER_COMMAND_READ);
	}

	unsafe {
	    let mut prdt_status_reg = Port::<u8>::new(busmaster_base + IDE_BUSMASTER_STATUS_REG);
	    let mut status = prdt_status_reg.read();
	    status |= IDE_BUSMASTER_STATUS_DMA_ERR | IDE_BUSMASTER_STATUS_INTERRUPT;
	    prdt_status_reg.write(status);
	}

	self.select_drive_and_set_xfer_params(&ctl, offset, size);
	
	unsafe {
	    let mut cmd_reg = Port::<u8>::new(ctl.io_base + IDE_CMD_REG);
	    cmd_reg.write(IDE_CMD_READ_DMA_EXT);
	}

	unsafe {
	    let mut prdt_command_reg = Port::<u8>::new(busmaster_base + IDE_BUSMASTER_COMMAND_REG);
	    prdt_command_reg.write(IDE_BUSMASTER_COMMAND_READ | IDE_BUSMASTER_COMMAND_START);
	}

	// TODO: we wait for the transfer here. We should be using interrupts to do other work, and let
	// the transfer complete asynchronously.
	for _ in 0 .. 1000000 { unsafe { asm!("nop"); } }

	unsafe {
	    let mut status_reg = Port::<u8>::new(ctl.io_base + IDE_STATUS_REG);

	    loop {
		let status = status_reg.read();
		if status & IDE_STATUS_BSY == 0 {
		    break;
		}
	    }

	    let status = status_reg.read();
	    if status & IDE_STATUS_DF != 0 {
		panic!("Read failure");
	    }
	    if status & IDE_STATUS_ERR != 0 {
		let mut err_reg = Port::<u8>::new(ctl.io_base + IDE_ERR_REG);
		let err = err_reg.read();
		panic!("Read failure: {:X}", err);
	    }
	}

	unsafe {
	    let mut status_reg = Port::<u8>::new(busmaster_base + IDE_BUSMASTER_STATUS_REG);
	    // Here
	    loop {
		let status = status_reg.read();
		if status & IDE_BUSMASTER_STATUS_ACTIVE == 0 {
		    break;
		}
	    }

	    let status = status_reg.read();
	    if status & IDE_BUSMASTER_STATUS_DMA_ERR != 0 {
		panic!("DMA error");
	    }
	}

	unsafe {
	    let mut prdt_command_reg = Port::<u8>::new(busmaster_base + IDE_BUSMASTER_COMMAND_REG);
	    prdt_command_reg.write(IDE_BUSMASTER_COMMAND_READ);
	}

	unsafe {
	    let mut prdt_status_reg = Port::<u8>::new(busmaster_base + IDE_BUSMASTER_STATUS_REG);
	    let mut status = prdt_status_reg.read();
	    status |= IDE_BUSMASTER_STATUS_DMA_ERR | IDE_BUSMASTER_STATUS_INTERRUPT;
	    prdt_status_reg.write(status);
	}

	// Marshall into a Bytes
	let data_from = unsafe {
	    slice::from_raw_parts(buf_virt.as_ptr::<u8>(), size as usize)
	};
	
	Ok(bytes::Bytes::from(data_from))
    }

    fn select_drive_and_set_xfer_params(&self, ctl: &MutexGuard<'_, IdeController>, offset: u64, size: u64) {
	self.select(&ctl);

	if self.ident.is_lba48() {
	    unsafe {
		let mut lba3_reg = Port::<u8>::new(ctl.io_base + IDE_REG_LBA0);
		let mut lba4_reg = Port::<u8>::new(ctl.io_base + IDE_REG_LBA1);
		let mut lba5_reg = Port::<u8>::new(ctl.io_base + IDE_REG_LBA2);
		let mut seccount1_reg = Port::<u8>::new(ctl.io_base + IDE_REG_SECCOUNT);

		lba3_reg.write((offset >> 24) as u8);
		lba4_reg.write((offset >> 32) as u8);
		lba5_reg.write((offset >> 40) as u8);
		seccount1_reg.write((size >> 8) as u8);
	    }
	}

	unsafe {
	    let mut lba0_reg = Port::<u8>::new(ctl.io_base + IDE_REG_LBA0);
	    let mut lba1_reg = Port::<u8>::new(ctl.io_base + IDE_REG_LBA1);
	    let mut lba2_reg = Port::<u8>::new(ctl.io_base + IDE_REG_LBA2);
	    let mut seccount0_reg = Port::<u8>::new(ctl.io_base + IDE_REG_SECCOUNT);
	    
	    lba0_reg.write(offset as u8);
	    lba1_reg.write((offset >> 8) as u8);
	    lba2_reg.write((offset >> 16) as u8);
	    seccount0_reg.write(size as u8);
	}
    }
}

pub fn detect_drives(control_base: u16, io_base: u16, busmaster_base: Option<u32>) {
    IdeController::new(control_base, io_base, busmaster_base);
}
