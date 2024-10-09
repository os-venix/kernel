#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

use bootloader_api;
use core::panic::PanicInfo;
use conquer_once::spin::OnceCell;
use printk;
use fixed::{types::extra::U3, FixedU64};
use crate::alloc::string::ToString;

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
    unsafe {
	let kernel_logger = match PRINTK.try_get() {
	    Ok(logger) => logger,
	    Err(_) => loop {} // Give up at this point
	};
	kernel_logger.force_unlock();
    }

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

    log::info!("Venix 0.0.1 - by Venos the Sergal :3");
    log::info!("Initialising CPU0...");
    log::info!("Memory map:");

    {
	let mut current_start: u64 = 0;
	for region_idx in 0 .. boot_info.memory_regions.len() {
	    if region_idx == 0 {
		current_start = boot_info.memory_regions[region_idx].start;
	    } else if boot_info.memory_regions[region_idx].start != boot_info.memory_regions[region_idx - 1].end {
		current_start = boot_info.memory_regions[region_idx].start;
	    } else if boot_info.memory_regions[region_idx].kind != boot_info.memory_regions[region_idx - 1].kind {
		current_start = boot_info.memory_regions[region_idx].start;
	    }

	    if region_idx == boot_info.memory_regions.len() - 1 {
		log::info!(
		    "    {:X} - {:X}: {:?}", current_start,
		    boot_info.memory_regions[region_idx].end, boot_info.memory_regions[region_idx].kind);
	    } else if boot_info.memory_regions[region_idx].end != boot_info.memory_regions[region_idx + 1].start {
		log::info!(
		    "    {:X} - {:X}: {:?}", current_start,
		    boot_info.memory_regions[region_idx].end, boot_info.memory_regions[region_idx].kind);
	    } else if boot_info.memory_regions[region_idx].kind != boot_info.memory_regions[region_idx + 1].kind {
		log::info!(
		    "    {:X} - {:X}: {:?}", current_start,
		    boot_info.memory_regions[region_idx].end, boot_info.memory_regions[region_idx].kind);
	    }
	}
    }

    gdt::init();
    interrupts::init_idt();
    memory::init(boot_info.recursive_index, &boot_info.memory_regions);
    allocator::init();

    memory::init_full_mode();
    let usable_ram = memory::get_usable_ram();
    log::info!("Total usable RAM: {} MiB", (FixedU64::<U3>::from_num(usable_ram) / FixedU64::<U3>::from_num(1024 * 1024)).to_string());
}

fn kernel_main(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    init(boot_info);
    loop {}
}
