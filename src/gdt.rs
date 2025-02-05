use x86_64::VirtAddr;
use x86_64::structures::tss::TaskStateSegment;
use x86_64::structures::gdt::{GlobalDescriptorTable, Descriptor, SegmentSelector};
use lazy_static::lazy_static;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

struct Selectors {
    code_selector: SegmentSelector,
    data_selector: SegmentSelector,
    user_dummy_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

lazy_static! {
    static ref TSS: TaskStateSegment = {
	let mut tss = TaskStateSegment::new();
	tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
	    const STACK_SIZE: usize = 4096 * 5;
	    static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

	    let stack_start = VirtAddr::from_ptr(&raw const STACK);
	    let stack_end = stack_start + STACK_SIZE as u64;
	    stack_end
	};
	tss.privilege_stack_table[0] = {
	    const STACK_SIZE: usize = 4096 * 5;
	    static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

	    let stack_start = VirtAddr::from_ptr(&raw const STACK);
	    let stack_end = stack_start + STACK_SIZE as u64;
	    stack_end
	};
	tss
    };

    static ref GDT: (GlobalDescriptorTable, Selectors) = {
	let mut gdt = GlobalDescriptorTable::new();
	let code_selector = gdt.append(Descriptor::kernel_code_segment());
	let data_selector = gdt.append(Descriptor::kernel_data_segment());
	let user_dummy_selector = gdt.append(Descriptor::kernel_code_segment());  // Dummy to make SYSREQ work
	gdt.append(Descriptor::user_data_segment());
	gdt.append(Descriptor::user_code_segment());
	let tss_selector = gdt.append(Descriptor::tss_segment(&TSS));
	(gdt, Selectors { code_selector, data_selector, user_dummy_selector, tss_selector })
    };
}

pub fn init() {
    use x86_64::instructions::tables::load_tss;
    use x86_64::instructions::segmentation::{CS, DS, Segment};

    GDT.0.load();
    TSS.privilege_stack_table[1] = memory::kernel_allocate_early(KERNEL_HEAP_SIZE as u64)
	.expect("Unable to allocate kernel stack").as_u64();
    unsafe {
	CS::set_reg(GDT.1.code_selector);
	DS::set_reg(GDT.1.data_selector);
	load_tss(GDT.1.tss_selector);
    }
}

pub fn init_full_mode() {
    TSS.privilege_stack_table[1] = 
}

pub fn get_code_selectors() -> (SegmentSelector, SegmentSelector) {
    (GDT.1.code_selector, GDT.1.user_dummy_selector)
}
