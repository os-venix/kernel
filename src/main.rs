#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

use bootloader_api;
use core::panic::PanicInfo;
use conquer_once::spin::OnceCell;
use printk;

mod interrupts;
mod gdt;

const CONFIG: bootloader_api::BootloaderConfig = {
    let mut config = bootloader_api::BootloaderConfig::new_default();
    config.kernel_stack_size = 100 * 1024; // 100 KiB
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
}

fn kernel_main(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    init(boot_info);
    loop {}
}
