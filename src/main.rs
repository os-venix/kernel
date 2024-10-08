#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

use bootloader_api;
use core::panic::PanicInfo;
use conquer_once::spin::OnceCell;
use printk;

use alloc::vec::Vec;

mod interrupts;
mod gdt;
mod memory;
mod allocator;

const CONFIG: bootloader_api::BootloaderConfig = {
    let mut config = bootloader_api::BootloaderConfig::new_default();
    config.kernel_stack_size = 100 * 1024; // 100 KiB
    config.mappings.page_table_recursive = Some(bootloader_api::config::Mapping::Dynamic);
    config
};
bootloader_api::entry_point!(kernel_main, config = &CONFIG);

pub static PRINTK: OnceCell<printk::LockedPrintk> = OnceCell::uninit();

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log::error!("{}", info);
    loop {}
}

fn init(boot_info: &'static mut bootloader_api::BootInfo) {
    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
	let info = framebuffer.info();
	let buffer = framebuffer.buffer_mut();

	let kernel_logger = PRINTK.get_or_init(move || printk::LockedPrintk::new(buffer, info));
	log::set_logger(kernel_logger).expect("Logger already set");
	log::set_max_level(log::LevelFilter::Trace);
    } else {
	panic!();
    }

    log::info!("Initialising CPU0...");
    gdt::init();
    interrupts::init_idt();
    memory::init(boot_info.recursive_index, &boot_info.memory_regions);
    allocator::init();

    memory::init_full_mode();

    let mut vec = Vec::new();
    for i in 0 .. 500 {
	vec.push(i);
    }

    log::info!("Vec at {:p}", vec.as_slice());
}

fn kernel_main(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    init(boot_info);
    loop {}
}
