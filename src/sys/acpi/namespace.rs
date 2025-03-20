use alloc::boxed::Box;
use alloc::fmt;
use alloc::string::{String, ToString};
use bitfield::bitfield;
use core::any::Any;
use core::ffi::{c_char, c_void, CStr};

use crate::interrupts;
use crate::utils::vector_map::VecMap;
use crate::sys::acpi::{Namespace, UacpiStatus, UacpiIterationDecision, UacpiObjectType, UacpiIdString};
use crate::sys::acpi::resources;
use crate::driver;
use crate::drivers::pcie::InterruptPin;
use crate::utils::__IncompleteArrayField;

type ForeachNamespaceCallbackPtr = unsafe extern "C" fn (user: *mut c_void, namespace: Namespace, depth: u32) -> UacpiIterationDecision;

#[repr(C)]
struct UacpiPciRoutingTableEntry {
    address: u32,
    index: u32,
    source: Namespace,
    pin: u8,
}

#[repr(C)]
struct UacpiPciRoutingTable {
    num_entries: usize,
    entries: __IncompleteArrayField<UacpiPciRoutingTableEntry>,
}

bitfield! {
    #[repr(C)]
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
    #[repr(C)]
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

#[derive(Debug, Eq, PartialOrd)]
pub struct PciInterruptFunction {
    pub device: u8,
    pub function: Option<u8>,
    pub pin: InterruptPin,
}

impl PartialEq for PciInterruptFunction {
    fn eq(&self, other: &Self) -> bool {
	let functions_equal = self.function == other.function || self.function == None || other.function == None;
	self.device == other.device && self.pin == other.pin && functions_equal
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

    fn uacpi_get_pci_routing_table(node: Namespace, ret: *mut *mut UacpiPciRoutingTable) -> UacpiStatus;
    fn uacpi_free_pci_routing_table(route: *mut UacpiPciRoutingTable);
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
	hid: hid.clone(),
	uid: uid.clone(),
	path: path,
    };

    if hid.is_some() || uid.is_some() {
	// If neither of these, probably not a device with a driver. Could be a PCI link, for instance
	driver::enumerate_device(Box::new(ident));
    }

    uacpi_free_namespace_node_info(namespace_info_ptr);
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

pub fn get_pci_routing_table(namespace: Namespace) -> Result<VecMap<PciInterruptFunction, interrupts::InterruptRoute>, UacpiStatus> {
    let mut pci_routing_ptr: *mut UacpiPciRoutingTable = core::ptr::null_mut();
    let ret = unsafe {
	uacpi_get_pci_routing_table(namespace, &mut pci_routing_ptr)
    };

    if ret != UacpiStatus::Ok {
	return Err(ret);
    }

    let pci_routing = unsafe { pci_routing_ptr.as_ref().unwrap() };
    let routes = unsafe { pci_routing.entries.as_slice(pci_routing.num_entries) };
    let mut routing_map: VecMap<PciInterruptFunction, interrupts::InterruptRoute> = VecMap::new();
    for route in routes {
	let all_functions = (route.address & 0xFFFF) == 0xFFFF;
	let function_address = if all_functions { None } else { Some(route.address as u8) };
	let device_address = (route.address >> 16) as u8;
	let pin = match route.pin {
	    0 => InterruptPin::IntA,
	    1 => InterruptPin::IntB,
	    2 => InterruptPin::IntC,
	    3 => InterruptPin::IntD,
	    _ => panic!("Malformed interrupt routing"),
	};

	if route.index != 0 {
	    // GSI
	    routing_map.insert(PciInterruptFunction {
		device: device_address,
		function: function_address,
		pin: pin,
	    }, interrupts::InterruptRoute::Gsi(route.index));
	} else {
	    // IRQ
	    let link_resources = resources::get_resources(route.source)?;
	    
	    let irqs = link_resources.iter()
		.filter(|r| match r {
		    resources::Resource::Irq { .. } => true,
		    _ => false,
		}).map(|r| match r {
		    resources::Resource::Irq {
			irqs,
			..
		    } => irqs,
		    _ => panic!("This shouldn't happen"),
		}).nth(0).expect("Expected a list of IRQs");
	    let irq = irqs[0];

	    routing_map.insert(PciInterruptFunction {
		device: device_address,
		function: function_address,
		pin: pin,
	    }, interrupts::InterruptRoute::Irq(irq));
	}
    }

    unsafe { uacpi_free_pci_routing_table(pci_routing_ptr); }
    Ok(routing_map)
}
