use alloc::string::String;
use core::arch::asm;
use core::ascii;
use x86_64::instructions::port::Port;
use itertools::Itertools;
use alloc::vec;
use core::ptr;
use bit_field::BitField;

const IDE_CTL_REG: u16 = 0;
const IDE_CTL_NIEN: u8 = 1 << 1;
const IDE_CTL_SRST: u8 = 1 << 2;

const IDE_DRIVE_HEAD_REG: u16 = 6;
const IDE_DRIVE_HEAD_BASE: u8 = 0xA0;  // LBA, + always set bits
const IDE_DRIVE_HEAD_DRIVE_SEL_PRIMARY: u8 = 0;
const IDE_DRIVE_HEAD_DRIVE_SEL_SECONDARY: u8 = 1 << 4;

const IDE_CMD_REG: u16 = 7;
const IDE_CMD_IDENTIFY: u8 = 0xEC;
const IDE_CMD_PACKET_IDENTIFY: u8 = 0xA1;

const IDE_STATUS_REG: u16 = 7;
const IDE_STATUS_ERR: u8 = 1;
const IDE_STATUS_RDY: u8 = 1 << 6;
const IDE_STATUS_BSY: u8 = 1 << 7;

const IDE_REG_CYL_LO: u16 = 4;
const IDE_REG_CYL_HI: u16 = 5;

const IDE_DATA_REG: u16 = 0;

const IDE_IDENT_MODEL: usize = 54;

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
}

#[derive(Debug)]
enum DriveType {
    ATA,
    ATAPI,
    SATA,
    SATAPI,
}

struct IdeController {
    control_base: u16,
    io_base: u16,
    drive_0: Option<IdeDrive>,
    drive_1: Option<IdeDrive>,
}

impl IdeController {
    pub fn new(control_base: u16, io_base: u16) -> IdeController {
	let mut ide_controller = IdeController {
	    control_base: control_base,
	    io_base: io_base,
	    drive_0: None,
	    drive_1: None,
	};

	ide_controller.reset();
	if let Some(ide_drive) = IdeDrive::new(control_base, io_base, 0) {
	    let model = ide_drive.ident.get_model();
	    let size = ide_drive.ident.get_size_in_sectors();
	    log::info!("Drive 0: {} - {} MiB", model, size / (1024 * 2));

	    ide_controller.drive_0 = Some(ide_drive);
	}
	if let Some(ide_drive) = IdeDrive::new(control_base, io_base, 1) {
	    let model = ide_drive.ident.get_model();
	    let size = ide_drive.ident.get_size_in_sectors();
	    log::info!("Drive 1: {} - {} MiB", model, size / (1024 * 2));

	    ide_controller.drive_1 = Some(ide_drive);
	}

	ide_controller
    }

    fn reset(&self) {
	// Reset the drive controller
	unsafe {
	    let mut ctl_reg = Port::<u8>::new(self.control_base + IDE_CTL_REG);
	    ctl_reg.write(IDE_CTL_NIEN | IDE_CTL_SRST);
	}
	for i in 0 .. 1000000 { unsafe { asm!("nop"); } }
	unsafe {
	    let mut ctl_reg = Port::<u8>::new(self.control_base + IDE_CTL_REG);
	    ctl_reg.write(IDE_CTL_NIEN);
	}
	for i in 0 .. 1000000 { unsafe { asm!("nop"); } }
    }
}

struct IdeDrive {
    control_base: u16,
    io_base: u16,
    drive_num: u8,
    ident: IdentifyStruct,
    drive_type: DriveType,
}

impl IdeDrive {
    pub fn new(control_base: u16, io_base: u16, drive_num: u8) -> Option<IdeDrive> {
	let mut ide_drive = IdeDrive {
	    control_base: control_base,
	    io_base: io_base,
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

    fn select(&self) {
	// TODO: make port a shared, locked resource
	let select_cmd = IDE_DRIVE_HEAD_BASE | if self.drive_num == 0 {
	    IDE_DRIVE_HEAD_DRIVE_SEL_PRIMARY
	} else {
	    IDE_DRIVE_HEAD_DRIVE_SEL_SECONDARY
	};
	unsafe {
	    let mut drive_head_reg = Port::<u8>::new(self.io_base + IDE_DRIVE_HEAD_REG);
	    drive_head_reg.write(select_cmd);
	}
	for i in 0 .. 1000000 { unsafe { asm!("nop"); } }
    }

    fn check_exists_and_set_type(&mut self) -> bool {
	self.select();
	unsafe {
	    let mut cmd_reg = Port::<u8>::new(self.io_base + IDE_CMD_REG);
	    cmd_reg.write(IDE_CMD_IDENTIFY);
	}
	for i in 0 .. 1000000 { unsafe { asm!("nop"); } }

	unsafe {
	    let mut status_reg = Port::<u8>::new(self.io_base + IDE_STATUS_REG);
	    // No drive detected
	    if status_reg.read() == 0 {
		return false;
	    }

	    loop {
		let status = status_reg.read();
		if status & IDE_STATUS_ERR == 1 {
		    unsafe {
			let mut reg_cyl_lo = Port::<u8>::new(self.io_base + IDE_REG_CYL_LO);
			let mut reg_cyl_hi = Port::<u8>::new(self.io_base + IDE_REG_CYL_HI);

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
			}
		    }

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
	self.select();

	let cmd = match self.drive_type {
	    DriveType::ATAPI => IDE_CMD_PACKET_IDENTIFY,
	    DriveType::SATAPI => IDE_CMD_PACKET_IDENTIFY,
	    DriveType::ATA => IDE_CMD_IDENTIFY,
	    DriveType::SATA => IDE_CMD_IDENTIFY,
	};
	unsafe {
	    let mut cmd_reg = Port::<u8>::new(self.io_base + IDE_CMD_REG);
	    cmd_reg.write(cmd);
	}

	unsafe {
	    let mut ctl_reg = Port::<u8>::new(self.control_base);
	    ctl_reg.read();
	    ctl_reg.read();
	    ctl_reg.read();
	    ctl_reg.read();
	}

	let mut buf: [u32; 128] = [0; 128];
	for i in 0 .. 128 {
	    unsafe {
		let mut data_reg = Port::<u32>::new(self.io_base + IDE_DATA_REG);
		buf[i] = data_reg.read();
	    }
	}

	self.ident = unsafe {
	    ptr::read(buf.as_ptr() as *const IdentifyStruct)
	};
    }
}

pub fn detect_drives(control_base: u16, io_base: u16) {
    IdeController::new(control_base, io_base);
}
