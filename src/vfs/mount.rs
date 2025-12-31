use alloc::sync::{Arc, Weak};
use alloc::collections::BTreeMap;
use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering;
use spin::{Once, RwLock};

use crate::vfs::filesystem::{VNode, VNodeKind, FileSystem, FileSystemInstance};
use crate::sys::syscall::CanonicalError;

use crate::vfs::traverse::vfs_walk_path;

struct Mount {
    mountpoint: Weak<dyn VNode>,
    fs: Arc<dyn FileSystem>,
    fsi: FileSystemInstance,
}

#[derive(Ord, Eq, PartialOrd, PartialEq)]
struct MountId {
    fs_instance_id: FileSystemInstance,
    inode_id: u64,
}

pub struct MountTable {
    mounts_by_mountpoint: RwLock<BTreeMap<MountId, Arc<Mount>>>,
    mounts_by_root: RwLock<BTreeMap<MountId, Arc<Mount>>>,

    root_fs: RwLock<Option<Arc<dyn FileSystem>>>,

    next_mount_id: AtomicU64,
}

impl MountTable {
    fn new() -> Self {
	Self {
	    mounts_by_mountpoint: RwLock::new(BTreeMap::new()),
	    mounts_by_root: RwLock::new(BTreeMap::new()),
	    next_mount_id: AtomicU64::new(1), // 0 is root
	    root_fs: RwLock::new(None),
	}
    }

    pub fn mount(&self, mountpoint: Arc<dyn VNode>, fs: Arc<dyn FileSystem>) -> Result<(), CanonicalError> {
	if mountpoint.kind() != VNodeKind::Directory {
	    return Err(CanonicalError::NotDir);
	}

	let fsi = FileSystemInstance(self.next_mount_id.fetch_add(1, Ordering::Relaxed));

	let mount = Arc::new(Mount {
	    mountpoint: Arc::downgrade(&mountpoint),
	    fs: fs.clone(),
	    fsi,
	});

	let mounted_id = MountId {
	    fs_instance_id: fsi,
	    inode_id: fs.root(mountpoint.fsi()).inode(),
	};
	let mount_id = MountId {
	    fs_instance_id: mountpoint.fsi(),
	    inode_id: mountpoint.inode(),
	};

	self.mounts_by_mountpoint
	    .write()
	    .insert(mount_id, mount.clone());
	self.mounts_by_root
	    .write()
	    .insert(mounted_id, mount.clone());

	Ok(())
    }

    pub fn lookup_mount(&self, vnode: &Arc<dyn VNode>) -> Option<Arc<dyn VNode>> {
	let inode = vnode.inode();
	let fsi_id = vnode.fsi();

	let mount_id = MountId {
	    fs_instance_id: fsi_id,
	    inode_id: inode
	};

	let mounts = self.mounts_by_mountpoint.read();
	mounts.get(&mount_id).map(|m| m.fs.clone().root(m.fsi))
    }

    pub fn parent(&self, vnode: &Arc<dyn VNode>) -> Result<Arc<dyn VNode>, CanonicalError> {
	let inode_id = vnode.inode();
	let fs_instance_id = vnode.fsi();

	let mount_id = MountId {
	    fs_instance_id,
	    inode_id,
	};

	if let Some(mount) = self.mounts_by_root.read().get(&mount_id) {
	    if let Some(mountpoint) = mount.mountpoint.upgrade() {
		return mountpoint.parent();
	    } else {
		return Err(CanonicalError::NoEnt);
	    }
	}

        // Global root
        if Arc::ptr_eq(vnode, &self.root()?.root(FileSystemInstance(0))) {
            return Ok(vnode.clone());
        }

	vnode.parent()
    }

    pub fn root(&self) -> Result<Arc<dyn FileSystem>, CanonicalError> {
	let root = self.root_fs.read();

	match &*root {
	    Some(r) => Ok(r.clone()),
	    None => Err(CanonicalError::NoEnt),
	}
    }

    pub fn mount_root(&self, fs: Arc<dyn FileSystem>) -> Result<(), CanonicalError> {
	let mut root = self.root_fs.write();

	// Check we're not about to do this twice
	if root.is_some() {
	    return Err(CanonicalError::Inval);
	}

	*root = Some(fs);
	Ok(())
    }
}

pub static MOUNT_TABLE: Once<MountTable> = Once::new();

pub fn init() {
    MOUNT_TABLE.call_once(MountTable::new);
}

pub async fn mount(path: &str, fs: Arc<dyn FileSystem>) -> Result<(), CanonicalError> {
    let mount_node = vfs_walk_path(path).await?;
    MOUNT_TABLE.get().expect("Accessed mount table before init").mount(mount_node, fs)
}

pub fn mount_root(fs: Arc<dyn FileSystem>) -> Result<(), CanonicalError> {
    MOUNT_TABLE.get().expect("Accessed mount table before init").mount_root(fs)
}
