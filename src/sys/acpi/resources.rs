use alloc::vec::Vec;
use core::ffi::c_void;

use crate::sys::acpi::{uacpi, uacpi_status};

#[derive(Debug)]
#[allow(dead_code)]
pub enum Resource {
    Irq {
	triggering: u8,
	polarity: u8,
	sharing: bool,
	wake_capability: bool,
	irqs: Vec<u32>,
    },
    FixedMemory32 {
	write_status: u8,
	address: u32,
	length: u32,
    }
}

pub fn get_resources(namespace: *mut uacpi::uacpi_namespace_node) -> Result<Vec<Resource>, uacpi_status> {
    unsafe extern "C" fn gather_resources_into_rust(user: *mut c_void, resource_ptr: *mut uacpi::uacpi_resource) -> uacpi::uacpi_iteration_decision {
	let resources_vec: &mut Vec<Resource> = unsafe { &mut *(user as *mut Vec<Resource>) };
	let resource = resource_ptr.as_ref().unwrap();

	match uacpi::uacpi_resource_type::try_from(resource.type_).unwrap() {
	    uacpi::uacpi_resource_type::UACPI_RESOURCE_TYPE_IRQ => {
		let irq_vec = unsafe { resource.__bindgen_anon_1.irq.as_ref().irqs.as_slice(resource.__bindgen_anon_1.irq.as_ref().num_irqs as usize).to_vec() };

		resources_vec.push(Resource::Irq {
		    triggering: resource.__bindgen_anon_1.irq.as_ref().triggering,
		    polarity: resource.__bindgen_anon_1.irq.as_ref().polarity,
		    sharing: resource.__bindgen_anon_1.irq.as_ref().sharing == 1,
		    wake_capability: resource.__bindgen_anon_1.irq.as_ref().wake_capability == 1,
		    irqs: irq_vec
			.into_iter()
			.map(|irq| irq as u32)
			.collect(),
		});
	    },
	    uacpi::uacpi_resource_type::UACPI_RESOURCE_TYPE_EXTENDED_IRQ => {
		let irq_vec = unsafe { resource.__bindgen_anon_1.extended_irq.as_ref().irqs
				       .as_slice(resource.__bindgen_anon_1.extended_irq.as_ref().num_irqs as usize).to_vec() };

		resources_vec.push(Resource::Irq {
		    triggering: resource.__bindgen_anon_1.extended_irq.as_ref().triggering,
		    polarity: resource.__bindgen_anon_1.extended_irq.as_ref().polarity,
		    sharing: resource.__bindgen_anon_1.extended_irq.as_ref().sharing == 1,
		    wake_capability: resource.__bindgen_anon_1.extended_irq.as_ref().wake_capability == 1,
		    irqs: irq_vec,
		});
	    },
	    uacpi::uacpi_resource_type::UACPI_RESOURCE_TYPE_FIXED_MEMORY32 => {
		resources_vec.push(Resource::FixedMemory32 {
		    write_status: resource.__bindgen_anon_1.fixed_memory32.as_ref().write_status,
		    address: resource.__bindgen_anon_1.fixed_memory32.as_ref().address,
		    length: resource.__bindgen_anon_1.fixed_memory32.as_ref().length,
		});
	    },
	    _ => (),
	}

	uacpi::uacpi_iteration_decision::UACPI_ITERATION_DECISION_CONTINUE
    }

    unsafe {
	let mut resource_ptr: *mut uacpi::uacpi_resources = core::ptr::null_mut();
	let ret = uacpi::uacpi_get_current_resources(namespace, &mut resource_ptr);

	if ret != uacpi_status::UACPI_STATUS_OK {
	    return Err(ret);
	}

	let mut resources_vec: Vec<Resource> = Vec::new();
	let ret = uacpi::uacpi_for_each_resource(resource_ptr, Some(gather_resources_into_rust), &mut resources_vec as *mut _ as *mut c_void);

	uacpi::uacpi_free_resources(resource_ptr);
	match ret {
	    uacpi_status::UACPI_STATUS_OK => Ok(resources_vec),
	    e => Err(e),
	}
    }
}
