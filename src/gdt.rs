use x86_64::VirtAddr;
use x86_64::structures::tss::TaskStateSegment;
use x86_64::structures::gdt::{GlobalDescriptorTable, Descriptor, SegmentSelector};
use x86_64::instructions::tables::load_tss;
use x86_64::instructions::segmentation::{CS, DS, ES, FS, GS, Segment};
use x86_64::registers::model_specific::Msr;

use crate::memory;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
const IA32_FSBASE_MSR: u32 = 0xC0000100;
const IA32_GSBASE_MSR: u32 = 0xC0000101;
const IA32_KERNELGSBASE_MSR: u32 = 0xC0000102;

struct Selectors {
    code_selector: SegmentSelector,
    data_selector: SegmentSelector,
    user_dummy_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

#[repr(C, align(4096))]
pub struct ProcessorControlBlock {
    pub self_ptr: usize,

    pub gdt: GlobalDescriptorTable,
    pub gdt_selectors: Selectors,

    pub tss: TaskStateSegment,

    pub tmp_user_stack_ptr: usize,
}

pub fn init() {
    log::info!("{}", size_of::<ProcessorControlBlock>() as u64);
    let (mut pcb, pcb_ptr) = unsafe {
	let pcb = memory::kernel_allocate(
	    size_of::<ProcessorControlBlock>() as u64,
	    memory::MemoryAllocationType::RAM,
	    memory::MemoryAllocationOptions::Arbitrary,
	    memory::MemoryAccessRestriction::Kernel).expect("Unable to allocate BSP PCB");

	(&mut *(pcb.0.as_mut_ptr::<ProcessorControlBlock>()), pcb.0.as_u64())
    };
    log::info!("Test");

    pcb.self_ptr = pcb as *mut ProcessorControlBlock as usize;

    pcb.tss = TaskStateSegment::new();
    pcb.tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
	const STACK_SIZE: usize = 4096 * 5;
	static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

	let stack_start = VirtAddr::from_ptr(&raw const STACK);
	let stack_end = stack_start + STACK_SIZE as u64;
	stack_end
    };

    let stack_start = memory::kernel_allocate(
	1024 * 1024 * 8 as u64,    
	memory::MemoryAllocationType::RAM,
	memory::MemoryAllocationOptions::Arbitrary,
	memory::MemoryAccessRestriction::Kernel).expect("Unable to allocate kernel stack").0;
    pcb.tss.privilege_stack_table[0] = stack_start + (1024 * 1024 * 8);

    pcb.gdt = GlobalDescriptorTable::new();
    let code_selector = pcb.gdt.append(Descriptor::kernel_code_segment());
    let data_selector = pcb.gdt.append(Descriptor::kernel_data_segment());
    let user_dummy_selector = pcb.gdt.append(Descriptor::kernel_code_segment());  // Dummy to make SYSREQ work
    pcb.gdt.append(Descriptor::user_data_segment());
    pcb.gdt.append(Descriptor::user_code_segment());
    let tss_selector = pcb.gdt.append(Descriptor::tss_segment(&pcb.tss));

    pcb.gdt_selectors = Selectors { code_selector, data_selector, user_dummy_selector, tss_selector };
    
    pcb.gdt.load();
    unsafe {
	CS::set_reg(pcb.gdt_selectors.code_selector);
	DS::set_reg(pcb.gdt_selectors.data_selector);
	ES::set_reg(SegmentSelector(0));
	FS::set_reg(SegmentSelector(0));
	GS::set_reg(SegmentSelector(0));
	load_tss(pcb.gdt_selectors.tss_selector);
    }

    let mut fsbase_msr = Msr::new(IA32_FSBASE_MSR);
    unsafe {
	fsbase_msr.write(0);
    }
    let mut gsbase_msr = Msr::new(IA32_GSBASE_MSR);
    unsafe {
	gsbase_msr.write(pcb_ptr);
    }
    let mut kernelgsbase_msr = Msr::new(IA32_KERNELGSBASE_MSR);
    unsafe {
	kernelgsbase_msr.write(0);
    }
}

pub fn get_pcb() -> *mut ProcessorControlBlock {
    let mut ret: *mut ProcessorControlBlock;
    unsafe {
	core::arch::asm!("mov {}, gs:[{}]", out(reg) ret, const(core::mem::offset_of!(ProcessorControlBlock, self_ptr)));
    }
    ret
}

pub fn get_code_selectors() -> (SegmentSelector, SegmentSelector) {
    let pcb = get_pcb();
    unsafe {
	((*pcb).gdt_selectors.code_selector, (*pcb).gdt_selectors.user_dummy_selector)
    }
}
