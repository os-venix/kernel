use alloc::sync::Arc;
use core::ascii;
use core::ptr;

mod fat1216;

use crate::sys::block;
use crate::vfs;

#[derive(Debug)]
enum FatFsType {
    Fat12,
    Fat16,
    Fat32,
    ExFat,
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
/*
#[repr(C, packed(1))]
#[allow(dead_code)]
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
#[allow(dead_code)]
struct Fat12Fs {
    boot_record: BootRecord,
    extended_boot_record: ExtendedBootRecord1216,
}

#[allow(dead_code)]
struct Fat32Fs {
    boot_record: BootRecord,
    extended_boot_record: ExtendedBootRecord32,
}

*/
fn detect_fat_fs(boot_record: BootRecord) -> FatFsType {
    if boot_record.bytes_per_sector == 0 {
	return FatFsType::ExFat;
    }

    if boot_record.sectors_per_fat == 0 {
	return FatFsType::Fat32;
    }

    let total_sectors = if boot_record.sectors_in_volume == 0 {
	boot_record.large_sector_count
    } else {
	boot_record.sectors_in_volume as u32
    };
    let fat_size = boot_record.sectors_per_fat;
    let root_dir_sectors = (boot_record.root_directory_entries * 32).div_ceil(boot_record.bytes_per_sector);
    let data_sectors = total_sectors - (boot_record.reserved_sectors + (boot_record.number_of_fats as u16 * fat_size) + root_dir_sectors) as u32;
    let total_clusters = data_sectors / boot_record.sectors_per_cluster as u32;

    if total_clusters < 4085 {
	FatFsType::Fat12
    } else if total_clusters < 65525 {
	FatFsType::Fat16
    } else {
	FatFsType::Fat32
    }
}

pub async fn register_fat_fs(dev: Arc<block::GptDevice>, partition: u32) {    
    let boot_record_buf_ptr = dev.read(partition, 0, 1).await.expect("Failed to read (possible) FAT boot record").as_ptr();
    let boot_record = unsafe {
	ptr::read(boot_record_buf_ptr as *const BootRecord)
    };

    match detect_fat_fs(boot_record) {
	FatFsType::Fat16 => {
	    let extended_boot_record = unsafe {
		ptr::read(boot_record_buf_ptr.wrapping_add(0x24) as *const fat1216::ExtendedBootRecord1216)
	    };

	    if let Some(fs) = fat1216::Fat16Fs::new(dev, partition, boot_record, extended_boot_record).await {
		// For now, assume this is root. At some point, root detection should be done properly
		vfs::mount_root(Arc::new(fs)).unwrap();
	    }
	},
	t => {
	    log::info!("{:?}", t);
	},
    }
}
