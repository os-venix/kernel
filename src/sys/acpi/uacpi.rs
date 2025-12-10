#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use core::alloc::{GlobalAlloc, Layout};
use core::ffi::{c_char, c_void, CStr};
use core::fmt;
use core::fmt::Display;
use pci_types::{ConfigRegionAccess, PciAddress};
use spin;
use spin::Once;
use x86_64::PhysAddr;
use x86_64::instructions::port::Port;
use x86_64::registers::rflags;

use crate::sys::acpi::acpi_lock::{Mutex, Semaphore};
use crate::drivers::pcie;
use crate::memory;
use crate::interrupts;
use crate::allocator;

include!(concat!(env!("OUT_DIR"), "/uacpi_bindings.rs"));

impl core::convert::TryFrom<u32> for uacpi_resource_type {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        use uacpi_resource_type::*;

        Ok(match value {
            x if x == UACPI_RESOURCE_TYPE_IRQ as u32 => UACPI_RESOURCE_TYPE_IRQ,
            x if x == UACPI_RESOURCE_TYPE_EXTENDED_IRQ as u32 => UACPI_RESOURCE_TYPE_EXTENDED_IRQ,
            x if x == UACPI_RESOURCE_TYPE_DMA as u32 => UACPI_RESOURCE_TYPE_DMA,
            x if x == UACPI_RESOURCE_TYPE_FIXED_DMA as u32 => UACPI_RESOURCE_TYPE_FIXED_DMA,
            x if x == UACPI_RESOURCE_TYPE_IO as u32 => UACPI_RESOURCE_TYPE_IO,
            x if x == UACPI_RESOURCE_TYPE_FIXED_IO as u32 => UACPI_RESOURCE_TYPE_FIXED_IO,
            x if x == UACPI_RESOURCE_TYPE_ADDRESS16 as u32 => UACPI_RESOURCE_TYPE_ADDRESS16,
            x if x == UACPI_RESOURCE_TYPE_ADDRESS32 as u32 => UACPI_RESOURCE_TYPE_ADDRESS32,
            x if x == UACPI_RESOURCE_TYPE_ADDRESS64 as u32 => UACPI_RESOURCE_TYPE_ADDRESS64,
            x if x == UACPI_RESOURCE_TYPE_ADDRESS64_EXTENDED as u32 =>
                UACPI_RESOURCE_TYPE_ADDRESS64_EXTENDED,
            x if x == UACPI_RESOURCE_TYPE_MEMORY24 as u32 => UACPI_RESOURCE_TYPE_MEMORY24,
            x if x == UACPI_RESOURCE_TYPE_MEMORY32 as u32 => UACPI_RESOURCE_TYPE_MEMORY32,
            x if x == UACPI_RESOURCE_TYPE_FIXED_MEMORY32 as u32 => UACPI_RESOURCE_TYPE_FIXED_MEMORY32,
            x if x == UACPI_RESOURCE_TYPE_START_DEPENDENT as u32 => UACPI_RESOURCE_TYPE_START_DEPENDENT,
            x if x == UACPI_RESOURCE_TYPE_END_DEPENDENT as u32 => UACPI_RESOURCE_TYPE_END_DEPENDENT,
            x if x == UACPI_RESOURCE_TYPE_VENDOR_SMALL as u32 => UACPI_RESOURCE_TYPE_VENDOR_SMALL,
            x if x == UACPI_RESOURCE_TYPE_VENDOR_LARGE as u32 => UACPI_RESOURCE_TYPE_VENDOR_LARGE,
            x if x == UACPI_RESOURCE_TYPE_GENERIC_REGISTER as u32 => UACPI_RESOURCE_TYPE_GENERIC_REGISTER,
            x if x == UACPI_RESOURCE_TYPE_GPIO_CONNECTION as u32 => UACPI_RESOURCE_TYPE_GPIO_CONNECTION,
            x if x == UACPI_RESOURCE_TYPE_SERIAL_I2C_CONNECTION as u32 =>
                UACPI_RESOURCE_TYPE_SERIAL_I2C_CONNECTION,
            x if x == UACPI_RESOURCE_TYPE_SERIAL_SPI_CONNECTION as u32 =>
                UACPI_RESOURCE_TYPE_SERIAL_SPI_CONNECTION,
            x if x == UACPI_RESOURCE_TYPE_SERIAL_UART_CONNECTION as u32 =>
                UACPI_RESOURCE_TYPE_SERIAL_UART_CONNECTION,
            x if x == UACPI_RESOURCE_TYPE_SERIAL_CSI2_CONNECTION as u32 =>
                UACPI_RESOURCE_TYPE_SERIAL_CSI2_CONNECTION,
            x if x == UACPI_RESOURCE_TYPE_PIN_FUNCTION as u32 => UACPI_RESOURCE_TYPE_PIN_FUNCTION,
            x if x == UACPI_RESOURCE_TYPE_PIN_CONFIGURATION as u32 =>
                UACPI_RESOURCE_TYPE_PIN_CONFIGURATION,
            x if x == UACPI_RESOURCE_TYPE_PIN_GROUP as u32 => UACPI_RESOURCE_TYPE_PIN_GROUP,
            x if x == UACPI_RESOURCE_TYPE_PIN_GROUP_FUNCTION as u32 =>
                UACPI_RESOURCE_TYPE_PIN_GROUP_FUNCTION,
            x if x == UACPI_RESOURCE_TYPE_PIN_GROUP_CONFIGURATION as u32 =>
                UACPI_RESOURCE_TYPE_PIN_GROUP_CONFIGURATION,
            x if x == UACPI_RESOURCE_TYPE_CLOCK_INPUT as u32 => UACPI_RESOURCE_TYPE_CLOCK_INPUT,
            x if x == UACPI_RESOURCE_TYPE_END_TAG as u32 => UACPI_RESOURCE_TYPE_END_TAG,
            _ => return Err(()),
        })
    }
}

impl Display for uacpi_id_string {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SAFETY: `self.contents` must be a valid NUL-terminated C string.
        let cstr = unsafe { CStr::from_ptr(self.value) };
        let s = String::from_utf8_lossy(cstr.to_bytes());
        f.write_str(&s)
    }
}

#[derive(Copy, Clone)]
#[allow(dead_code)]
struct PortRange {
    pub base: u16,
    pub length: u16,
}

static RDSP_PHYS_PTR: Once<u64> = Once::new();
static ACPI_ALLOCS: Once<spin::Mutex<BTreeMap<usize, usize>>> = Once::new();

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_initialize(_uacpi_init_level: uacpi_init_level) -> uacpi_status {
    uacpi_status::UACPI_STATUS_OK
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_deinitialize() { }

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_device_open(address: uacpi_pci_address, handle: *mut *mut PciAddress) -> uacpi_status {
    let address = Box::new(PciAddress::new(address.segment, address.bus, address.device, address.function));
    unsafe {
	*handle = Box::into_raw(address);
    }

    uacpi_status::UACPI_STATUS_OK
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_device_close(handle: *mut PciAddress) {
    let _: Box<PciAddress> = unsafe { Box::from_raw(handle) };
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_read8(address: *mut PciAddress, offset: usize, out: *mut u8) -> uacpi_status {
    let pci_address = if let Some(r) = unsafe { address.as_ref() } {
	r
    } else {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    };

    let pci_access = pcie::PCI_ACCESS.get().unwrap().lock();
    let val = unsafe { pci_access.read(*pci_address, offset as u16 & 0xFC) } >> ((offset & 3) * 8);
    
    unsafe {
	*out = val as u8;
    }

    uacpi_status::UACPI_STATUS_OK
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_read16(address: *mut PciAddress, offset: usize, out: *mut u16) -> uacpi_status {
    let pci_address = if let Some(r) = unsafe { address.as_ref() } {
	r
    } else {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    };

    let pci_access = pcie::PCI_ACCESS.get().unwrap().lock();
    let val = unsafe { pci_access.read(*pci_address, offset as u16 & 0xFC) } >> ((offset & 2) * 8);
    
    unsafe {
	*out = val as u16;
    }

    uacpi_status::UACPI_STATUS_OK
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_read32(address: *mut PciAddress, offset: usize, out: *mut u32) -> uacpi_status {
    let pci_address = if let Some(r) = unsafe { address.as_ref() } {
	r
    } else {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    };

    let pci_access = pcie::PCI_ACCESS.get().unwrap().lock();
    let val = unsafe { pci_access.read(*pci_address, offset as u16 & 0xFC) };
    
    unsafe {
	*out = val;
    }

    uacpi_status::UACPI_STATUS_OK
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_write8(_handle: *const c_void, _offset: usize, _out: u8) -> uacpi_status {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_write16(_handle: *const c_void, _offset: usize, _out: u16) -> uacpi_status {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_write32(_handle: *const c_void, _offset: usize, _out: u32) -> uacpi_status {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_map(base: u64, len: usize, handle: *mut *mut PortRange) -> uacpi_status {
    if base + len as u64 > 0xFFFF {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    }

    let port_range = Box::new(PortRange {
	base: base as u16,
	length: len as u16,
    });

    unsafe {
	*handle = Box::into_raw(port_range);
    }

    uacpi_status::UACPI_STATUS_OK
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_unmap(handle: *mut PortRange) {
    let _: Box<PortRange> = unsafe { Box::from_raw(handle) };
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_read8(port_range: *const PortRange, offset: usize, out: *mut u8) -> uacpi_status {
    let port_range_ref = if let Some(r) = unsafe { port_range.as_ref() } {
	r
    } else {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    };

    if port_range_ref.length <= offset as u16 {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    }
    let mut port = Port::<u8>::new(port_range_ref.base + offset as u16);
    unsafe { *out = port.read(); }
    uacpi_status::UACPI_STATUS_OK
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_read16(port_range: *const PortRange, offset: usize, out: *mut u16) -> uacpi_status {
    let port_range_ref = if let Some(r) = unsafe { port_range.as_ref() } {
	r
    } else {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    };

    if port_range_ref.length <= offset as u16 {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    }
    let mut port = Port::<u16>::new(port_range_ref.base + offset as u16);
    unsafe { *out = port.read(); }
    uacpi_status::UACPI_STATUS_OK
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_read32(port_range: *const PortRange, offset: usize, out: *mut u32) -> uacpi_status {
    let port_range_ref = if let Some(r) = unsafe { port_range.as_ref() } {
	r
    } else {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    };

    if port_range_ref.length <= offset as u16 {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    }
    let mut port = Port::<u32>::new(port_range_ref.base + offset as u16);
    unsafe { *out = port.read(); }
    uacpi_status::UACPI_STATUS_OK
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_write8(port_range: *const PortRange, offset: usize, out: u8) -> uacpi_status {
    let port_range_ref = if let Some(r) = unsafe { port_range.as_ref() } {
	r
    } else {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    };

    if port_range_ref.length <= offset as u16 {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    }
    let mut port = Port::<u8>::new(port_range_ref.base + offset as u16);
    unsafe { port.write(out); }
    uacpi_status::UACPI_STATUS_OK
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_write16(port_range: *const PortRange, offset: usize, out: u16) -> uacpi_status {
    let port_range_ref = if let Some(r) = unsafe { port_range.as_ref() } {
	r
    } else {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    };

    if port_range_ref.length <= offset as u16 {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    }
    let mut port = Port::<u16>::new(port_range_ref.base + offset as u16);
    unsafe { port.write(out); }
    uacpi_status::UACPI_STATUS_OK
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_write32(port_range: *const PortRange, offset: usize, out: u32) -> uacpi_status {
    let port_range_ref = if let Some(r) = unsafe { port_range.as_ref() } {
	r
    } else {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    };

    if port_range_ref.length <= offset as u16 {
	return uacpi_status::UACPI_STATUS_INVALID_ARGUMENT;
    }
    let mut port = Port::<u32>::new(port_range_ref.base + offset as u16);
    unsafe { port.write(out); }
    uacpi_status::UACPI_STATUS_OK
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_map(phys_addr: u64, _len: usize) -> *mut c_void {
    memory::get_ptr_in_hhdm(PhysAddr::new(phys_addr)).as_mut_ptr()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_unmap(_addr: *const c_void) { /* No-op */ }

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_alloc(size: usize) -> *mut c_void {
    let ptr = unsafe {
	allocator::ALLOCATOR.alloc(
	    Layout::from_size_align(size, 8).unwrap()) as *mut c_void
    };

    {
	let mut allocs = ACPI_ALLOCS.get().unwrap().lock();
	allocs.insert(ptr as usize, size);
    }

    ptr
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_free(mem: *mut u8) {
    if mem as usize == 0 {
	return;
    }

    let size = {
	let mut allocs = ACPI_ALLOCS.get().unwrap().lock();
	allocs.remove(&(mem as usize)).unwrap()
    };

    unsafe {
	allocator::ALLOCATOR.dealloc(
	    mem, Layout::from_size_align(size, 8).unwrap());
    }
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_get_nanoseconds_since_boot() -> u64 {
    0
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_stall(_usec: u8) {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_sleep(_msec: u64) {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_create_mutex() -> *mut Mutex {
    let mutex = Box::new(Mutex::new());
    Box::into_raw(mutex)
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_free_mutex(handle: *mut Mutex) {
    let _: Box<Mutex> = unsafe { Box::from_raw(handle) };
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_acquire_mutex(mutex: *mut Mutex, _msec: u16) -> uacpi_status {
    unsafe { mutex.as_mut().unwrap().lock(); }
    uacpi_status::UACPI_STATUS_OK
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_release_mutex(mutex: *mut Mutex) {
    unsafe { mutex.as_mut().unwrap().unlock(); }
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_create_event() -> *mut Semaphore {
    let semaphore = Box::new(Semaphore::new());
    Box::into_raw(semaphore)
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_free_event(handle: *mut Semaphore) {
    let _: Box<Semaphore> = unsafe { Box::from_raw(handle) };
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_wait_for_event(semaphore: *mut Semaphore, msec: u16) -> bool {
    let timeout = if msec == 0xFFFF { None } else { Some(msec) };
    unsafe { semaphore.as_mut().unwrap().wait_for_event(timeout) }
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_signal_event(semaphore: *mut Semaphore) {
    unsafe { semaphore.as_mut().unwrap().signal(); }
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_reset_event(semaphore: *mut Semaphore) {
    unsafe { semaphore.as_mut().unwrap().reset(); }
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_get_thread_id() -> u64 {
    0
}

#[no_mangle]
#[allow(dead_code)]
// TODO: struct UacpiFirmwareRequest
extern "C" fn uacpi_kernel_handle_firmware_request(_request: *const c_void) -> uacpi_status {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_install_interrupt_handler(irq: u32, handler: unsafe extern "C" fn(u64), ctx: u64, _out_handle: *const c_void) -> uacpi_status {
    let route = interrupts::InterruptRoute::Irq(irq as u8);
    route.register_handler(Box::new(move || unsafe { handler(ctx) }));
    uacpi_status::UACPI_STATUS_OK
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_uninstall_interrupt_handler(_handler: *const c_void, _out_handle: *const c_void) -> uacpi_status {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_create_spinlock() -> *mut Mutex {
    let mutex = Box::new(Mutex::new());
    Box::into_raw(mutex)
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_free_spinlock(handle: *mut Mutex) {
    let _: Box<Mutex> = unsafe { Box::from_raw(handle) };
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_lock_spinlock(mutex: *mut Mutex) -> u64 {
    unsafe { mutex.as_mut().unwrap().lock(); }
    rflags::read_raw()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_unlock_spinlock(mutex: *mut Mutex, rflags: u64) {
    unsafe { mutex.as_mut().unwrap().unlock(); }
    unsafe { rflags::write_raw(rflags) }
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_schedule_work() {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_wait_for_work_completion() {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_log(level: uacpi_log_level, log: *const c_char) {
    let cstr = unsafe { CStr::from_ptr(log) };
    let rust_string_untrimmed = String::from_utf8_lossy(cstr.to_bytes()).to_string();
    let rust_string = rust_string_untrimmed.trim();

    match level {
	uacpi_log_level::UACPI_LOG_ERROR => log::error!("{}", rust_string),
	uacpi_log_level::UACPI_LOG_WARN => log::warn!("{}", rust_string),
	uacpi_log_level::UACPI_LOG_INFO => log::info!("{}", rust_string),
	uacpi_log_level::UACPI_LOG_TRACE => log::trace!("{}", rust_string),
	uacpi_log_level::UACPI_LOG_DEBUG => log::debug!("{}", rust_string),
    }
}

#[no_mangle]
#[allow(dead_code)]
pub fn uacpi_kernel_get_rsdp(rdsp_address: *mut uacpi_phys_addr) -> uacpi_status {
    if let Some(addr) = RDSP_PHYS_PTR.get() {
	unsafe { *rdsp_address = *addr; }
	uacpi_status::UACPI_STATUS_OK
    } else {
	uacpi_status::UACPI_STATUS_NOT_FOUND
    }
}

pub fn init(rdsp_addr: u64) {
    RDSP_PHYS_PTR.call_once(|| rdsp_addr);
    ACPI_ALLOCS.call_once(|| spin::Mutex::new(BTreeMap::new()));

    let status = unsafe { uacpi_initialize(0) };
    if status != uacpi_status::UACPI_STATUS_OK {
	panic!("Not okay");
    }

    let status = unsafe { uacpi_namespace_load() };
    if status != uacpi_status::UACPI_STATUS_OK {
	panic!("Not okay");
    }

    let status = unsafe { uacpi_namespace_initialize() };
    if status != uacpi_status::UACPI_STATUS_OK {
	panic!("Not okay");
    }

    let status = unsafe { uacpi_finalize_gpe_initialization() };
    if status != uacpi_status::UACPI_STATUS_OK {
	panic!("Not okay");
    }
}
