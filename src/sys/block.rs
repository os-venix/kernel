use spin::{Once, RwLock};
use alloc::vec::Vec;
use alloc::sync::Arc;
use alloc::string::String;
use core::ptr;
use core::slice;
use core::ascii;
use uuid::Uuid;

use crate::driver;
use crate::fs::fat;

#[repr(C, packed(1))]
#[derive(Copy, Clone)]
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
#[derive(Copy, Clone)]
struct PartitionEntry {
    partition_type_guid: PackedUuid,
    partition_guid: PackedUuid,
    starting_lba: u64,
    ending_lba: u64,
    attributes: u64,
    partition_name: [u16; 36],
}

pub struct GptDevice {
    mbr: Mbr,
    pth: PartitionTableHeader,
    pt: Vec<PartitionEntry>,
    dev: Arc<dyn driver::Device + Send + Sync>,
}

impl GptDevice {
    fn new(dev: Arc<dyn driver::Device + Send + Sync>) -> Option<Arc<GptDevice>> {
	let mbr_buf_ptr = dev.read(0, 1).expect("Read went wrong");
	let mbr = unsafe {
	    ptr::read(mbr_buf_ptr as *const Mbr)
	};

	if mbr.partitions[0].system_id != 0xEE {
	    return None;
	}

	let pth_buf_ptr = dev.read(1, 1).expect("Read went wrong");
	let pth = unsafe {
	    ptr::read(pth_buf_ptr as *const PartitionTableHeader)
	};

	let sig = pth.signature.iter()
	    .map(|c| c.to_char())
	    .collect::<String>();
	if sig != "EFI PART" {
	    return None;
	}

	let pt_size_in_sector_bytes = pth.partition_entry_array_size + (512 - (pth.partition_entry_array_size % 512));  // Total amount, aligned to page boundaries
	let pt_size_in_sectors = pt_size_in_sector_bytes / 512;
	let pt_buf = dev.read(2, pt_size_in_sectors as u64).expect("Could not read Partition Entry table");

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

	let device_arc = Arc::new(GptDevice {
	    mbr: mbr,
	    pth: pth,
	    pt: partition_entries.clone(),
	    dev: dev,
	});

	for partition in 0 .. partition_entries.len() {
	    fat::register_fat_fs(device_arc.clone(), partition as u32);
	}

	Some(device_arc)
    }

    pub fn read(&self, partition: u32, starting_block: u64, size: u64) -> Result<*const u8, ()> {
	if partition as usize >= self.pt.len() {
	    return Err(());
	}

	let pt = self.pt[partition as usize];
	if starting_block >= (pt.ending_lba - pt.starting_lba) {
	    return Err(());
	}

	let adjusted_start = starting_block + pt.starting_lba;
	if adjusted_start + size >= pt.ending_lba {
	    return Err(());
	}

	self.dev.read(adjusted_start, size)
    }
}

unsafe impl Send for GptDevice { }
unsafe impl Sync for GptDevice { }

static BLOCK_DEVICE_TABLE: Once<RwLock<Vec<Arc<GptDevice>>>> = Once::new();

pub fn init() {
    BLOCK_DEVICE_TABLE.call_once(|| RwLock::new(Vec::new()));
}

pub fn register_block_device(dev: Arc<dyn driver::Device + Send + Sync>) {
    if let Some(gpt_device) = GptDevice::new(dev) {
	let mut device_tbl = BLOCK_DEVICE_TABLE.get().expect("Attempted to access device table before it is initialised").write();
	device_tbl.push(gpt_device);
    }
}
