use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ascii;
use core::ptr;
use core::slice;

use crate::sys::block;

enum FatFsType {
    FAT12,
    FAT16,
    FAT32,
    EXFAT,
}

enum Entry {
    FILE(String),
    DIRECTORY(String),
}

#[repr(C, packed(1))]
struct DirectoryEntry {
    file_name: [ascii::Char; 11],
    attributes: u8,
    reserved: u8,
    creation_time_hundredths: u8,
    creation_time: u16,
    creation_date: u16,
    last_accessed_date: u16,
    cluster_high: u16,
    modification_time: u16,
    modification_date: u16,
    cluster_low: u16,
    file_size: u32,
}

#[repr(C, packed(1))]
struct LongFileName {
    order: u8,
    name1: [u16; 5],
    attributes: u8,
    long_entry_type: u8,
    checksum: u8,
    name2: [u16; 6],
    zero: u16,
    name3: [u16; 2],
}

#[repr(C, packed(1))]
#[derive(Copy, Clone)]
struct BootRecord {
    jump: [u8; 3],
    oem_ident: [ascii::Char; 8],
    bytes_per_sector: u16,
    sectors_per_cluster: u8,
    reserved_sectors: u16,
    number_of_fats: u8,
    root_directory_entries: u16,
    sectors_in_volume: u16,
    media_descriptor_type: u8,
    sectors_per_fat: u16,
    sectors_per_track: u16,
    number_of_heads: u16,
    hidden_sectors: u32,
    large_sector_count: u32,
}

#[repr(C, packed(1))]
struct ExtendedBootRecord1216 {
    drive_number: u8,
    reserved: u8,
    signature: u8,
    volume_id: u32,
    volume_label: [ascii::Char; 11],
    system_identifier: [ascii::Char; 8],
    padding: [u8; 448],
    boot_signature: u16,
}

#[repr(C, packed(1))]
struct ExtendedBootRecord32 {
    sectors_per_fat: u32,
    flags: u16,
    version: u16,
    root_dir_cluster: u32,
    fsinfo_sector: u16,
    backup_boot_sector: u16,
    reserved: [u8; 12],
    drive_number: u8,
    reserved_2: u8,
    signature: u8,
    volume_id: [u8; 4],
    volume_label: [ascii::Char; 11],
    system_identifier: [ascii::Char; 8],
    padding: [u8; 420],
    boot_signature: u16,
}

struct Fat12Fs {
    boot_record: BootRecord,
    extended_boot_record: ExtendedBootRecord1216,
}

struct Fat16Fs {
    boot_record: BootRecord,
    extended_boot_record: ExtendedBootRecord1216,
    dev: Arc<block::GptDevice>,
    partition: u32,
}

impl Fat16Fs {
    fn new(dev: Arc<block::GptDevice>, partition: u32, boot_record: BootRecord, extended_boot_record: ExtendedBootRecord1216) -> Option<Fat16Fs> {
	// Check signature, double check this is actually FAT
	if extended_boot_record.signature != 0x28 && extended_boot_record.signature != 0x29 {
	    return None;
	}

	let vol = extended_boot_record.volume_label.iter()
	    .map(|c| c.to_char())
	    .collect::<String>();

	log::info!("Found FAT16 volume {}", vol);

	Some(Fat16Fs {
	    boot_record: boot_record,
	    extended_boot_record: extended_boot_record,
	    dev: dev,
	    partition: partition,
	})
    }

    fn get_dir_entries(&self, path: String) -> Option<Vec<Entry>> {
	if path != "/" {
	    panic!("Attempted to read from non-root directory. Not implemented.");
	}

	let sectors_per_lba = self.boot_record.bytes_per_sector / 512;

	let root_directory_sect = self.boot_record.reserved_sectors +
	    (self.boot_record.number_of_fats as u16 * self.boot_record.sectors_per_fat);
	let root_directory_lba = root_directory_sect / sectors_per_lba;

	let root_directory_size_sectors = ((self.boot_record.root_directory_entries * 32) +
				(self.boot_record.bytes_per_sector - 1)) / self.boot_record.bytes_per_sector;
	let root_directory_size_lba = root_directory_size_sectors / sectors_per_lba;

	let root_dir_buf_ptr = self.dev.read(
	    self.partition, root_directory_lba as u64, root_directory_size_lba as u64).expect("Couldn't read root directory");
	let mut files: Vec<Entry> = Vec::new();

	let mut file_name = String::new();
	for entry in 0 .. self.boot_record.root_directory_entries {
	    let directory_entry = unsafe {
		ptr::read(root_dir_buf_ptr.wrapping_add(entry as usize * 32) as *const DirectoryEntry)
	    };

	    if directory_entry.file_name[0].to_u8() == 0x00 {
		break;
	    } else if directory_entry.file_name[0].to_u8() == 0xE5 {
		continue;
	    }

	    if directory_entry.attributes == 0x0F {
		let long_filename_entry = unsafe {
		    ptr::read(root_dir_buf_ptr.wrapping_add(entry as usize * 32) as *const LongFileName)
		};

		let name1 = long_filename_entry.name1;
		let name2 = long_filename_entry.name2;
		let name3 = long_filename_entry.name3;

		let name1_vec = name1.iter()
		    .map(|i| *i)
		    .filter(|i| *i != 0x0000 && *i != 0xFFFF)
		    .collect::<Vec<u16>>();
		let name2_vec = name2.iter()
		    .map(|i| *i)
		    .filter(|i| *i != 0x0000 && *i != 0xFFFF)
		    .collect::<Vec<u16>>();
		let name3_vec = name3.iter()
		    .map(|i| *i)
		    .filter(|i| *i != 0x0000 && *i != 0xFFFF)
		    .collect::<Vec<u16>>();

		let mut long_filename = String::from_utf16(name1_vec.as_slice()).expect("Malformed filename");
		long_filename.push_str(
		    String::from_utf16(name2_vec.as_slice()).expect("Malformed filename").as_str());
		long_filename.push_str(
		    String::from_utf16(name3_vec.as_slice()).expect("Malformed filename").as_str());
		file_name.push_str(long_filename.as_str());

		continue;
	    }

	    // Volume ID isn't a file
	    if directory_entry.attributes & 0x08 != 0 {
		continue;
	    }

	    if file_name.is_empty() {
		file_name.push_str(
		    directory_entry.file_name[0 .. 8].iter()
			.map(|c| c.to_char())
			.filter(|i| *i != '\0')
			.collect::<String>()
			.as_str());
		file_name.push('.');
		file_name.push_str(
		directory_entry.file_name[8 .. 11].iter()
			.map(|c| c.to_char())
			.filter(|i| *i != '\0')
			.collect::<String>()
			.as_str());
	    }

	    if directory_entry.attributes & 0x10 != 0 {
		log::info!("Found directory /{}/", file_name);
		files.push(Entry::DIRECTORY(file_name));
	    } else {
		log::info!("Found file /{}", file_name);
		files.push(Entry::FILE(file_name));
	    }

	    file_name = String::new();
	}

	Some(files)
    }
}

struct Fat32Fs {
    boot_record: BootRecord,
    extended_boot_record: ExtendedBootRecord32,
}

fn detect_fat_fs(boot_record: BootRecord) -> FatFsType {
    if boot_record.bytes_per_sector == 0 {
	return FatFsType::EXFAT;
    }

    if boot_record.sectors_per_fat == 0 {
	return FatFsType::FAT32;
    }

    let total_sectors = if boot_record.sectors_in_volume == 0 {
	boot_record.large_sector_count
    } else {
	boot_record.sectors_in_volume as u32
    };
    let fat_size = boot_record.sectors_per_fat;
    let root_dir_sectors = ((boot_record.root_directory_entries * 32) + (boot_record.bytes_per_sector - 1)) / boot_record.bytes_per_sector;
    let data_sectors = total_sectors - (boot_record.reserved_sectors + (boot_record.number_of_fats as u16 * fat_size) + root_dir_sectors) as u32;
    let total_clusters = data_sectors / boot_record.sectors_per_cluster as u32;

    if total_clusters < 4085 {
	return FatFsType::FAT12;
    } else if total_clusters < 65525 {
	return FatFsType::FAT16;
    } else {
	return FatFsType::FAT32;
    }
}

pub fn register_fat_fs(dev: Arc<block::GptDevice>, partition: u32) {    
    let boot_record_buf_ptr = dev.read(partition, 0, 1).expect("Read went wrong");
    let boot_record = unsafe {
	ptr::read(boot_record_buf_ptr as *const BootRecord)
    };

    match detect_fat_fs(boot_record) {
	FatFsType::FAT16 => {
	    let extended_boot_record = unsafe {
		ptr::read(boot_record_buf_ptr.wrapping_add(0x24) as *const ExtendedBootRecord1216)
	    };
	    if let Some(fs) = Fat16Fs::new(dev, partition, boot_record, extended_boot_record) {
		fs.get_dir_entries(String::from("/"));
	    }
	},
	_ => (),
    }
}
