use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ascii;
use core::ptr;
use spin::RwLock;
use alloc::format;
use bytes;
use alloc::slice;
use futures_util::future::BoxFuture;
use alloc::boxed::Box;

use crate::sys::block;
use crate::sys::syscall;
use crate::sys::vfs;
use crate::memory;

enum FatFsType {
    FAT12,
    FAT16,
    FAT32,
    EXFAT,
}

struct INode {
    file_name: String,
    file_size: u32,
    start_cluster: u32,
}

enum Entry {
    FILE(INode),
    DIRECTORY(INode),
}

#[repr(C, packed(1))]
#[derive(Default, Debug)]
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
    boot_record: RwLock<BootRecord>,
    extended_boot_record: RwLock<ExtendedBootRecord1216>,
    dev: Arc<block::GptDevice>,
    partition: RwLock<u32>,
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

	let root_directory_size_sectors = ((boot_record.root_directory_entries * 32) +
					   (boot_record.bytes_per_sector - 1)) / boot_record.bytes_per_sector;
	let total_sectors = if boot_record.sectors_in_volume != 0 { boot_record.sectors_in_volume as u32 } else { boot_record.large_sector_count };
	let data_sectors = total_sectors - (boot_record.reserved_sectors as u32 + (boot_record.number_of_fats as u32 * boot_record.sectors_per_fat as u32) + root_directory_size_sectors as u32);
	
	let total_clusters = data_sectors / boot_record.sectors_per_cluster as u32;

	if boot_record.sectors_per_fat != 0 &&
	    total_clusters >= 4085 &&
	    total_clusters <= 65525 {
		log::info!("Found FAT16 volume {}", vol);
	    } else {
		log::info!("Not FAT16");
		return None;
	    }

	Some(Fat16Fs {
	    boot_record: RwLock::new(boot_record),
	    extended_boot_record: RwLock::new(extended_boot_record),
	    dev: dev,
	    partition: RwLock::new(partition),
	})
    }

    async fn get_root_directory(&self) -> bytes::Bytes {
	let inner = {
	    let boot_record = self.boot_record.read();
	    let partition = self.partition.read();

	    let sectors_per_lba = boot_record.bytes_per_sector / 512;

	    let root_directory_sect = boot_record.reserved_sectors +
		(boot_record.number_of_fats as u16 * boot_record.sectors_per_fat);
	    let root_directory_lba = root_directory_sect * sectors_per_lba;

	    let root_directory_size_sectors = ((boot_record.root_directory_entries * 32) +
					       (boot_record.bytes_per_sector - 1)) / boot_record.bytes_per_sector;
	    let root_directory_size_lba = root_directory_size_sectors / sectors_per_lba;

	    self.dev.read(
		*partition, root_directory_lba as u64, root_directory_size_lba as u64, memory::MemoryAccessRestriction::Kernel)
	};

	inner.await.expect("Couldn't read root directory")
    }

    async fn get_allocation_table(&self) -> Vec<u16> {
	let inner = {
	    let boot_record = self.boot_record.read();
	    let partition = self.partition.read();

	    let sectors_per_lba = boot_record.bytes_per_sector / 512;

	    let fat_lba = 1 * sectors_per_lba;

	    let fat_size_sectors = boot_record.sectors_per_fat;
	    let fat_size_lba = fat_size_sectors / sectors_per_lba;

	    self.dev.read(
		*partition, fat_lba as u64, fat_size_lba as u64, memory::MemoryAccessRestriction::Kernel)
	};

	let fat_buf_ptr = inner.await.expect("Couldn't read FAT").as_ptr();
	let mut table: Vec<u16> = Vec::new();

	{
	    let boot_record = self.boot_record.read();
	    for entry in 0 .. boot_record.sectors_per_fat * boot_record.bytes_per_sector / 2 {
		unsafe {
		    table.push(
			ptr::read(fat_buf_ptr.wrapping_add(entry as usize * 2) as *const u16)
		    );
		}
	    }
	}

	table
    }

    fn get_filename(&self, dir: *const u8, index: usize) -> (Option<String>, usize) {
	let mut file_name = String::new();
	let mut cnt = 0;
	let boot_record = self.boot_record.read();

	for entry in index .. boot_record.root_directory_entries as usize {
	    let directory_entry = unsafe {
		ptr::read(dir.wrapping_add(entry as usize * 32) as *const DirectoryEntry)
	    };

	    if directory_entry.file_name[0].to_u8() == 0x00 {
		return (None, cnt);
	    } else if directory_entry.file_name[0].to_u8() == 0xE5 {
		return (None, cnt);
	    }

	    if directory_entry.attributes == 0x0F {
		let long_filename_entry = unsafe {
		    ptr::read(dir.wrapping_add(entry as usize * 32) as *const LongFileName)
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

		cnt += 1;

		continue;
	    }

	    // Volume ID isn't a file
	    if directory_entry.attributes & 0x08 != 0 {
		return (None, cnt);
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

	    return (Some(file_name), cnt)
	}

	(None, cnt)
    }

    async fn get_dir_entries(&self, path: String) -> Option<Vec<Entry>> {
	if path != "/" {
	    panic!("Attempted to read from non-root directory. Not implemented.");
	}

	let root_dir_buf = self.get_root_directory().await;
	let root_directory_entries = {
	    self.boot_record.read().root_directory_entries
	};

	let mut ptr = 0;
	let mut files: Vec<Entry> = Vec::new();
	while ptr < root_directory_entries {
	    let (maybe_fn, offset) = self.get_filename(root_dir_buf.as_ptr(), ptr as usize);
	    ptr += offset as u16;

	    if let Some(file_name) = maybe_fn {
		let directory_entry = unsafe {
		    ptr::read(root_dir_buf.as_ptr().wrapping_add(ptr as usize * 32) as *const DirectoryEntry)
		};

		if directory_entry.attributes & 0x10 != 0 {
		    files.push(Entry::DIRECTORY(INode {
			file_name: file_name,
			file_size: directory_entry.file_size,
			start_cluster: directory_entry.cluster_low as u32,
		    }));
		} else {
		    files.push(Entry::FILE(INode {
			file_name: file_name,
			file_size: directory_entry.file_size,
			start_cluster: directory_entry.cluster_low as u32,
		    }));
		}

		ptr += 1;
	    } else {
		let directory_entry = unsafe {
		    ptr::read(root_dir_buf.as_ptr().wrapping_add(ptr as usize * 32) as *const DirectoryEntry)
		};

		if directory_entry.file_name[0].to_u8() == 0x00 {
		    break;
		} else if directory_entry.file_name[0].to_u8() == 0xE5 {
		    ptr += 1;
		    continue;
		} else if directory_entry.attributes & 0x08 != 0 {
		    ptr += 1;
		    continue;
		}
	    }
	}

	Some(files)
    }

    fn find_dir_entry(&self, path: String, dir_buf_ptr: *const u8) -> Option<Entry> {
	let mut ptr = 0;
	loop {
	    let (maybe_fn, offset) = self.get_filename(dir_buf_ptr, ptr as usize);
	    ptr += offset as u16;

	    if let Some(file_name) = maybe_fn {
		if file_name != path {
		    ptr += 1;
		    continue;
		}

		let directory_entry = unsafe {
		    ptr::read(dir_buf_ptr.wrapping_add(ptr as usize * 32) as *const DirectoryEntry)
		};

		if directory_entry.attributes & 0x10 != 0 {
		    return Some(Entry::DIRECTORY(INode {
			file_name: file_name,
			file_size: directory_entry.file_size,
			start_cluster: directory_entry.cluster_low as u32,
		    }));
		} else {
		    return Some(Entry::FILE(INode {
			file_name: file_name,
			file_size: directory_entry.file_size,
			start_cluster: directory_entry.cluster_low as u32,
		    }));
		}
	    } else {
		let directory_entry = unsafe {
		    ptr::read(dir_buf_ptr.wrapping_add(ptr as usize * 32) as *const DirectoryEntry)
		};

		if directory_entry.file_name[0].to_u8() == 0x00 {
		    break;
		} else if directory_entry.file_name[0].to_u8() == 0xE5 {
		    ptr += 1;
		    continue;
		} else if directory_entry.attributes & 0x08 != 0 {
		    ptr += 1;
		    continue;
		}
	    }
	}

	None
    }
}

impl vfs::FileSystem for Fat16Fs {
    fn ioctl(&self, path: String, ioctl: u64) -> Result<(bytes::Bytes, usize, u64), ()> {
	// ioctls are devices only
	Err(())
    }

    fn read(self: Arc<Self>, path: String, offset: u64, len: u64) -> BoxFuture<'static, Result<bytes::Bytes, syscall::CanonicalError>> {
	Box::pin(async move {
	    let parts = path.split("/")
		.filter(|s| s.len() != 0)
		.collect::<Vec<&str>>();

	    let mut current_buf_ptr = self.get_root_directory().await.as_ptr();
	    let mut file_size: usize = 0 as usize;

	    for path_part in parts {
		let (inode, access) = match self.find_dir_entry(path_part.to_string(), current_buf_ptr)
		    .expect(&format!("Could not find file {}", path)) {
			Entry::DIRECTORY(i) => (i, memory::MemoryAccessRestriction::Kernel),
			Entry::FILE(i) => (i, memory::MemoryAccessRestriction::User),
		    };

		// This is FAT16, no high cluster
		let mut cluster = inode.start_cluster as u16;
		let fat = self.get_allocation_table().await;

		let mut clusters_to_read: Vec<u16> = Vec::new();
		clusters_to_read.push(cluster);

		loop {
		    cluster = fat[cluster as usize];

		    if cluster >= 0xFFF8 {
			break;
		    }

		    clusters_to_read.push(cluster);
		}

		let mut cluster_strings_to_read: Vec<(u16, u64)> = Vec::new();
		let mut current_start: u16 = 0;
		for (idx, entry) in clusters_to_read.iter().enumerate() {
		    if idx == 0 {
			current_start = *entry;
		    } else if *entry != (clusters_to_read[idx - 1] + 1) {
			current_start = *entry;
		    }

		    if idx == clusters_to_read.len() - 1 {
			cluster_strings_to_read.push((current_start, (*entry as u64 - current_start as u64) + 1));
		    } else if (*entry + 1) != clusters_to_read[idx + 1] {
			cluster_strings_to_read.push((current_start, (*entry as u64 - current_start as u64) + 1));
		    }
		}

		if cluster_strings_to_read.len() == 1 {
		    let (cluster_lba, size_lba) = {
			let boot_record = self.boot_record.read();
			let sectors_per_lba = boot_record.bytes_per_sector as u64 / 512;

			let root_directory_size_sectors: u64 = ((boot_record.root_directory_entries as u64 * 32) +
								(boot_record.bytes_per_sector as u64 - 1))
			    / boot_record.bytes_per_sector as u64;

			let first_data_sector: u64 = boot_record.reserved_sectors as u64 +
			    (boot_record.number_of_fats as u64 * boot_record.sectors_per_fat as u64) +
			    root_directory_size_sectors as u64;

			let cluster_sector: u64 = ((cluster_strings_to_read[0].0 as u64 - 2) * boot_record.sectors_per_cluster as u64)
			    + first_data_sector;

			let cluster_lba = cluster_sector * sectors_per_lba;

			let size_sectors = 1 * boot_record.sectors_per_cluster as u64;
			(cluster_lba, size_sectors / sectors_per_lba)
		    };

		    let partition = {
			let partition = self.partition.read();
			*partition
		    };

		    current_buf_ptr = self.dev.read(
			partition, cluster_lba as u64,
			(size_lba as u64) * cluster_strings_to_read[0].1, access).await.expect("Couldn't read file")
			.as_ptr();

		    file_size = inode.file_size as usize;
		} else {
		    // Not supported
		    panic!("More than one cluster string attempted to be loaded: {:?}", cluster_strings_to_read);
		    //		return Err(());
		}
	    }

	    // Marshall into a Bytes
	    let data_from = unsafe {
		slice::from_raw_parts(current_buf_ptr, file_size)
	    };
	    Ok(bytes::Bytes::from_static(&data_from[offset as usize .. (offset + len) as usize]))
	})
    }
	
    fn write(&self, path: String, buf: *const u8, len: usize) -> Result<u64, ()> {
	panic!("FAT write not yet implemented");
    }

    fn stat(self: Arc<Self>, path: String) -> BoxFuture<'static, Result<vfs::Stat, ()>> {
	Box::pin(async move {
	    let parts = path.split("/")
		.filter(|s| s.len() != 0)
		.collect::<Vec<&str>>();

	    let mut current_buf_ptr = self.get_root_directory().await.as_ptr();
	    let mut file_size: usize = 0 as usize;

	    for path_part in parts {
		let inode = match self.find_dir_entry(path_part.to_string(), current_buf_ptr).ok_or(())? {
		    Entry::DIRECTORY(i) => i,
		    Entry::FILE(i) => {
			file_size = i.file_size as usize;
			break;
		    },
		};

		// This is FAT16, no high cluster
		let mut cluster = inode.start_cluster as u16;
		let fat = self.get_allocation_table().await;

		let mut clusters_to_read: Vec<u16> = Vec::new();
		clusters_to_read.push(cluster);

		loop {
		    cluster = fat[cluster as usize];

		    if cluster >= 0xFFF8 {
			break;
		    }

		    clusters_to_read.push(cluster);
		}

		let mut cluster_strings_to_read: Vec<(u16, u64)> = Vec::new();
		let mut current_start: u16 = 0;
		for (idx, entry) in clusters_to_read.iter().enumerate() {
		    if idx == 0 {
			current_start = *entry;
		    } else if *entry != (clusters_to_read[idx - 1] + 1) {
			current_start = *entry;
		    }

		    if idx == clusters_to_read.len() - 1 {
			cluster_strings_to_read.push((current_start, (*entry as u64 - current_start as u64) + 1));
		    } else if (*entry + 1) != clusters_to_read[idx + 1] {
			cluster_strings_to_read.push((current_start, (*entry as u64 - current_start as u64) + 1));
		    }
		}

		if cluster_strings_to_read.len() == 1 {
		    let (cluster_lba, size_lba) = {
			let boot_record = self.boot_record.read();
			let sectors_per_lba = boot_record.bytes_per_sector as u64 / 512;

			let root_directory_size_sectors: u64 = ((boot_record.root_directory_entries as u64 * 32) +
								(boot_record.bytes_per_sector as u64 - 1))
			    / boot_record.bytes_per_sector as u64;

			let first_data_sector: u64 = boot_record.reserved_sectors as u64 +
			    (boot_record.number_of_fats as u64 * boot_record.sectors_per_fat as u64) +
			    root_directory_size_sectors as u64;

			let cluster_sector: u64 = ((cluster_strings_to_read[0].0 as u64 - 2) * boot_record.sectors_per_cluster as u64)
			    + first_data_sector;

			let cluster_lba = cluster_sector * sectors_per_lba;

			let size_sectors = 1 * boot_record.sectors_per_cluster as u64;
			(cluster_lba, size_sectors / sectors_per_lba)
		    };

		    let partition = {
			let partition = self.partition.read();
			*partition
		    };

		    current_buf_ptr = self.dev.read(
			partition, cluster_lba as u64,
			(size_lba as u64) * cluster_strings_to_read[0].1,
			memory::MemoryAccessRestriction::Kernel).await.expect("Couldn't read file")
			.as_ptr();
		} else {
		    // Not supported
		    panic!("More than one cluster string attempted to be loaded: {:?}", cluster_strings_to_read);
		    //		return Err(());
		}
	    }

	    Ok(vfs::Stat {
		file_name: path,
		size: Some(file_size as u64),
	    })
	})
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

pub async fn register_fat_fs(dev: Arc<block::GptDevice>, partition: u32) {    
    let boot_record_buf_ptr = dev.read(partition, 0, 1, memory::MemoryAccessRestriction::Kernel).await.expect("Failed to read (possible) FAT boot record").as_ptr();
    let boot_record = unsafe {
	ptr::read(boot_record_buf_ptr as *const BootRecord)
    };

    match detect_fat_fs(boot_record) {
	FatFsType::FAT16 => {
	    let extended_boot_record = unsafe {
		ptr::read(boot_record_buf_ptr.wrapping_add(0x24) as *const ExtendedBootRecord1216)
	    };

	    if let Some(fs) = Fat16Fs::new(dev, partition, boot_record, extended_boot_record) {
		vfs::mount("/".to_string(), Arc::new(fs));
	    }
	},
	_ => (),
    }
}
