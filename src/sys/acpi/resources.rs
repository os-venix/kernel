use alloc::vec::Vec;
use core::ffi::{c_char, c_void};

use crate::sys::acpi::{Namespace, UacpiIterationDecision, UacpiStatus};
use crate::utils::__IncompleteArrayField;

#[repr(u32)]
#[allow(dead_code)]
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

    ResourceTypeEndTag,
}

#[repr(C)]
#[derive(Copy, Clone)]
#[allow(dead_code)]
struct UacpiResourceSource {
    index: u8,
    index_present: bool,
    length: u16,
    string: *const c_char,
}

#[repr(C)]
#[derive(Copy, Clone)]
#[allow(dead_code)]
struct UacpiResourceIrq {
    pub length_kind: u8,
    pub triggering: u8,
    pub polarity: u8,
    pub sharing: u8,
    pub wake_capability: u8,
    pub num_irqs: u8,
    pub irqs: __IncompleteArrayField<u8>,
}

#[repr(C)]
#[derive(Copy, Clone)]
#[allow(dead_code)]
struct UacpiResourceExtendedIrq {
    pub direction: u8,
    pub triggering: u8,
    pub polarity: u8,
    pub sharing: u8,
    pub wake_capability: u8,
    pub num_irqs: u8,
    pub source: UacpiResourceSource,
    pub irqs: __IncompleteArrayField<u8>,
}

#[repr(C)]
#[derive(Copy, Clone)]
#[allow(dead_code)]
struct UacpiFixedMemory32 {
    pub write_status: u8,
    pub address: u32,
    pub length: u32,
}

#[repr(C)]
#[allow(dead_code)]
union UacpiResourceUnion {
    irq: UacpiResourceIrq,
    extended_irq: UacpiResourceExtendedIrq,
    fixed_memory_32: UacpiFixedMemory32,
}

#[repr(C)]
#[allow(dead_code)]
struct UacpiResource {
    resource_type: UacpiResourceType,
    length: u32,

    resource: UacpiResourceUnion,
}

#[repr(C)]
#[allow(dead_code)]
struct UacpiResources {
    length: usize,
    entries: *mut UacpiResource,
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum Resource {
    Irq {
	triggering: u8,
	polarity: u8,
	sharing: bool,
	wake_capability: bool,
	irqs: Vec<u8>,
    },
    FixedMemory32 {
	write_status: u8,
	address: u32,
	length: u32,
    }
}

type ForeachResourceCallbackPtr = unsafe extern "C" fn (user: *mut c_void, resource: *mut UacpiResource) -> UacpiIterationDecision;

extern "C" {
    fn uacpi_get_current_resources(namespace: Namespace, resources: *mut *mut UacpiResources) -> UacpiStatus;
    fn uacpi_free_resources(resources: *mut UacpiResources);
    fn uacpi_for_each_resource(resources: *mut UacpiResources, callback: ForeachResourceCallbackPtr, user: *mut c_void) -> UacpiStatus;
}

pub fn get_resources(namespace: Namespace) -> Result<Vec<Resource>, UacpiStatus> {
    unsafe extern "C" fn gather_resources_into_rust(user: *mut c_void, resource_ptr: *mut UacpiResource) -> UacpiIterationDecision {
	let resources_vec: &mut Vec<Resource> = unsafe { &mut *(user as *mut Vec<Resource>) };
	let resource = resource_ptr.as_ref().unwrap();

	match resource.resource_type {
	    UacpiResourceType::Irq => {
		let irq_vec = unsafe { resource.resource.irq.irqs.as_slice(resource.resource.irq.num_irqs as usize).to_vec() };

		resources_vec.push(Resource::Irq {
		    triggering: resource.resource.irq.triggering,
		    polarity: resource.resource.irq.polarity,
		    sharing: resource.resource.irq.sharing == 1,
		    wake_capability: resource.resource.irq.wake_capability == 1,
		    irqs: irq_vec,
		});
	    },
	    UacpiResourceType::ExtendedIrq => {
		let irq_vec = unsafe { resource.resource.extended_irq.irqs.as_slice(resource.resource.extended_irq.num_irqs as usize).to_vec() };

		resources_vec.push(Resource::Irq {
		    triggering: resource.resource.extended_irq.triggering,
		    polarity: resource.resource.extended_irq.polarity,
		    sharing: resource.resource.extended_irq.sharing == 1,
		    wake_capability: resource.resource.extended_irq.wake_capability == 1,
		    irqs: irq_vec,
		});
	    },
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

	uacpi_free_resources(resource_ptr);
	match ret {
	    UacpiStatus::Ok => Ok(resources_vec),
	    e => Err(e),
	}
    }
}
