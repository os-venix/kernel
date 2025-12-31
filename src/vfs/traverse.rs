use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::vfs::mount;
use crate::vfs::filesystem::{VNode, VNodeKind, FileHandle, FileSystemInstance};
use crate::sys::syscall::CanonicalError;

// const MAX_SYMLINK_DEPTH: u8 = 8;

async fn traverse(current: Arc<dyn VNode>, name: &str) -> Result<Arc<dyn VNode>, CanonicalError> {
    if name == "." {
        return Ok(current);
    }

    if name == ".." {
	// Defers through mount table to allow for FS root to parent FS
        return mount::MOUNT_TABLE.get().expect("Attempted to use mount table before init").parent(&current);
    }

    // Must be directory. If it isn't, we can't descend
    if current.kind() != VNodeKind::Directory {
        return Err(CanonicalError::NotDir);
    }

    // Lookup child in current directory
    let fs = current.filesystem();
    let mut child = fs.lookup(current.fsi(), &current, name).await?;

    // Follow symlinks (for intermediate path components)
//    let mut depth = 0;
    while child.kind() == VNodeKind::Symlink {
	unimplemented!();
        // if depth >= MAX_SYMLINK_DEPTH {
        //     return Err(CanonicalError::ELOOP);
        // }

        // let target = child.readlink()?;

        // // Symlink targets may be absolute or relative
        // child = if target.starts_with('/') {
        //     // Absolute: restart from VFS root
        //     vfs_walk_path(&target).await?
        // } else {
        //     // Relative: resolve against current directory
        //     vfs_walk_path_relative(&target, current.clone()).await?
        // };

        // depth += 1;
    }

    // If the child is a mountpoint, pick the root up for that FS
    if let Some(mounted_root) = mount::MOUNT_TABLE.get().expect("Used mount before init").lookup_mount(&child) {
        child = mounted_root;
    }

    Ok(child)
}

// async fn vfs_walk_path_relative(path: &str, current: Arc<dyn VNode>) -> Result<Arc<dyn VNode>, CanonicalError> {
//     let mut current = current.clone();
//     let components: Vec<&str> = path
// 	.split('/')
// 	.filter(|c| !c.is_empty())
// 	.collect();

//     // Cannot open nothing
//     if components.is_empty() {
// 	return Err(CanonicalError::Inval);
//     }

//     for component in components {
// 	current = traverse(current, component).await?;
//     }

//     Ok(current)
// }
    
pub async fn vfs_walk_path(path: &str) -> Result<Arc<dyn VNode>, CanonicalError> {
    let mut current = if path.starts_with('/') {
	mount::MOUNT_TABLE.get().expect("Attempted to use mount table before init")
	    .root()?.root(FileSystemInstance(0))
    } else {
	unimplemented!();
//	current_process.cwd()
    };

    let components: Vec<&str> = path
	.split('/')
	.filter(|c| !c.is_empty())
	.collect();

    // Cannot open nothing
    if components.is_empty() {
	return Err(CanonicalError::Inval);
    }

    for component in components {
	current = traverse(current, component).await?;
    }

    Ok(current)
}

pub async fn vfs_open(path: &str/*, flags: OpenFlags, mode: FileMode*/) -> Result<Arc<dyn FileHandle>, CanonicalError> {
    // We don't do file creation yet. It'd go here otherwise
    // We don't handle symlinking either, that also belongs here
    // We don't handle permissions (single user OS), that would go here
    let vnode = vfs_walk_path(path).await?;
    vnode.open()
}
