use crate::sys::acpi::{uacpi_status, uacpi};
use core::mem::size_of;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    Conforming,
    ActiveHigh,
    ActiveLow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerMode {
    Conforming,
    Edge,
    Level,
}

impl uacpi::acpi_madt_interrupt_source_override {
    pub fn polarity(&self) -> Option<Polarity> {
        match self.flags & 0b11 {
            0b00 => Some(Polarity::Conforming),
            0b01 => Some(Polarity::ActiveHigh),
            0b11 => Some(Polarity::ActiveLow),
            _ => None, // invalid per ACPI spec
        }
    }

    pub fn trigger_mode(&self) -> Option<TriggerMode> {
        match (self.flags >> 2) & 0b11 {
            0b00 => Some(TriggerMode::Conforming),
            0b01 => Some(TriggerMode::Edge),
            0b11 => Some(TriggerMode::Level),
            _ => None, // invalid per ACPI spec
        }
    }
}

pub struct IoApicData {
    pub io_apics: Vec<&'static uacpi::acpi_madt_ioapic>,
    pub isos: Vec<&'static uacpi::acpi_madt_interrupt_source_override>,
}

impl IoApicData {
    fn new() -> Self {
	Self {
	    io_apics: Vec::new(),
	    isos: Vec::new(),
	}
    }
}

pub unsafe fn get_madt() -> Result<&'static uacpi::acpi_madt, uacpi_status> {
    let sig = c"APIC".as_ptr();

    let mut madt_tbl = core::mem::MaybeUninit::<uacpi::uacpi_table>::uninit();
    let res = uacpi::uacpi_table_find_by_signature(sig, madt_tbl.as_mut_ptr());
    if res != uacpi_status::UACPI_STATUS_OK {
	return Err(res);
    }

    let madt_tbl = unsafe { madt_tbl.assume_init() };
    let madt_ptr = madt_tbl.__bindgen_anon_1.ptr as *const uacpi::acpi_madt;
    if madt_ptr.is_null() {
        return Err(uacpi_status::UACPI_STATUS_INTERNAL_ERROR);
    }

    Ok(&*madt_ptr)
}


pub fn iterate_madt_ioapics() -> Result<IoApicData, uacpi_status> {
    unsafe {
	let madt_ref = get_madt()?;

        // Calculate start and end of entries
        let base = madt_ref as *const uacpi::acpi_madt as *const u8;
        let entries_start = base.add(size_of::<uacpi::acpi_madt>());
        let entries_end = base.add(madt_ref.hdr.length as usize);

        let mut ptr = entries_start;
	let mut ret = IoApicData::new();

        while ptr < entries_end {
            let hdr = &*(ptr as *const uacpi::acpi_entry_hdr);

            // SAFETY: ACPI ensures length >= 2
            let entry_len = hdr.length as usize;
            if entry_len < size_of::<uacpi::acpi_entry_hdr>() {
                break; // malformed
            }

            // Convert entry type
            let Ok(entry_type) = uacpi::acpi_madt_entry_type::try_from(hdr.type_) else {
                ptr = ptr.add(entry_len);
                continue;
            };

            match entry_type {
                uacpi::acpi_madt_entry_type::ACPI_MADT_ENTRY_TYPE_IOAPIC => {
                    let ioapic = &*(ptr as *const uacpi::acpi_madt_ioapic);
		    ret.io_apics.push(ioapic);
                },
		uacpi::acpi_madt_entry_type::ACPI_MADT_ENTRY_TYPE_INTERRUPT_SOURCE_OVERRIDE => {
		    let iso = &*(ptr as *const uacpi::acpi_madt_interrupt_source_override);
		    ret.isos.push(iso);
		},

                _ => { /* Ignore other entries */ }
            }

            ptr = ptr.add(entry_len);
        }

	Ok(ret)
    }
}
