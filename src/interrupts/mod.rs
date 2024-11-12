use alloc::vec::Vec;
use alloc::boxed::Box;
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

pub fn init_handler_funcs() {
    idt::init_handlers();
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
	    Err(AmlError::ValueDoesNotExist(_)) => (),  // Optional method is not implemented by firmware, skip
	    Err(e) => panic!("Attempting to switch system into APIC mode failed: {:?}", e),
	}
    }
    
//    x86_64::instructions::interrupts::enable();
}

pub fn enable_gsi(gsi: u32, handler: &'static (dyn Fn() + Send + Sync)) {
    let irq = io_apic::get_irq_for_gsi(gsi);
    log::info!("GSI = {}, IRQ = {}", gsi, irq);
    idt::add_handler_to_irq(irq, Box::new(handler));

    io_apic::enable_gsi(gsi);
}
