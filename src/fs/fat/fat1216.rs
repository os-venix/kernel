use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ascii;
use core::ptr;
use spin::RwLock;
use bytes::{Bytes, BytesMut};
use futures_util::future::BoxFuture;
use core::sync::atomic::AtomicU64;
use futures_util::FutureExt;
use alloc::borrow::ToOwned;
use core::sync::atomic::Ordering;

use crate::sys::block;
use crate::sys::syscall;
use crate::vfs;
use crate::sys::ioctl;
use crate::fs::fat::BootRecord;
use crate::syscall::CanonicalError;

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
pub struct ExtendedBootRecord1216 {
    drive_number: u8,
    reserved: u8,
    signature: u8,
    volume_id: u32,
    volume_label: [ascii::Char; 11],
    system_identifier: [ascii::Char; 8],
    padding: [u8; 448],
    boot_signature: u16,
}

struct INode {
    file_name: String,
    file_size: u32,
    start_cluster: u32,
    kind: vfs::filesystem::VNodeKind,
    fs: Arc<Fat16Fs>,
    fsi: vfs::filesystem::FileSystemInstance,
    parent: Option<Arc<dyn vfs::filesystem::VNode>>,
}

impl vfs::filesystem::VNode for INode {
    fn inode(&self) -> u64 {
	self.start_cluster as u64
    }

    fn kind(&self) -> vfs::filesystem::VNodeKind {
	self.kind
    }

    fn stat(&self) -> Result<vfs::filesystem::Stat, CanonicalError> {
	Ok(vfs::filesystem::Stat {
	    file_name: self.file_name.clone(),
	    size: Some(self.file_size as u64),
	})
    }

    fn open(self: Arc<Self>/*, flags: OpenFlags */) -> Result<Arc<dyn vfs::filesystem::FileHandle>, CanonicalError> {
	let mut clusters_to_read: Vec<u32> = Vec::new();

	{
	    let fat = self.fs.fat.read();
	    let mut cluster = self.start_cluster;
	    loop {
		clusters_to_read.push(cluster);
		cluster = fat[cluster as usize] as u32;

		if cluster >= 0xFFF8 {
		    break;
		}
	    }
	}

	let mut sector_lba_to_read: Vec<u64> = Vec::new();
	for cluster in clusters_to_read.iter() {
	    let (cluster_lba, size_lba) = {
		let boot_record = self.fs.boot_record.read();
		let sectors_per_lba = boot_record.bytes_per_sector as u64 / 512;

		let root_directory_size_sectors: u64 = (boot_record.root_directory_entries as u64 * 32).div_ceil(boot_record.bytes_per_sector as u64);

		let first_data_sector: u64 = boot_record.reserved_sectors as u64 +
		    (boot_record.number_of_fats as u64 * boot_record.sectors_per_fat as u64) +
		    root_directory_size_sectors;

		let cluster_sector: u64 = ((*cluster as u64 - 2) * boot_record.sectors_per_cluster as u64)
		    + first_data_sector;

		let cluster_lba = cluster_sector * sectors_per_lba;

		let size_sectors = boot_record.sectors_per_cluster as u64;
		(cluster_lba, size_sectors * sectors_per_lba)
	    };

	    for i in 0 .. size_lba {
		sector_lba_to_read.push(cluster_lba + i);
	    }
	}

	Ok(Arc::new(FatFileHandle::new(
	    self.clone(),  // inode
	    sector_lba_to_read,
	    self.fs.dev.clone(),
	    self.fs.partition,
	    512,
	)))
    }

    fn filesystem(&self) -> Arc<dyn vfs::filesystem::FileSystem> {
	self.fs.clone()
    }

    fn fsi(&self) -> vfs::filesystem::FileSystemInstance {
	self.fsi
    }

    fn parent(&self) -> Result<Arc<dyn vfs::filesystem::VNode>, CanonicalError> {
	if let Some(parent) = self.parent.clone() {
	    Ok(parent)
	} else {
	    Err(CanonicalError::NoEnt)
	}
    }

    fn set_fsi(self: Arc<Self>, _fsi: vfs::filesystem::FileSystemInstance) {
	unimplemented!();
    }
}

struct RootINode {
    root_directory_block: u64,
    root_directory_size: u64,
    fs: Arc<Fat16Fs>,
    fsi: vfs::filesystem::FileSystemInstance,
}

impl vfs::filesystem::VNode for RootINode {
    fn inode(&self) -> u64 {
	0xFFFF_FFFF_FFFF_FFFF
    }

    fn kind(&self) -> vfs::filesystem::VNodeKind {
	vfs::filesystem::VNodeKind::Directory
    }

    fn stat(&self) -> Result<vfs::filesystem::Stat, CanonicalError> {
	Ok(vfs::filesystem::Stat {
	    file_name: String::from("/"),
	    size: Some(self.root_directory_size * 512),
	})
    }

    fn open(self: Arc<Self>/*, flags: OpenFlags */) -> Result<Arc<dyn vfs::filesystem::FileHandle>, CanonicalError> {
	Ok(Arc::new(FatFileHandle::new(
	    self.clone(),  // inode
	    (self.root_directory_block .. (self.root_directory_block + self.root_directory_size))
		.collect::<Vec<u64>>(),  // block_list
	    self.fs.dev.clone(),
	    self.fs.partition,
	    512,
	)))
    }

    fn filesystem(&self) -> Arc<dyn vfs::filesystem::FileSystem> {
	self.fs.clone()
    }

    fn fsi(&self) -> vfs::filesystem::FileSystemInstance {
	self.fsi
    }

    fn parent(&self) -> Result<Arc<dyn vfs::filesystem::VNode>, CanonicalError> {
	Err(CanonicalError::NoEnt)
    }

    fn set_fsi(self: Arc<Self>, _fsi: vfs::filesystem::FileSystemInstance) {
	unimplemented!();
    }
}

struct FatFileHandle {
    inode: Arc<dyn vfs::filesystem::VNode>,
    block_list: Vec<u64>,
    dev: Arc<block::GptDevice>,
    partition: u32,
    current_offset: AtomicU64,
    block_size: u64,
}

impl FatFileHandle {
    pub fn new(
        vnode: Arc<dyn vfs::filesystem::VNode>,
        block_list: Vec<u64>,
        dev: Arc<block::GptDevice>,
        partition: u32,
	block_size: u64,
    ) -> Self {
        Self {
            inode: vnode,
            block_list,
            dev,
            partition,
            current_offset: AtomicU64::new(0),
	    block_size,
        }
    }
}

impl vfs::filesystem::FileHandle for FatFileHandle {
    fn read(self: Arc<Self>, len: u64) -> BoxFuture<'static, Result<bytes::Bytes, CanonicalError>> {
	let this = self.clone();

	async move {
            let stat = this.inode.stat()?;
            let size = stat.size.unwrap();

	    let start = this.current_offset.load(Ordering::SeqCst);
	    
            if start >= size {
		return Ok(Bytes::new());
            }

            let to_read = core::cmp::min(len, size - start);
            if to_read == 0 {
		return Ok(Bytes::new());
            }

	    let end = start.saturating_add(to_read);

	    let start_block = start / self.block_size;
	    let end_block = end / self.block_size;

	    let mut out = BytesMut::new();

	    for block_index in start_block .. (end_block + 1) {
		let block = match this.block_list.get(block_index as usize) {
		    Some(b) => *b,
		    None => break,
		};

		let data = this.dev
		    .read(this.partition, block, 1)
		    .await
		    .map_err(|_| CanonicalError::Io)?;

		out.extend_from_slice(&data);
	    }

            let offset_in_first = (start % this.block_size) as usize;
            let wanted = to_read as usize;

            let slice = &out[offset_in_first..];
            let slice = &slice[..slice.len().min(wanted)];

	    this.current_offset.fetch_add(slice.len() as u64, Ordering::SeqCst);

            Ok(Bytes::copy_from_slice(slice))
        }
        .boxed()
    }

    fn write(self: Arc<Self>, _buf: bytes::Bytes) -> BoxFuture<'static, Result<u64, CanonicalError>> {	
	async move {
            Err(CanonicalError::Badf)
	}
	.boxed()
    }

    fn poll(self: Arc<Self>, events: syscall::PollEvents) -> BoxFuture<'static, Result<syscall::PollEvents, CanonicalError>> {	
	async move {
            Ok(events & (syscall::PollEvents::In | syscall::PollEvents::Out))
	}
	.boxed()
    }

    fn stat(self: Arc<Self>) -> Result<vfs::filesystem::Stat, CanonicalError> {
	self.inode.stat()
    }

    fn ioctl(self: Arc<Self>, _ioctl: ioctl::IoCtl, _arg: u64) -> BoxFuture<'static, Result<u64, CanonicalError>> {
	async move {
	    Err(CanonicalError::Inval)
	}.boxed()
    }

    fn seek(&self, offset: vfs::filesystem::SeekFrom) -> Result<u64, CanonicalError> {
	match offset {
            vfs::filesystem::SeekFrom::Set(n) => {
		self.current_offset.store(n.try_into().unwrap(), Ordering::SeqCst);
		Ok(n as u64)
	    },
            vfs::filesystem::SeekFrom::Cur(n) => {
		if n.is_negative() {
		    Ok(self.current_offset.fetch_sub((-n) as u64, Ordering::SeqCst) - (-n) as u64)
		} else {
		    Ok(self.current_offset.fetch_add(n as u64, Ordering::SeqCst) + n as u64)
		}
            }
            vfs::filesystem::SeekFrom::End(n) => {
		let size = self.inode.stat()?.size.unwrap();
		let new_offset = size as i64 + n;

		self.current_offset.store(new_offset as u64, Ordering::SeqCst);

		Ok(new_offset as u64)
            }
	}
    }
}

#[allow(dead_code)]
pub struct Fat16Fs {
    boot_record: RwLock<BootRecord>,
    extended_boot_record: RwLock<ExtendedBootRecord1216>,

    fat: RwLock<Vec<u16>>,

    dev: Arc<block::GptDevice>,
    partition: u32,
}

impl Fat16Fs {
    pub async fn new(dev: Arc<block::GptDevice>, partition: u32, boot_record: BootRecord, extended_boot_record: ExtendedBootRecord1216) -> Option<Fat16Fs> {
	// Check signature, double check this is actually FAT
	if extended_boot_record.signature != 0x28 && extended_boot_record.signature != 0x29 {
	    return None;
	}

	let vol = extended_boot_record.volume_label.iter()
	    .map(|c| c.to_char())
	    .collect::<String>();

	let root_directory_size_sectors = (boot_record.root_directory_entries * 32).div_ceil(boot_record.bytes_per_sector);
	let total_sectors = if boot_record.sectors_in_volume != 0 { boot_record.sectors_in_volume as u32 } else { boot_record.large_sector_count };
	let data_sectors = total_sectors - (boot_record.reserved_sectors as u32 + (boot_record.number_of_fats as u32 * boot_record.sectors_per_fat as u32) + root_directory_size_sectors as u32);
	
	let total_clusters = data_sectors / boot_record.sectors_per_cluster as u32;

	// If total clusters is off, it might9 be FAT but not FAT16
	if boot_record.sectors_per_fat != 0 &&
	    (4085..=65525).contains(&total_clusters) {
		log::info!("Found FAT16 volume {}", vol);
	    } else {
		log::info!("Not FAT16");
		return None;
	    }

	let mut fat = Fat16Fs {
	    boot_record: RwLock::new(boot_record),
	    extended_boot_record: RwLock::new(extended_boot_record),
	    fat: RwLock::new(Vec::new()),
	    dev,
	    partition,
	};

	fat.load_allocation_table().await;
	Some(fat)
    }

    async fn load_allocation_table(&mut self) {
	let inner = {
	    let boot_record = self.boot_record.read();
	    let partition = self.partition;

	    let sectors_per_lba = boot_record.bytes_per_sector / 512;

	    let fat_lba = sectors_per_lba * boot_record.reserved_sectors;

	    let fat_size_sectors = boot_record.sectors_per_fat;
	    let fat_size_lba = fat_size_sectors / sectors_per_lba;

	    self.dev.read(
		partition, fat_lba as u64, fat_size_lba as u64)
	};

	let fat_buf_ptr = inner.await.expect("Couldn't read FAT").as_ptr();
	let mut table: Vec<u16> = Vec::new();

	{
	    let boot_record = self.boot_record.read();
	    for entry in 0 .. (boot_record.sectors_per_fat as u32) * (boot_record.bytes_per_sector as u32) / 2 {
		unsafe {
		    table.push(
			ptr::read(fat_buf_ptr.wrapping_add(entry as usize * 2) as *const u16)
		    );
		}
	    }
	}

	let mut fat = self.fat.write();
	*fat = table;
    }

    fn get_filename(&self, dir: Bytes, index: usize) -> (Option<String>, usize) {
	let mut file_name = String::new();
	let mut cnt = 0;
	let boot_record = self.boot_record.read();

	for entry in index .. boot_record.root_directory_entries as usize {
	    let offset = entry * core::mem::size_of::<DirectoryEntry>();
	    let end = offset + core::mem::size_of::<DirectoryEntry>();
	    
	    let directory_entry = unsafe {
		ptr::read_unaligned(
		    dir[offset..end].as_ptr() as *const DirectoryEntry
		)
	    };

	    if directory_entry.file_name[0].to_u8() == 0x00 || directory_entry.file_name[0].to_u8() == 0xE5 {
		return (None, cnt);
	    }

	    if directory_entry.attributes == 0x0F {
		let long_filename_entry = unsafe {
		    ptr::read_unaligned(
			dir[offset .. end].as_ptr() as *const LongFileName
		    )
		};

		let name1 = long_filename_entry.name1;
		let name2 = long_filename_entry.name2;
		let name3 = long_filename_entry.name3;

		let name1_vec = name1.iter().copied()
		    .filter(|i| *i != 0x0000 && *i != 0xFFFF)
		    .collect::<Vec<u16>>();
		let name2_vec = name2.iter().copied()
		    .filter(|i| *i != 0x0000 && *i != 0xFFFF)
		    .collect::<Vec<u16>>();
		let name3_vec = name3.iter().copied()
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
			.as_str()
			.trim());

		let extn = directory_entry.file_name[8 .. 11].iter()
		    .map(|c| c.to_char())
		    .filter(|i| *i != '\0')
		    .collect::<String>()
		    .trim()
		    .to_string();

		if !extn.is_empty() {
		    file_name.push('.');
		    file_name.push_str(extn.as_str());
		}
	    }

	    return (Some(file_name), cnt)
	}

	(None, cnt)
    }

    fn read_dirent(buf: &Bytes, index: usize) -> Result<DirectoryEntry, CanonicalError> {
	let offset = index * core::mem::size_of::<DirectoryEntry>();
	let end = offset + core::mem::size_of::<DirectoryEntry>();

	if end > buf.len() {
            return Err(CanonicalError::Inval);
	}

	Ok(unsafe {
            ptr::read_unaligned(
		buf[offset..end].as_ptr() as *const DirectoryEntry
            )
	})
    }
}

impl vfs::filesystem::FileSystem for Fat16Fs {
    fn root(self: Arc<Self>, fsi: vfs::filesystem::FileSystemInstance) -> Arc<dyn vfs::filesystem::VNode> {
	let boot_record = self.boot_record.read();

	let sectors_per_lba = boot_record.bytes_per_sector / 512;

	let root_directory_sect = boot_record.reserved_sectors +
	    (boot_record.number_of_fats as u16 * boot_record.sectors_per_fat);
	let root_directory_lba = root_directory_sect * sectors_per_lba;

	let root_directory_size_sectors = (boot_record.root_directory_entries * 32).div_ceil(boot_record.bytes_per_sector);
	let root_directory_size_lba = root_directory_size_sectors / sectors_per_lba;

	Arc::new(RootINode {
	    root_directory_block: root_directory_lba.into(),
	    root_directory_size: root_directory_size_lba.into(),

	    fs: self.clone(),
	    fsi,
	})
    }

    fn lookup(self: Arc<Self>, fsi: vfs::filesystem::FileSystemInstance, parent: &Arc<dyn vfs::filesystem::VNode>, name: &str) -> BoxFuture<'static, Result<Arc<dyn vfs::filesystem::VNode>, CanonicalError>> {
	let this = self.clone();
	let parent = parent.clone();
	let name = name.to_owned();

	async move {
	    let directory_file_handle = parent.clone().open()?;
	    let size = directory_file_handle.clone().stat()?.size.unwrap();
	    let directory_contents = directory_file_handle.clone().read(size).await?;
	    
	    let mut entry = 0;
	    loop {
		let (maybe_fn, offset) = this.get_filename(directory_contents.clone(), entry as usize);
		entry += offset as u16;

		if let Some(file_name) = maybe_fn {
		    // This is a kludge; long file names in FAT are case sensitive, whereas normal ones aren't.
		    // TODO - make the semantics of this correct
		    if file_name != name && file_name != name.to_uppercase() {
			entry += 1;
			continue;
		    }

		    let directory_entry = Fat16Fs::read_dirent(&directory_contents, entry.into())?;

		    if directory_entry.attributes & 0x10 != 0 {
			let node: Arc<dyn vfs::filesystem::VNode> = Arc::new(INode {
			    file_name: name,
			    file_size: directory_entry.file_size,
			    start_cluster: directory_entry.cluster_low as u32,
			    kind: vfs::filesystem::VNodeKind::Directory,
			    fs: this.clone(),
			    fsi,
			    parent: Some(parent.clone()),
			});
			return Ok(node);
		    } else {
			let node: Arc<dyn vfs::filesystem::VNode> = Arc::new(INode {
			    file_name: name,
			    file_size: directory_entry.file_size,
			    start_cluster: directory_entry.cluster_low as u32,
			    kind: vfs::filesystem::VNodeKind::Regular,
			    fs: this.clone(),
			    fsi,
			    parent: Some(parent.clone()),
			});
			return Ok(node);
		    }
		} else {
		    let directory_entry = Fat16Fs::read_dirent(&directory_contents, entry.into())?;

		    if directory_entry.file_name[0].to_u8() == 0x00 {
			break;
		    } else if directory_entry.file_name[0].to_u8() == 0xE5 || directory_entry.attributes & 0x08 != 0 {
			entry += 1;
			continue;
		    }
		}
	    }

	    Err(CanonicalError::NoEnt)
	}.boxed()
    }
}
