use alloc::boxed::Box;
use alloc::fmt;
use alloc::string::{String, ToString};
use bitfield::bitfield;
use core::any::Any;
use core::ffi::{c_char, c_void, CStr};

use crate::sys::acpi::{Namespace, UacpiStatus, UacpiIterationDecision, UacpiObjectType, UacpiIdString};
use crate::driver;

type ForeachNamespaceCallbackPtr = unsafe extern "C" fn (user: *mut c_void, namespace: Namespace, depth: u32) -> UacpiIterationDecision;

bitfield! {
    pub struct NamespaceNodeInfoFlags(u8);

    sxw, _: 6;
    sxd, _: 5;
    cls, _: 4;
    cid, _: 3;
    uid, _: 2;
    hid, _: 1;
    adr, _: 0;
}

#[repr(C)]
struct UacpiPnpIdList {
    num_ids: u32,
    size: u32,
    ids: [UacpiIdString],
}

#[repr(C)]
struct UacpiNamespaceNodeInfo {
    size: u32,

    name: [c_char; 4],
    object_type: UacpiObjectType,
    num_params: u8,

    flags: NamespaceNodeInfoFlags,
    sxd: [u8; 4],
    sxw: [u8; 5],

    adr: u64,
    hid: UacpiIdString,
    uid: UacpiIdString,
    cls: UacpiIdString,
//    cid: UacpiPnpIdList,
}

bitfield! {
    #[derive(Clone, Copy, Default)]
    pub struct UacpiObjectTypeBits(u32);

    device_bit, set_device_bit: 6;
}

#[derive(PartialEq, Eq, Clone)]
pub struct SystemBusDeviceIdentifier {
    pub namespace: Namespace,
    pub hid: Option<String>,
    pub uid: Option<String>,
    pub path: String,
}

impl driver::DeviceTypeIdentifier for SystemBusDeviceIdentifier {
    fn as_any(&self) -> &dyn Any {
	self
    }
}

impl fmt::Display for SystemBusDeviceIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
	write!(f, "{}/", self.path)?;

	if let Some(hid) = &self.hid {
	    write!(f, "{}:", hid)?;
	}

	if let Some(uid) = &self.uid {
	    write!(f, "{}", uid)?;
	}

	Ok(())
    }
}

extern "C" {
    fn uacpi_namespace_node_generate_absolute_path(namespace: Namespace) -> *const c_char;
    fn uacpi_free_absolute_path(path: *const c_char);
    fn uacpi_get_namespace_node_info(namespace: Namespace, info: *mut *mut UacpiNamespaceNodeInfo) -> UacpiStatus;
    fn uacpi_free_namespace_node_info(info: *mut UacpiNamespaceNodeInfo);
    fn uacpi_namespace_for_each_child(
	namespace: Namespace, ascending_callback: Option<ForeachNamespaceCallbackPtr>,
	descending_callback: Option<ForeachNamespaceCallbackPtr>, type_mask: UacpiObjectTypeBits,
	max_depth: u32, user: *mut c_void) -> UacpiStatus;
    fn uacpi_namespace_root() -> Namespace;
}

unsafe extern "C" fn acpi_probe_device(user: *mut c_void, namespace: Namespace, depth: u32) -> UacpiIterationDecision {
    let path = {
	let path_cchar: *const c_char = uacpi_namespace_node_generate_absolute_path(namespace);
	
	let path_cstr = unsafe { CStr::from_ptr(path_cchar) };
	let path = String::from_utf8_lossy(path_cstr.to_bytes()).to_string();

	uacpi_free_absolute_path(path_cchar);
	path
    };
    
    let mut namespace_info_ptr: *mut UacpiNamespaceNodeInfo = core::ptr::null_mut();
    let ret = uacpi_get_namespace_node_info(namespace, &mut namespace_info_ptr);

    if ret != UacpiStatus::Ok {
	log::error!("Unable to retrieve node {} information: {:?}", path, ret);
	return UacpiIterationDecision::Continue;
    }

    let namespace_info = namespace_info_ptr.as_ref().unwrap();
    let hid = if namespace_info.flags.hid() { Some(namespace_info.hid.to_string()) } else { None };
    let uid = if namespace_info.flags.uid() { Some(namespace_info.uid.to_string()) } else { None };

    let ident = SystemBusDeviceIdentifier {
	namespace: namespace,
	hid: hid,
	uid: uid,
	path: path,
    };

    driver::enumerate_device(Box::new(ident));
    UacpiIterationDecision::Continue
}

pub fn enumerate() -> Result<(), UacpiStatus> {
    let mut type_bits = UacpiObjectTypeBits::default();
    type_bits.set_device_bit(true);

    let ret = unsafe {
	let root_namespace = uacpi_namespace_root();
	uacpi_namespace_for_each_child(
	    root_namespace, Some(acpi_probe_device), None, type_bits, 0xFFFF_FFFF, core::ptr::null_mut())
    };

    match ret {
	UacpiStatus::Ok => Ok(()),
	_ => Err(ret),
    }
}
