#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(allocator_api)]
#![feature(ascii_char)]
#![feature(ascii_char_variants)]
#![feature(extract_if)]

extern crate alloc;

use core::panic::PanicInfo;
use conquer_once::spin::OnceCell;
use fixed::{types::extra::U3, FixedU64};
use alloc::string::{String, ToString};

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

pub static PRINTK: OnceCell<printk::LockedPrintk> = OnceCell::uninit();

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    if let Some(printk) = PRINTK.get() {
	printk.clear();
    }
    log::error!("{}", info);
    loop {}
}

fn init() {
    assert!(BASE_REVISION.is_supported());

    if let Some(framebuffer_response) = FRAMEBUFFER_REQUEST.get_response() {
	if let Some(framebuffer) = framebuffer_response.framebuffers().next() {
	    let kernel_logger = PRINTK.get_or_init(move || printk::LockedPrintk::new(framebuffer));
	    log::set_logger(kernel_logger).expect("Logger already set");
	    log::set_max_level(log::LevelFilter::Trace);
	} else {
	    panic!();
	}
    } else {
	panic!();
    }

    log::info!("Venix 0.2.0 - by Venos the Sergal :3");
    log::info!("Initialising CPU0...");

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

    gdt::init();
    interrupts::init_idt();

    let direct_map_offset = HHDM_REQUEST.get_response().expect("Limine did not direct-map the higher half.").offset();
    memory::init(direct_map_offset, memory_map.entries());
    allocator::init();
    scheduler::init();

    memory::init_full_mode();
    let usable_ram = memory::get_usable_ram();
    log::info!("Total usable RAM: {} MiB", (FixedU64::<U3>::from_num(usable_ram) / FixedU64::<U3>::from_num(1024 * 1024)).to_string());
    let rsdp_addr = RSDP_REQUEST.get_response().expect("Limine did not return RDSP pointer.").address() as u64;
    sys::acpi::init(rsdp_addr - direct_map_offset);
    interrupts::init_handler_funcs();
    interrupts::init_bsp_apic();

    sys::vfs::init();
    driver::init();
    sys::block::init();
    drivers::init();

    driver::configure_drivers();

    sys::syscall::init();
}

extern "C" fn kmain() -> ! {
    init();

    let pid = match scheduler::elf_loader::load_elf(String::from("/init/init")) {
	Ok(pid) => pid,
	Err(e) => panic!("{}", e),
    };
    scheduler::switch_to(pid);
    scheduler::open_fd(String::from("/dev/console"));  // Stdin
    scheduler::open_fd(String::from("/dev/console"));  // Stdout
    scheduler::open_fd(String::from("/dev/console"));  // Stderr
    scheduler::start_active_process();
}
