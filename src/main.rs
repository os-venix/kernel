#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(allocator_api)]
#![feature(ascii_char)]
#![feature(ascii_char_variants)]
#![feature(extract_if)]
#![feature(naked_functions)]
#![feature(alloc_layout_extra)]

extern crate alloc;

use core::panic::PanicInfo;
use fixed::{types::extra::U3, FixedU64};
use alloc::string::ToString;
use spin::Once;
use alloc::vec;
use alloc::vec::Vec;
use alloc::ffi::CString;

use limine::request::{
    EntryPointRequest,
    FramebufferRequest,
    MemoryMapRequest,
    HhdmRequest,
    RsdpRequest,
    StackSizeRequest,
    RequestsEndMarker,
    RequestsStartMarker
};
use limine::BaseRevision;
use limine::memory_map::EntryType;

mod interrupts;
mod gdt;
mod memory;
mod allocator;
mod sys;
mod drivers;
mod driver;
mod printk;
mod fs;
mod scheduler;
mod dma;
mod utils;
mod console;

use crate::sys::syscall;

#[used]
#[link_section = ".requests"]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
#[link_section = ".requests"]
static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

#[used]
#[link_section = ".requests"]
static MEMORY_MAP_REQUEST: MemoryMapRequest = MemoryMapRequest::new();

#[used]
#[link_section = ".requests"]
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[used]
#[link_section = ".requests"]
static RSDP_REQUEST: RsdpRequest = RsdpRequest::new();

#[used]
#[link_section = ".requests"]
static ENTRY_POINT_REQUEST: EntryPointRequest = EntryPointRequest::new().with_entry_point(kmain);

#[used]
#[link_section = ".requests"]
static STACK_SIZE_REQUEST: StackSizeRequest = StackSizeRequest::new().with_size(200 * 1024);

#[used]
#[link_section = ".requests_start_marker"]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();

#[used]
#[link_section = ".requests_end_marker"]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

pub static PRINTK: Once<printk::LockedPrintk> = Once::new();

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // unsafe {
    // 	let kernel_logger = match PRINTK.try_get() {
    // 	    Ok(logger) => logger,
    // 	    Err(_) => loop {}  // Give up at this point
    // 	};
    // 	kernel_logger.force_unlock();
    // }

    // if let Some(printk) = PRINTK.get() {
    // 	printk.clear();
    // }
    log::error!("{}", info);
    loop {}
}

fn init() {
    assert!(BASE_REVISION.is_supported());

    if let Some(framebuffer_response) = FRAMEBUFFER_REQUEST.get_response() {
	if let Some(framebuffer) = framebuffer_response.framebuffers().next() {
	    let kernel_logger = PRINTK.call_once(move || printk::LockedPrintk::new(framebuffer));
	    log::set_logger(kernel_logger).expect("Logger already set");
	    log::set_max_level(log::LevelFilter::Trace);
	} else {
	    panic!();
	}
    } else {
	panic!();
    }

    log::info!("Venix 0.4.0 - by Venos the Sergal :3");
    log::info!("Initialising CPU0...");
    sys::init();

    let memory_map = MEMORY_MAP_REQUEST.get_response().expect("Limine did not return a memory map.");
    log::info!("Memory map:");

    for entry in memory_map.entries() {
	log::info!("   {:X} - {:X}: {:?}", entry.base, entry.base + entry.length, match entry.entry_type {
	    EntryType::USABLE => "Usable",
	    EntryType::RESERVED => "Reserved",
	    EntryType::ACPI_RECLAIMABLE => "ACPI Reclaimable",
	    EntryType::ACPI_NVS => "ACPI NVS",
	    EntryType::BAD_MEMORY => "Bad Memory",
	    EntryType::BOOTLOADER_RECLAIMABLE => "Bootloader reclamiable",
	    EntryType::KERNEL_AND_MODULES => "Kernel (and modules)",
	    EntryType::FRAMEBUFFER => "Framebuffer",
	    _ => "Unknown",
	});
    }

    let direct_map_offset = HHDM_REQUEST.get_response().expect("Limine did not direct-map the higher half.").offset();
    memory::init(direct_map_offset, memory_map.entries());
    allocator::init();
    memory::init_full_mode();

    log::info!("Bringing up BSP");
    gdt::init();
    interrupts::init_idt();
    interrupts::init_handler_funcs();
    drivers::pcie::init_pci_subsystem_for_acpi();

    let usable_ram = memory::get_usable_ram();
    log::info!("Total usable RAM: {} MiB", (FixedU64::<U3>::from_num(usable_ram) / FixedU64::<U3>::from_num(1024 * 1024)).to_string());

    let rsdp_addr = RSDP_REQUEST.get_response().expect("Limine did not return RDSP pointer.").address() as u64;
    sys::acpi::init(rsdp_addr - direct_map_offset);
    interrupts::init_bsp_apic();
    sys::acpi::init_aml(rsdp_addr - direct_map_offset);
    interrupts::enable_interrupts();

    scheduler::init();
    sys::vfs::init();
    driver::init();
    console::init();
    sys::block::init();
    drivers::init();

    driver::configure_drivers();

    sys::syscall::init();
}

extern "C" fn kmain() -> ! {
    init();

    if let Some(printk) = PRINTK.get() {
	printk.clear();
    }

    scheduler::kthread_start(init_setup);
    scheduler::start();
}

// TODO - this will need to mount the rootfs, as that can no longer happen in the boot context due to async code
// TODO - anywhere where a syscall will write to user memory, expectations now break; before, we were snooping memory from current PID. That doens't work any more.
fn init_setup() -> ! {
    // First, open stdin, stdout, and stderr
    let console_cstring = CString::new("/dev/console").unwrap();
    let console_ptr = console_cstring.as_ptr() as u64;

    unsafe {
	syscall::do_syscall6(0x02, console_ptr, 0, 0, 0, 0, 0);  // Open stdin
	syscall::do_syscall6(0x02, console_ptr, 0, 0, 0, 0, 0);  // Open stdout
	syscall::do_syscall6(0x02, console_ptr, 0, 0, 0, 0, 0);  // Open stderr
    }

    // Second, actually run init
    let path_cstring = CString::new("/init/init").unwrap();
    let args_strs: Vec<&str> = vec![];
    let env_strs: Vec<&str> = vec!["PATH=/bin", "USER=root", "LD_SHOW_AUXV=1"];

    let args_cstrings: Vec<CString> = args_strs.iter()
        .map(|s| CString::new(*s).unwrap())
        .collect();
    let env_cstrings: Vec<CString> = env_strs.iter()
        .map(|s| CString::new(*s).unwrap())
        .collect();

    let mut args_ptrs: Vec<u64> = args_cstrings.iter()
        .map(|cstr| cstr.as_ptr() as u64)
        .collect();
    args_ptrs.push(0);

    let mut env_ptrs: Vec<u64> = env_cstrings.iter()
        .map(|cstr| cstr.as_ptr() as u64)
        .collect();
    env_ptrs.push(0);

    // Keep all these alive while syscall runs
    let path_ptr = path_cstring.as_ptr() as u64;
    let args_ptr = args_ptrs.as_ptr() as u64;
    let envvars_ptr = env_ptrs.as_ptr() as u64;

    // Loop until we can stat init executable; that implies the filesystem has been successfully mounted
    loop {
	let (ret, err) = unsafe {
	    syscall::do_syscall6(0x05, path_ptr, 0, 0, 0, 0, 0)
	};
	if ret == 0 {
	    break;
	}
    }

    // Actually run init
    unsafe {
	syscall::do_syscall6(0x3b, path_ptr, args_ptr, envvars_ptr, 0, 0, 0);
    }

    loop {}  // This shouldn't happen, due to the execve above. However, Rust needs it to satisfy bounds checks
}
