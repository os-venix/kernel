use alloc::vec::Vec;
use core::ffi::c_void;

use crate::sys::acpi::{Namespace, UacpiIterationDecision, UacpiStatus};

#[repr(u32)]
enum UacpiResourceType {
    Irq,
    ExtendedIrq,

    Dma,
    FixedDma,

    Io,
    FixedIo,

    Address16,
    Address32,
    Address64,
    Address64Extended,

    Memory24,
    Memory32,
    FixedMemory32,

    StartDependent,
    EndDependent,

    
    VendorSmall,
    VendorLarge,

    GenericRegister,
    GpioConnection,

    SerialI2CConnection,
    SerialSpiConnection,
    SerialUartConnection,
    SerialCsi2Connection,

    PinFunction,
    PinConfiguration,
    PinGroup,
    PinGroupFunction,
    PinGroupConfiguration,

    ClockInput,

    UacpiResourceTypeEndTag,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct UacpiFixedMemory32 {
    pub write_status: u8,
    pub address: u32,
    pub length: u32,
}

#[repr(C)]
union UacpiResourceUnion {
    fixed_memory_32: UacpiFixedMemory32,
}

#[repr(C)]
struct UacpiResource {
    resource_type: UacpiResourceType,
    length: u32,

    resource: UacpiResourceUnion,
}

#[repr(C)]
struct UacpiResources {
    length: usize,
    entries: *mut UacpiResource,
}

pub enum Resource {
    FixedMemory32 {
	write_status: u8,
	address: u32,
	length: u32,
    }
}

type ForeachResourceCallbackPtr = unsafe extern "C" fn (user: *mut c_void, resource: *mut UacpiResource) -> UacpiIterationDecision;

extern "C" {
    fn uacpi_get_current_resources(namespace: Namespace, resources: *mut *mut UacpiResources) -> UacpiStatus;
    fn uacpi_for_each_resource(resources: *mut UacpiResources, callback: ForeachResourceCallbackPtr, user: *mut c_void) -> UacpiStatus;
}

pub fn get_resources(namespace: Namespace) -> Result<Vec<Resource>, UacpiStatus> {
    unsafe extern "C" fn gather_resources_into_rust(user: *mut c_void, resource_ptr: *mut UacpiResource) -> UacpiIterationDecision {
	let resources_vec: &mut Vec<Resource> = unsafe { &mut *(user as *mut Vec<Resource>) };
	let resource = resource_ptr.as_ref().unwrap();

	match resource.resource_type {
	    UacpiResourceType::FixedMemory32 => {
		resources_vec.push(Resource::FixedMemory32 {
		    write_status: resource.resource.fixed_memory_32.write_status,
		    address: resource.resource.fixed_memory_32.address,
		    length: resource.resource.fixed_memory_32.length,
		});
	    },
	    _ => (),
	}

	UacpiIterationDecision::Continue
    }

    unsafe {
	let mut resource_ptr: *mut UacpiResources = core::ptr::null_mut();
	let ret = uacpi_get_current_resources(namespace, &mut resource_ptr);

	if ret != UacpiStatus::Ok {
	    return Err(ret);
	}

	let mut resources_vec: Vec<Resource> = Vec::new();
	let ret = uacpi_for_each_resource(resource_ptr, gather_resources_into_rust, &mut resources_vec as *mut _ as *mut c_void);

	match ret {
	    UacpiStatus::Ok => Ok(resources_vec),
	    e => Err(e),
	}
    }
}
