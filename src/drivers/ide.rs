use alloc::boxed::Box;
use alloc::string::String;
use core::arch::asm;
use x86_64::instructions::port::Port;

use crate::drivers::pcie;
use crate::driver;

const IDE_CTL_REG: u16 = 0;
const IDE_CTL_NIEN: u8 = 1 << 1;
const IDE_CTL_SRST: u8 = 1 << 2;

const IDE_DRIVE_HEAD_REG: u16 = 6;
const IDE_DRIVE_HEAD_BASE: u8 = 0xA0;  // LBA, + always set bits
const IDE_DRIVE_HEAD_DRIVE_SEL_PRIMARY: u8 = 0;
const IDE_DRIVE_HEAD_DRIVE_SEL_SECONDARY: u8 = 1 << 4;

const IDE_CMD_REG: u16 = 7;
const IDE_CMD_IDENTIFY: u8 = 0xEC;

const IDE_STATUS_REG: u16 = 7;
const IDE_STATUS_ERR: u8 = 1;
const IDE_STATUS_RDY: u8 = 1 << 6;
const IDE_STATUS_BSY: u8 = 1 << 7;

const IDE_REG_CYL_LO: u16 = 4;
const IDE_REG_CYL_HI: u16 = 5;

const IDE_DATA_REG: u16 = 0;

const IDE_IDENT_MODEL: usize = 54;

pub fn init() {
    let ide_driver = IdeDriver {};
    driver::register_driver(Box::new(ide_driver));
}

fn detect_drives(control_base: u16, io_base: u16) {
    // Reset the drive controller
    unsafe {
	let mut ctl_reg = Port::<u8>::new(control_base + IDE_CTL_REG);
	ctl_reg.write(IDE_CTL_NIEN | IDE_CTL_SRST);
    }
    for i in 0 .. 1000000 { unsafe { asm!("nop"); } }
    unsafe {
	let mut ctl_reg = Port::<u8>::new(control_base + IDE_CTL_REG);
	ctl_reg.write(IDE_CTL_NIEN);
    }
    for i in 0 .. 1000000 { unsafe { asm!("nop"); } }

    'outer: for drive in 0 ..= 1 {
	unsafe {
	    let mut drive_head_reg = Port::<u8>::new(io_base + IDE_DRIVE_HEAD_REG);
	    drive_head_reg.write(IDE_DRIVE_HEAD_BASE | if drive == 0 { IDE_DRIVE_HEAD_DRIVE_SEL_PRIMARY } else { IDE_DRIVE_HEAD_DRIVE_SEL_SECONDARY });
	}
	for i in 0 .. 1000000 { unsafe { asm!("nop"); } }

	unsafe {
	    let mut cmd_reg = Port::<u8>::new(io_base + IDE_CMD_REG);
	    cmd_reg.write(IDE_CMD_IDENTIFY);
	}
	for i in 0 .. 1000000 { unsafe { asm!("nop"); } }

	unsafe {
	    let mut status_reg = Port::<u8>::new(io_base + IDE_STATUS_REG);
	    // No drive detected
	    if status_reg.read() == 0 {
		log::info!("No drive detected");
		continue;
	    }
	    loop {
		let status = status_reg.read();
		if status & IDE_STATUS_ERR == 1 {
		    log::info!("Drive {} encountered error.", drive);
		    continue 'outer;
		}

		if (status & IDE_STATUS_BSY == 0) &&
		    (status & IDE_STATUS_RDY != 0) {
			break;
		    }
	    }
	}

	for i in 0 .. 1000000 { unsafe { asm!("nop"); } }
	unsafe {
	    let mut reg_cyl_lo = Port::<u8>::new(io_base + IDE_REG_CYL_LO);
	    let mut reg_cyl_hi = Port::<u8>::new(io_base + IDE_REG_CYL_HI);

	    let cl = reg_cyl_lo.read();
	    let ch = reg_cyl_hi.read();

	    if cl == 0x14 && ch == 0xEB {
		log::info!("Drive {} is PATAPI", drive);
	    } else if cl == 0x69 && ch == 0x96 {
		log::info!("Drive {} is SATAPI", drive);
	    } else if cl == 0 && ch == 0 {
		log::info!("Drive {} is PATA", drive);
	    } else if cl == 0x3C && ch == 0xC3 {
		log::info!("Drive {} is SATA", drive);
	    } else {
		log::info!("Unrecognised drive type {}:{} for drive {}", cl, ch, drive);
	    }
	}

	unsafe {
	    let mut cmd_reg = Port::<u8>::new(io_base + IDE_CMD_REG);
	    cmd_reg.write(IDE_CMD_IDENTIFY);
	}
	for i in 0 .. 1000000 { unsafe { asm!("nop"); } }

	let mut buf: [u8; 512] = [0; 512];
	for i in 0 .. 128 {
	    unsafe {
		let mut data_reg = Port::<u32>::new(io_base + IDE_DATA_REG);
		let quad = data_reg.read();

		buf[(i * 4) + 3] = ((quad & 0xFF00_0000) >> 24) as u8;
		buf[(i * 4) + 2] = ((quad & 0x00FF_0000) >> 16) as u8;
		buf[(i * 4) + 1] = ((quad & 0x0000_FF00) >> 8) as u8;
		buf[(i * 4) + 0] = (quad & 0x0000_00FF) as u8;
	    }
	}

	let mut model = String::new();
	for k in 0 .. 20 {
	    model.push(buf[IDE_IDENT_MODEL + (k * 2) + 1] as char);
	    model.push(buf[IDE_IDENT_MODEL + (k * 2)] as char);
	}

	log::info!("{}", model);
    }
}

pub struct IdeDriver {}
impl driver::Driver for IdeDriver {
    fn init(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) {
	let (address, interface) = if let Some(pci_info) = info.as_any().downcast_ref::<pcie::PciDeviceType>() {
	    (pci_info.address, pci_info.interface)
	} else {
	    return;
	};

	// For now, we only support compatibility mode
	if (interface & 1) == 1 {
	    unimplemented!();
	}

	let control_primary_base = 0x3F6;
	let control_secondary_base = 0x376;
	let io_primary_base = 0x1F0;
	let io_secondary_base = 0x170;

	log::info!("Primary IDE Bus:");
	detect_drives(control_primary_base, io_primary_base);
	log::info!("Secondary IDE Bus:");
	detect_drives(control_secondary_base, io_secondary_base);
    }

    fn check_device(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	if let Some(pci_info) = info.as_any().downcast_ref::<pcie::PciDeviceType>() {
	    pci_info.base_class == 1 &&
		pci_info.sub_class == 1
	} else {
	    false
	}
    }

    fn check_new_device(&self, info: &Box<dyn driver::DeviceTypeIdentifier>) -> bool {
	true // Not yet implemented
    }
}
