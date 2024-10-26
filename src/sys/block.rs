use spin::{Once, RwLock};
use alloc::vec::Vec;
use alloc::sync::Arc;
use alloc::string::String;
use core::ptr;
use core::slice;
use core::ascii;
use uuid::Uuid;

use crate::driver;

const MBR_PART1_OFFSET: usize = 0x1BE;

const MBR_SYSTEM_ID: usize = 0x04;

static BLOCK_DEVICE_TABLE: Once<RwLock<Vec<Arc<dyn driver::Device + Send + Sync>>>> = Once::new();

#[repr(C, packed(1))]
struct PackedUuid {
    d1: u32,
    d2: u16,
    d3: u16,
    d4: [u8; 8],
}

#[repr(C, packed(1))]
struct MbrEntry {
    boot_indicator: u8,
    starting_head: u8,
    starting_sect: u8,
    starting_cyl: u8,
    system_id: u8,
    ending_head: u8,
    ending_sect: u8,
    ending_cyl: u8,
    total_sectors: u8,
}

#[repr(C, packed(1))]
struct Mbr {
    unused_preamble: [u8; 0x1BE],
    partitions: [MbrEntry; 4],
    boot_sig: u16,
}

#[repr(C, packed(1))]
struct PartitionTableHeader {
    signature: [ascii::Char; 8],
    revision: u32,
    header_size: u32,
    header_checksum: u32,
    reserved: [u8; 4],
    lba_partition_table_header: u64,
    lba_alternate_partition_table_header: u64,
    first_usable_block: u64,
    last_usable_block: u64,
    disk_guid: PackedUuid,
    lba_partition_array_start: u64,
    partition_entries: u32,
    partition_entry_array_size: u32,
    partition_array_checksum: u32,
    reserved_footer: [u8; 0x1A4],
}

#[repr(C, packed(1))]
struct PartitionEntry {
    partition_type_guid: PackedUuid,
    partition_guid: PackedUuid,
    starting_lba: u64,
    ending_lba: u64,
    attributes: u64,
    partition_name: [u16; 36],
}

pub fn init() {
    BLOCK_DEVICE_TABLE.call_once(|| RwLock::new(Vec::new()));
}

pub fn register_block_device(dev: Arc<dyn driver::Device + Send + Sync>) {
    let mbr_buf_ptr = dev.read(1, 1).expect("Read went wrong");
    let mbr = unsafe {
	ptr::read(mbr_buf_ptr as *const Mbr)
    };

    if mbr.partitions[0].system_id == 0xEE {
	log::info!("Found a GPT device");

	let pth_buf_ptr = dev.read(2, 1).expect("Read went wrong");
	let pth = unsafe {
	    ptr::read(pth_buf_ptr as *const PartitionTableHeader)
	};

	let sig = pth.signature.iter()
	    .map(|c| c.to_char())
	    .collect::<String>();
	if sig != "EFI PART" {
	    panic!("Not actually GPT (or the partition table is in a weird place");
	}

	let pt_size_in_sector_bytes = pth.partition_entry_array_size + (512 - (pth.partition_entry_array_size % 512));  // Total amount, aligned to page boundaries
	let pt_size_in_sectors = pt_size_in_sector_bytes / 512;
	let pt_buf = dev.read(3, pt_size_in_sectors as u64).expect("Could not read Partition Entry table");

	let mut partition_entries: Vec<PartitionEntry> = Vec::new();

	for p in 0 .. (pth.partition_entry_array_size / 128) {
	    let partition = unsafe {
		ptr::read(pt_buf.wrapping_add(p as usize * 128) as *const PartitionEntry)
	    };

	    partition_entries.push(partition);

	    let partition_name_utf16 = partition_entries[p as usize].partition_name;
	    let partition_name = String::from_utf16(
		partition_name_utf16.iter()
		    .map(|i| *i)
		    .filter(|i| *i != 0)
		    .collect::<Vec<u16>>()
		    .as_slice())
		.expect("Malformed partition name");
	    let partition_uuid = Uuid::from_fields(
		partition_entries[p as usize].partition_type_guid.d1,
		partition_entries[p as usize].partition_type_guid.d2,
		partition_entries[p as usize].partition_type_guid.d3,
		&partition_entries[p as usize].partition_type_guid.d4);
	    log::info!("Found partition {}, type = {}", partition_name, partition_uuid);
	}
    }

    
}
