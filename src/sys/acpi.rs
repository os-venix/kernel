use core::ptr::NonNull;
use core::ops::Deref;
use bootloader_api::info::Optional;
use acpi::{AcpiTables, AcpiHandler, PhysicalMapping};
pub use acpi::platform::interrupt::InterruptModel;
use x86_64::addr::PhysAddr;
use spin::{Once, RwLock};

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
	let start_phys_addr = phys_addr - (phys_addr % 4096);  // Page align
	let total_size = size + (phys_addr % 4096) + (4096 - (size % 4096));  // Total amount, aligned to page boundaries

	let allocated_region = match memory::allocate_contiguous_region_kernel(
	    total_size as u64, PhysAddr::new(start_phys_addr as u64), memory::MemoryAllocationType::MMIO) {
	    Ok(v) => v,
	    Err(e) => panic!("{:#?}", e),
	};

	let offset_from_start = phys_addr - start_phys_addr;
	let virt_addr = allocated_region + offset_from_start as u64;

	let ptr_to_t = virt_addr.as_mut_ptr();
	PhysicalMapping::new(phys_addr, NonNull::new(ptr_to_t).expect("Allocation was unsuccessful"), size, total_size, Self)
    }

    fn unmap_physical_region<T>(_region: &PhysicalMapping<Self, T>) {}
}

pub static ACPI: Once<RwLock<SyncAcpiTables<VenixAcpiHandler>>> = Once::new();

pub fn init(rdsp_addr: Optional<u64>) {
    let rdsp = rdsp_addr.into_option().expect("Unable to find ACPI tables.");

    let acpi = unsafe {
	AcpiTables::from_rsdp(VenixAcpiHandler, rdsp as usize)
    };

    match acpi {
	Ok(a) => {
	    ACPI.call_once(|| RwLock::new(SyncAcpiTables(a)));
	},
	Err(e) => panic!("{:#?}", e),
    }
}
