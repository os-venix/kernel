use alloc::format;
use alloc::string::String;

mod uacpi;
mod acpi_lock;
mod tables;
pub mod resources;
pub mod namespace;

pub use uacpi::{InterruptModel, Namespace, UacpiIterationDecision, UacpiStatus, UacpiObjectType, UacpiIdString};
pub use tables::ACPI;

pub fn init(rdsp_addr: u64) {
    tables::init(rdsp_addr);
}

pub fn init_aml(rdsp_addr: u64) {
    uacpi::init(rdsp_addr);
}

pub fn eisa_id_to_string(eisa_id: u64) -> String {
    let c1 = char::from_u32(0x40 + ((eisa_id & 0x7C) >> 2) as u32).expect("Unable to decode EISA string");
    let c2 = char::from_u32(0x40 + (((eisa_id & 0x03) << 3) | ((eisa_id & 0xE000) >> 13)) as u32).expect("Unable to decode EISA string");
    let c3 = char::from_u32(0x40 + ((eisa_id & 0x1F00) >> 8) as u32).expect("Unable to decode EISA string");

    format!("{}{}{}{:02X}{:02X}", c1, c2, c3, (eisa_id & 0x00FF0000) >> 16, (eisa_id & 0xFF000000) >> 24)
}

pub fn set_interrupt_model(model: InterruptModel) -> Result<(), UacpiStatus> {
    let ret = unsafe {
	uacpi::uacpi_set_interrupt_model(model)
    };

    match ret {
	UacpiStatus::Ok => Ok(()),
	e => Err(e),
    }
}
