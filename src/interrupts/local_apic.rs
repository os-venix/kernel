use ::acpi::InterruptModel;
use x86_64::registers::model_specific::Msr;
use raw_cpuid::CpuId;
use pic8259::ChainedPics;
use spin::RwLock;

use crate::interrupts::IRQ_BASE;
use crate::sys::acpi;

const PIC_2_OFFSET: u8 = IRQ_BASE + 8;

const IA32_APIC_BASE_MSR: u32 = 0x1B;
const IA32_APIC_BASE_MSR_IS_BSP: u64 = 1 << 8;
const IA32_APIC_BASE_MSR_EXTD: u64 = 1 << 10;
const IA32_APIC_BASE_MSR_ENABLE: u64 = 1 << 11;

const IA32_X2APIC_SIVR: u32 = 0x80F;
const IA32_X2APIC_SIVR_VECTOR: u64 = 0xFF;
const IA32_X2APIC_SIVR_EN: u64 = 1 << 8;

const IA32_X2APIC_IDR: u32 = 0x802;
const IA32_X2APIC_EOI: u32 = 0x80B;

static PICS: RwLock<ChainedPics> = RwLock::new(unsafe { ChainedPics::new(IRQ_BASE, PIC_2_OFFSET) });

pub fn init_bsp_local_apic() -> u64 {
    let cpu_id = CpuId::new();
    let features = cpu_id.get_feature_info().expect("CPUID get features info failed.");

    // Check that we have an APIC
    if !features.has_apic() {
	panic!("System does not have a Local APIC. CPU not supported.");
    }

    if !features.has_x2apic() {
	panic!("System APIC does not support X2 mode. CPU not supported.");
    }

    {
	let acpi = acpi::ACPI.get().expect("Attempted to access ACPI tables before ACPI is initialised").read();
	let platform_info = match acpi.platform_info() {
	    Ok(pi) => pi,
	    Err(e) => panic!("{:#?}", e),
	};

	let interrupt_model = match platform_info.interrupt_model {
	    InterruptModel::Unknown => panic!("ACPI reports no APIC presence. CPU not supported."),
	    InterruptModel::Apic(a) => a,
	    _ => panic!("Unrecognised interrupt model."),
	};

	if interrupt_model.also_has_legacy_pics {
	    log::info!("Legacy PIC is present. Remapping.");
	    remap_pics();
	}
    }

    // Get the base address of the APIC
    let mut ia32_apic_base_msr = Msr::new(IA32_APIC_BASE_MSR);
    let base_msr_val = unsafe {
	ia32_apic_base_msr.read()
    };

    if base_msr_val & IA32_APIC_BASE_MSR_IS_BSP == 0 {
	panic!("Attempted to initialise BSP APIC on an AP");
    }

    // Enable the APIC in X2 mode
    unsafe {
	ia32_apic_base_msr.write(base_msr_val | IA32_APIC_BASE_MSR_ENABLE | IA32_APIC_BASE_MSR_EXTD);
    }

    // Enable the APIC using the Spurious Interrupt Vector Register
    let mut ia32_x2apic_sivr = Msr::new(IA32_X2APIC_SIVR);
    unsafe {
	ia32_x2apic_sivr.write(IA32_X2APIC_SIVR_VECTOR | IA32_X2APIC_SIVR_EN);
    }

    let ia32_x2apic_idr = Msr::new(IA32_X2APIC_IDR);
    let bsp_apic_id = unsafe {
	ia32_x2apic_idr.read()
    };

    bsp_apic_id
}

pub fn ack_apic() {
    let mut ia32_x2apic_eoi = Msr::new(IA32_X2APIC_EOI);
    unsafe {
	ia32_x2apic_eoi.write(0);
    }
}

fn remap_pics() {
    let mut pics = PICS.write();

    unsafe {
	pics.initialize();
	pics.disable();
    }
}
