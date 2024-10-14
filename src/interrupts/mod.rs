use alloc::vec::Vec;
use aml::value::{Args, AmlValue};
use aml::{AmlName, AmlError};

use crate::sys::acpi::AML;

mod local_apic;
mod io_apic;
mod idt;

const IRQ_BASE: u8 = 32;

pub fn init_idt() {
    idt::init();
}

pub fn init_bsp_apic() {
    let bsp_apic_id = local_apic::init_bsp_local_apic();
    io_apic::init_io_apics(bsp_apic_id);

    {
	let mut aml = AML.get().expect("Unable to access AML Context.").write();

	match aml.invoke_method(
	    &AmlName::from_str("\\_PIC").expect("Unable to find \\_PIC method"),
	    Args::from_list(Vec::from([AmlValue::Integer(1)])).expect("Unable to construct AML args list")) {
	    Ok(_) => (),
	    Err(AmlError::ValueDoesNotExist(_)) => (),  // Method is not implemented by firmware, skip
	    Err(e) => panic!("Attempting to switch system into APIC mode failed: {:?}", e),
	}
    }
    
//    x86_64::instructions::interrupts::enable();
}
