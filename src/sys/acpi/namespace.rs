use alloc::boxed::Box;
use alloc::fmt;
use alloc::string::{String, ToString};
use bitfield::bitfield;
use core::any::Any;
use core::ffi::{c_char, c_void, CStr};

use crate::interrupts;
use crate::utils::vector_map::VecMap;
use crate::sys::acpi::{uacpi_status, uacpi};
use crate::sys::acpi::resources;
use crate::driver;
use crate::drivers::pcie::InterruptPin;

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

#[derive(PartialEq, Eq, Clone)]
#[allow(dead_code)]
pub struct SystemBusDeviceIdentifier {
    pub namespace: *mut uacpi::uacpi_namespace_node,
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
#[allow(dead_code)]
pub struct PciInterruptFunction {
    pub device: u8,
    pub function: Option<u8>,
    pub pin: InterruptPin,
}

impl PartialEq for PciInterruptFunction {
    fn eq(&self, other: &Self) -> bool {
	let functions_equal = self.function == other.function || self.function.is_none() || other.function.is_none();
	self.device == other.device && self.pin == other.pin && functions_equal
    }
}

unsafe extern "C" fn acpi_probe_device(_user: *mut c_void, namespace: *mut uacpi::uacpi_namespace_node, _depth: u32) -> uacpi::uacpi_iteration_decision {
    let path = {
	let path_cchar: *const c_char = uacpi::uacpi_namespace_node_generate_absolute_path(namespace);
	
	let path_cstr = unsafe { CStr::from_ptr(path_cchar) };
	let path = String::from_utf8_lossy(path_cstr.to_bytes()).to_string();

	uacpi::uacpi_free_absolute_path(path_cchar);
	path
    };
    
    let mut namespace_info_ptr: *mut uacpi::uacpi_namespace_node_info = core::ptr::null_mut();
    let ret = uacpi::uacpi_get_namespace_node_info(namespace, &mut namespace_info_ptr);

    if ret != uacpi_status::UACPI_STATUS_OK {
	log::error!("Unable to retrieve node {} information: {:?}", path, ret);
	return uacpi::uacpi_iteration_decision::UACPI_ITERATION_DECISION_CONTINUE;
    }

    let namespace_info = namespace_info_ptr.as_ref().unwrap();
    let hid = if (namespace_info.flags & uacpi::UACPI_NS_NODE_INFO_HAS_HID as u8) != 0 { Some(namespace_info.hid.to_string()) } else { None };
    let uid = if (namespace_info.flags & uacpi::UACPI_NS_NODE_INFO_HAS_UID as u8) != 0 { Some(namespace_info.uid.to_string()) } else { None };

    let ident = SystemBusDeviceIdentifier {
	namespace,
	hid: hid.clone(),
	uid: uid.clone(),
	path,
    };

    if hid.is_some() || uid.is_some() {
	// If neither of these, probably not a device with a driver. Could be a PCI link, for instance
	driver::enumerate_device(Box::new(ident));
    }

    uacpi::uacpi_free_namespace_node_info(namespace_info_ptr);
    uacpi::uacpi_iteration_decision::UACPI_ITERATION_DECISION_CONTINUE
}

pub fn enumerate() -> Result<(), uacpi_status> {
    let type_bits = uacpi::uacpi_object_type_bits::UACPI_OBJECT_DEVICE_BIT;

    let ret = unsafe {
	let root_namespace = uacpi::uacpi_namespace_root();
	uacpi::uacpi_namespace_for_each_child(
	    root_namespace, Some(acpi_probe_device), None, type_bits, 0xFFFF_FFFF, core::ptr::null_mut())
    };

    match ret {
	uacpi_status::UACPI_STATUS_OK => Ok(()),
	_ => Err(ret),
    }
}

pub fn get_pci_routing_table(namespace: *mut uacpi::uacpi_namespace_node) -> Result<VecMap<PciInterruptFunction, interrupts::InterruptRoute>, uacpi_status> {
    let mut pci_routing_ptr: *mut uacpi::uacpi_pci_routing_table = core::ptr::null_mut();
    let ret = unsafe {
	uacpi::uacpi_get_pci_routing_table(namespace, &mut pci_routing_ptr)
    };

    if ret != uacpi_status::UACPI_STATUS_OK {
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
		pin,
	    }, interrupts::InterruptRoute::Gsi(route.index));
	} else {
	    // IRQ
	    let link_resources = resources::get_resources(route.source)?;
	    
	    let irqs = link_resources.iter()
		.filter(|r| matches!(r, resources::Resource::Irq { .. }))
		.map(|r| match r {
		    resources::Resource::Irq {
			irqs,
			..
		    } => irqs,
		    _ => panic!("This shouldn't happen"),
		}).next().expect("Expected a list of IRQs");
	    let irq = irqs[0];

	    routing_map.insert(PciInterruptFunction {
		device: device_address,
		function: function_address,
		pin,
	    }, interrupts::InterruptRoute::Irq(irq.try_into().unwrap()));
	}
    }

    unsafe { uacpi::uacpi_free_pci_routing_table(pci_routing_ptr); }
    Ok(routing_map)
}
