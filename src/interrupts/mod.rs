use alloc::boxed::Box;

use crate::sys::acpi;

mod local_apic;
mod io_apic;
mod idt;

const IRQ_BASE: u8 = 32;

#[derive(Clone, Debug)]
pub enum InterruptRoute {
    Gsi(u32),
    Irq(u8),
}

impl InterruptRoute {
    pub fn register_handler(&self, handler: Box<dyn Fn() + Send + Sync>) {
	match self {
	    InterruptRoute::Gsi(gsi) => {
		let irq = io_apic::get_irq_for_gsi(*gsi);
		log::info!("GSI = {}, IRQ = {}", gsi, irq);
		idt::add_handler_to_irq(irq, handler);

		io_apic::enable_gsi(*gsi);
	    },
	    InterruptRoute::Irq(irq) => {
		log::info!("IRQ = {}", irq + IRQ_BASE);
		idt::add_handler_to_irq(irq + IRQ_BASE, handler);

		io_apic::enable_irq(*irq);
	    }
	}
    }
}

pub fn init_idt() {
    idt::init();
}

pub fn init_handler_funcs() {
    idt::init_handlers();
}

pub fn init_bsp_apic() {
    let bsp_apic_id = local_apic::init_bsp_local_apic();
    io_apic::init_io_apics(bsp_apic_id);
}

pub fn enable_interrupts() {
    acpi::set_interrupt_model(acpi::InterruptModel::IoApic).expect("Unable to switch into IO APIC mode");    
}

pub fn enable_gsi(gsi: u32, handler: &'static (dyn Fn() + Send + Sync)) {
    let irq = io_apic::get_irq_for_gsi(gsi);
    log::info!("GSI = {}, IRQ = {}", gsi, irq);
    idt::add_handler_to_irq(irq, Box::new(handler));

    io_apic::enable_gsi(gsi);
}
