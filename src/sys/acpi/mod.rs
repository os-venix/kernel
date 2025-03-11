use core::ptr::NonNull;
use core::ops::Deref;
use acpi::{AcpiTables, AcpiHandler, PhysicalMapping, AmlTable};
pub use acpi::platform::interrupt::InterruptModel;
use aml::{AmlContext, Handler, DebugVerbosity};
use spin::{Once, RwLock};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;

use crate::memory;

mod uacpi;

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
	let (ptr_to_t, total_size) = match memory::allocate_arbitrary_contiguous_region_kernel(
	    phys_addr, size, memory::MemoryAllocationType::MMIO) {
	    Ok((ptr_to_t, total_size)) => (ptr_to_t, total_size),
	    Err(e) => panic!("{:#?}", e),
	};
	PhysicalMapping::new(
	    phys_addr, NonNull::new(ptr_to_t.as_mut_ptr()).expect("Allocation was unsuccessful"), size, total_size, Self)
    }

    fn unmap_physical_region<T>(_region: &PhysicalMapping<Self, T>) {}
}

#[derive(Clone)]
struct VenixAmlHandler;
impl Handler for VenixAmlHandler {
    fn read_u8(&self, address: usize) -> u8 {
	let (ptr_to_t, _) = match memory::allocate_arbitrary_contiguous_region_kernel(
	    address, 1, memory::MemoryAllocationType::MMIO) {
	    Ok((ptr_to_t, total_size)) => (ptr_to_t, total_size),
	    Err(e) => panic!("{:#?}", e),
	};

        unsafe { *ptr_to_t.as_ptr::<u8>() }
    }
    fn read_u16(&self, address: usize) -> u16 {
	let (ptr_to_t, _) = match memory::allocate_arbitrary_contiguous_region_kernel(
	    address, 2, memory::MemoryAllocationType::MMIO) {
	    Ok((ptr_to_t, total_size)) => (ptr_to_t, total_size),
	    Err(e) => panic!("{:#?}", e),
	};

        unsafe { *ptr_to_t.as_ptr::<u16>() }
    }
    fn read_u32(&self, address: usize) -> u32 {
	let (ptr_to_t, _) = match memory::allocate_arbitrary_contiguous_region_kernel(
	    address, 4, memory::MemoryAllocationType::MMIO) {
	    Ok((ptr_to_t, total_size)) => (ptr_to_t, total_size),
	    Err(e) => panic!("{:#?}", e),
	};

        unsafe { *ptr_to_t.as_ptr::<u32>() }
    }
    fn read_u64(&self, address: usize) -> u64 {
	let (ptr_to_t, _) = match memory::allocate_arbitrary_contiguous_region_kernel(
	    address, 8, memory::MemoryAllocationType::MMIO) {
	    Ok((ptr_to_t, total_size)) => (ptr_to_t, total_size),
	    Err(e) => panic!("{:#?}", e),
	};

        unsafe { *ptr_to_t.as_ptr::<u64>() }
    }

    fn write_u8(&mut self, address: usize, val: u8) {
	let (ptr_to_t, _) = match memory::allocate_arbitrary_contiguous_region_kernel(
	    address, 1, memory::MemoryAllocationType::MMIO) {
	    Ok((ptr_to_t, total_size)) => (ptr_to_t, total_size),
	    Err(e) => panic!("{:#?}", e),
	};

        unsafe { *ptr_to_t.as_mut_ptr::<u8>() = val }
    }
    fn write_u16(&mut self, address: usize, val: u16) {
	let (ptr_to_t, _) = match memory::allocate_arbitrary_contiguous_region_kernel(
	    address, 2, memory::MemoryAllocationType::MMIO) {
	    Ok((ptr_to_t, total_size)) => (ptr_to_t, total_size),
	    Err(e) => panic!("{:#?}", e),
	};

        unsafe { *ptr_to_t.as_mut_ptr::<u16>() = val }
    }
    fn write_u32(&mut self, address: usize, val: u32) {
	let (ptr_to_t, _) = match memory::allocate_arbitrary_contiguous_region_kernel(
	    address, 4, memory::MemoryAllocationType::MMIO) {
	    Ok((ptr_to_t, total_size)) => (ptr_to_t, total_size),
	    Err(e) => panic!("{:#?}", e),
	};

        unsafe { *ptr_to_t.as_mut_ptr::<u32>() = val }
    }
    fn write_u64(&mut self, address: usize, val: u64) {
	let (ptr_to_t, _) = match memory::allocate_arbitrary_contiguous_region_kernel(
	    address, 8, memory::MemoryAllocationType::MMIO) {
	    Ok((ptr_to_t, total_size)) => (ptr_to_t, total_size),
	    Err(e) => panic!("{:#?}", e),
	};

        unsafe { *ptr_to_t.as_mut_ptr::<u64>() = val }
    }

    fn read_io_u8(&self, _: u16) -> u8 {
        unimplemented!()
    }
    fn read_io_u16(&self, _: u16) -> u16 {
        unimplemented!()
    }
    fn read_io_u32(&self, _: u16) -> u32 {
        unimplemented!()
    }
    fn write_io_u8(&self, _: u16, _: u8) {
        unimplemented!()
    }
    fn write_io_u16(&self, _: u16, _: u16) {
        unimplemented!()
    }
    fn write_io_u32(&self, _: u16, _: u32) {
        unimplemented!()
    }
    fn read_pci_u8(&self, _: u16, _: u8, _: u8, _: u8, _: u16) -> u8 {
        unimplemented!()
    }
    fn read_pci_u16(&self, _: u16, _: u8, _: u8, _: u8, _: u16) -> u16 {
        unimplemented!()
    }
    fn read_pci_u32(&self, _: u16, _: u8, _: u8, _: u8, _: u16) -> u32 {
        unimplemented!()
    }
    fn write_pci_u8(&self, _: u16, _: u8, _: u8, _: u8, _: u16, _: u8) {
        unimplemented!()
    }
    fn write_pci_u16(&self, _: u16, _: u8, _: u8, _: u8, _: u16, _: u16) {
        unimplemented!()
    }
    fn write_pci_u32(&self, _: u16, _: u8, _: u8, _: u8, _: u16, _: u32) {
        unimplemented!()
    }
}

pub static ACPI: Once<RwLock<SyncAcpiTables<VenixAcpiHandler>>> = Once::new();
pub static AML: Once<RwLock<AmlContext>> = Once::new();

fn map_aml_table(tbl: AmlTable, aml: &mut AmlContext) {
    let ptr_to_tbl = match memory::allocate_arbitrary_contiguous_region_kernel(
	tbl.address, tbl.length as usize, memory::MemoryAllocationType::MMIO) {
	Ok((ptr_to_t, _)) => ptr_to_t.as_ptr(),
	Err(e) => panic!("{:#?}", e),
    };
    let tbl_slice = unsafe { core::slice::from_raw_parts(ptr_to_tbl as *const u8, tbl.length as usize) };

    aml.parse_table(tbl_slice).expect("Failed to parse DSDT.");
}

pub fn init(rdsp_addr: u64) {
    uacpi::init();
    let acpi = unsafe {
	AcpiTables::from_rsdp(VenixAcpiHandler, rdsp_addr as usize)
    };

    match acpi {
	Ok(a) => {
	    ACPI.call_once(|| RwLock::new(SyncAcpiTables(a)));
	},
	Err(e) => panic!("{:#?}", e),
    }

    {
	let acpi_tables = ACPI.get().expect("Unable to access ACPI tables after initialisation.").read();
	let mut aml = AmlContext::new(Box::new(VenixAmlHandler), DebugVerbosity::None);
	let dsdt = match acpi_tables.dsdt() {
	    Ok(d) => d,
	    Err(e) => panic!("{:#?}", e),
	};
	map_aml_table(dsdt, &mut aml);

	for ssdt in acpi_tables.ssdts() {
	    map_aml_table(ssdt, &mut aml);
	}	

	AML.call_once(|| RwLock::new(aml));
    }
}

pub fn eisa_id_to_string(eisa_id: u64) -> String {
    let c1 = char::from_u32(0x40 + ((eisa_id & 0x7C) >> 2) as u32).expect("Unable to decode EISA string");
    let c2 = char::from_u32(0x40 + (((eisa_id & 0x03) << 3) | ((eisa_id & 0xE000) >> 13)) as u32).expect("Unable to decode EISA string");
    let c3 = char::from_u32(0x40 + ((eisa_id & 0x1F00) >> 8) as u32).expect("Unable to decode EISA string");

    format!("{}{}{}{:02X}{:02X}", c1, c2, c3, (eisa_id & 0x00FF0000) >> 16, (eisa_id & 0xFF000000) >> 24)
}
