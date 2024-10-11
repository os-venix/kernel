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

    x86_64::instructions::interrupts::enable();
}
