use acpi::{AcpiHandler, AcpiTables, PhysicalMapping};
use core::ops::Deref;
use core::ptr::NonNull;
use spin::{Once, RwLock};
use x86_64::PhysAddr;

use crate::memory;

pub struct SyncAcpiTables<H: AcpiHandler>(AcpiTables<H>);
unsafe impl<H: AcpiHandler> Sync for SyncAcpiTables<H> {}

impl<H: AcpiHandler> Deref for SyncAcpiTables<H> {
    type Target = AcpiTables<H>;

    fn deref(&self) -> &AcpiTables<H> {
	&self.0
    }
}

#[derive(Clone)]
pub struct VenixAcpiHandler;
impl AcpiHandler for VenixAcpiHandler {
    unsafe fn map_physical_region<T>(&self, phys_addr: usize, size: usize) -> PhysicalMapping<Self, T> {
	PhysicalMapping::new(
	    phys_addr, NonNull::new(
		memory::get_ptr_in_hhdm(PhysAddr::new(phys_addr as u64)).as_mut_ptr())
		.expect("Got a null pointer"),
	    size, size, Self)
    }

    fn unmap_physical_region<T>(_region: &PhysicalMapping<Self, T>) {}
}

pub static ACPI: Once<RwLock<SyncAcpiTables<VenixAcpiHandler>>> = Once::new();

pub fn init(rdsp_addr: u64) {
    let acpi = unsafe {
	AcpiTables::from_rsdp(VenixAcpiHandler, rdsp_addr as usize)
    };

    match acpi {
	Ok(a) => {
            ACPI.call_once(|| RwLock::new(SyncAcpiTables(a)));
	},
	Err(e) => panic!("{:#?}", e),
    }
}
