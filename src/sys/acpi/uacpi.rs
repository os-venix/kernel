use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;
use core::ffi::{c_char, c_void, CStr};
use pci_types::{ConfigRegionAccess, PciAddress};
use spin::Once;
use x86_64::PhysAddr;
use x86_64::instructions::port::Port;
use x86_64::registers::rflags;

use crate::sys::acpi::acpi_lock::{Mutex, Semaphore};
use crate::drivers::pcie;
use crate::memory;
use crate::interrupts;

#[repr(C)]
#[derive(PartialEq, Eq, Debug)]
#[allow(dead_code)]
pub enum UacpiStatus {
    Ok = 0,
    MappingFailed = 1,
    OutOfMemory = 2,
    BadChecksum = 3,
    InvalidSignature = 4,
    InvalidTableLength = 5,
    NotFound = 6,
    InvalidArgument = 7,
    Unimplemented = 8,
    AlreadyExists = 9,
    InternalError = 10,
    TypeMismatch = 11,
    InitLevelMismatch = 12,
    NamespaceNodeDangling = 13,
    NoHandler = 14,
    NoResourceEndTag = 15,
    CompiledOut = 16,
    HardwareTimeout = 17,
    Timeout = 18,
    Overridden = 19,
    Denied = 20,

    // all ERRORS THAT HAVE BYTECODE-RELATED ORIGIN SHOULD GO HERE
    AmlUndefinedReference = 0x0_eff0000,
    AmlInvalidNamestring = 0x0_eff0001,
    AmlObjectAlreadyExists = 0x0_eff0002,
    AmlInvalidOpcode = 0x0_eff0003,
    AmlIncompatibleObjectType = 0x0_eff0004,
    AmlBadEncoding = 0x0_eff0005,
    AmlOutOfBoundsIndex = 0x0_eff0006,
    AmlSyncLevelTooHigh = 0x0_eff0007,
    AmlInvalidResource = 0x0_eff0008,
    AmlLoopTimeout = 0x0_eff0009,
    AmlCallStackDepthLimit = 0x0_eff000_a,
}

#[repr(C)]
#[allow(dead_code)]
pub enum UacpiIterationDecision {    
    Continue,
    Break,
    NextPeer,
}

#[repr(C)]
#[allow(dead_code)]
enum UacpiInitLevel {
    Early,
    SubsystemInitialized,
    NamespaceLoaded,
    NamespaceInitialized,
}

#[repr(C)]
enum UacpiLogLevel {
    Error = 1,
    Warn,
    Info,
    Trace,
    Debug,
}

#[repr(C)]
pub enum InterruptModel {
    Pic,
    IoApic,
    IosApic,
}

#[repr(C)]
struct UacpiPciAddress {
    segment: u16,
    bus: u8,
    device: u8,
    function: u8,
}

#[derive(Copy, Clone)]
struct PortRange {
    pub base: u16,
    pub length: u16,
}

#[derive(Eq, PartialEq, Clone)]
pub struct Namespace(u64);

#[repr(C)]
pub struct UacpiObject {
    
}

#[repr(C)]
pub struct UacpiObjectArray {
    objects: *mut *mut UacpiObject,
    count: usize,
}

static RDSP_PHYS_PTR: Once<u64> = Once::new();

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_initialize(_uacpi_init_level: UacpiInitLevel) -> UacpiStatus {
    UacpiStatus::Ok
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_deinitialize() { }

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_device_open(address: UacpiPciAddress, handle: *mut *mut PciAddress) -> UacpiStatus {
    let address = Box::new(PciAddress::new(address.segment, address.bus, address.device, address.function));
    unsafe {
	*handle = Box::into_raw(address);
    }

    UacpiStatus::Ok
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_device_close(handle: *mut PciAddress) {
    let _: Box<PciAddress> = unsafe { Box::from_raw(handle) };
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_read8(address: *mut PciAddress, offset: usize, out: *mut u8) -> UacpiStatus {
    let pci_address = if let Some(r) = unsafe { address.as_ref() } {
	r
    } else {
	return UacpiStatus::InvalidArgument;
    };

    let pci_access = pcie::PCI_ACCESS.get().unwrap().lock();
    let val = unsafe { pci_access.read(*pci_address, offset as u16 & 0xFC) } >> ((offset & 3) * 8);
    
    unsafe {
	*out = val as u8;
    }

    UacpiStatus::Ok
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_read16(address: *mut PciAddress, offset: usize, out: *mut u16) -> UacpiStatus {
    let pci_address = if let Some(r) = unsafe { address.as_ref() } {
	r
    } else {
	return UacpiStatus::InvalidArgument;
    };

    let pci_access = pcie::PCI_ACCESS.get().unwrap().lock();
    let val = unsafe { pci_access.read(*pci_address, offset as u16 & 0xFC) } >> ((offset & 2) * 8);
    
    unsafe {
	*out = val as u16;
    }

    UacpiStatus::Ok
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_read32(address: *mut PciAddress, offset: usize, out: *mut u32) -> UacpiStatus {
    let pci_address = if let Some(r) = unsafe { address.as_ref() } {
	r
    } else {
	return UacpiStatus::InvalidArgument;
    };

    let pci_access = pcie::PCI_ACCESS.get().unwrap().lock();
    let val = unsafe { pci_access.read(*pci_address, offset as u16 & 0xFC) };
    
    unsafe {
	*out = val;
    }

    UacpiStatus::Ok
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_write8(handle: *const c_void, offset: usize, out: u8) -> UacpiStatus {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_write16(handle: *const c_void, offset: usize, out: u16) -> UacpiStatus {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_write32(handle: *const c_void, offset: usize, out: u32) -> UacpiStatus {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_map(base: u64, len: usize, handle: *mut *mut PortRange) -> UacpiStatus {
    if base + len as u64 > 0xFFFF {
	return UacpiStatus::InvalidArgument;
    }

    let port_range = Box::new(PortRange {
	base: base as u16,
	length: len as u16,
    });

    unsafe {
	*handle = Box::into_raw(port_range);
    }

    UacpiStatus::Ok
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_unmap(handle: *mut PortRange) {
    let _: Box<PortRange> = unsafe { Box::from_raw(handle) };
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_read8(port_range: *const PortRange, offset: usize, out: *mut u8) -> UacpiStatus {
    let port_range_ref = if let Some(r) = unsafe { port_range.as_ref() } {
	r
    } else {
	return UacpiStatus::InvalidArgument;
    };

    if port_range_ref.length <= offset as u16 {
	return UacpiStatus::InvalidArgument;
    }
    let mut port = Port::<u8>::new(port_range_ref.base + offset as u16);
    unsafe { *out = port.read(); }
    UacpiStatus::Ok
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_read16(port_range: *const PortRange, offset: usize, out: *mut u16) -> UacpiStatus {
    let port_range_ref = if let Some(r) = unsafe { port_range.as_ref() } {
	r
    } else {
	return UacpiStatus::InvalidArgument;
    };

    if port_range_ref.length <= offset as u16 {
	return UacpiStatus::InvalidArgument;
    }
    let mut port = Port::<u16>::new(port_range_ref.base + offset as u16);
    unsafe { *out = port.read(); }
    UacpiStatus::Ok
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_read32(port_range: *const PortRange, offset: usize, out: *mut u32) -> UacpiStatus {
    let port_range_ref = if let Some(r) = unsafe { port_range.as_ref() } {
	r
    } else {
	return UacpiStatus::InvalidArgument;
    };

    if port_range_ref.length <= offset as u16 {
	return UacpiStatus::InvalidArgument;
    }
    let mut port = Port::<u32>::new(port_range_ref.base + offset as u16);
    unsafe { *out = port.read(); }
    UacpiStatus::Ok
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_write8(port_range: *const PortRange, offset: usize, out: u8) -> UacpiStatus {
    let port_range_ref = if let Some(r) = unsafe { port_range.as_ref() } {
	r
    } else {
	return UacpiStatus::InvalidArgument;
    };

    if port_range_ref.length <= offset as u16 {
	return UacpiStatus::InvalidArgument;
    }
    let mut port = Port::<u8>::new(port_range_ref.base + offset as u16);
    unsafe { port.write(out); }
    UacpiStatus::Ok
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_write16(port_range: *const PortRange, offset: usize, out: u16) -> UacpiStatus {
    let port_range_ref = if let Some(r) = unsafe { port_range.as_ref() } {
	r
    } else {
	return UacpiStatus::InvalidArgument;
    };

    if port_range_ref.length <= offset as u16 {
	return UacpiStatus::InvalidArgument;
    }
    let mut port = Port::<u16>::new(port_range_ref.base + offset as u16);
    unsafe { port.write(out); }
    UacpiStatus::Ok
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_write32(port_range: *const PortRange, offset: usize, out: u32) -> UacpiStatus {
    let port_range_ref = if let Some(r) = unsafe { port_range.as_ref() } {
	r
    } else {
	return UacpiStatus::InvalidArgument;
    };

    if port_range_ref.length <= offset as u16 {
	return UacpiStatus::InvalidArgument;
    }
    let mut port = Port::<u32>::new(port_range_ref.base + offset as u16);
    unsafe { port.write(out); }
    UacpiStatus::Ok
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
    Box::into_raw(vec![0; size].into_boxed_slice()) as *mut c_void
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_free(mem: *mut u8, size: usize) {
    // unsafe {
    // 	let s = slice::from_raw_parts_mut(mem, size);
    // 	Box::from_raw(ptr::from_mut(s));
    // }
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_get_nanoseconds_since_boot() -> u64 {
    0
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_stall(usec: u8) {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_sleep(msec: u64) {
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
extern "C" fn uacpi_kernel_acquire_mutex(mutex: *mut Mutex, msec: u16) -> UacpiStatus {
    unsafe { mutex.as_mut().unwrap().lock(); }
    UacpiStatus::Ok
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
extern "C" fn uacpi_kernel_handle_firmware_request(request: *const c_void) -> UacpiStatus {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_install_interrupt_handler(irq: u32, handler: unsafe extern "C" fn(u64), ctx: u64, out_handle: *const c_void) -> UacpiStatus {
    interrupts::add_irq_handler(irq as u8, Box::new(move || unsafe { handler(ctx) }));
    UacpiStatus::Ok
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_uninstall_interrupt_handler(handler: *const c_void, out_handle: *const c_void) -> UacpiStatus {
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
extern "C" fn uacpi_kernel_log(level: UacpiLogLevel, log: *const c_char) {
    let cstr = unsafe { CStr::from_ptr(log) };
    let rust_string_untrimmed = String::from_utf8_lossy(cstr.to_bytes()).to_string();
    let rust_string = rust_string_untrimmed.trim();

    match level {
	UacpiLogLevel::Error => log::error!("{}", rust_string),
	UacpiLogLevel::Warn => log::warn!("{}", rust_string),
	UacpiLogLevel::Info => log::info!("{}", rust_string),
	UacpiLogLevel::Trace => log::trace!("{}", rust_string),
	UacpiLogLevel::Debug => log::debug!("{}", rust_string),
    }
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_get_rsdp(rdsp_address: *mut u64) -> UacpiStatus {
    if let Some(addr) = RDSP_PHYS_PTR.get() {
	unsafe { *rdsp_address = *addr; }
	UacpiStatus::Ok
    } else {
	UacpiStatus::NotFound
    }
}

extern "C" {
    fn uacpi_initialize(flags: u64) -> UacpiStatus;
    fn uacpi_namespace_load() -> UacpiStatus;
    fn uacpi_namespace_initialize() -> UacpiStatus;
    fn uacpi_finalize_gpe_initialization() -> UacpiStatus;

    fn uacpi_execute(parent_node: u64, path: *const i8, args: *mut UacpiObjectArray) -> UacpiStatus;
    pub fn uacpi_set_interrupt_model(model: InterruptModel) -> UacpiStatus;
}

pub fn init(rdsp_addr: u64) {
    RDSP_PHYS_PTR.call_once(|| rdsp_addr);

    let status = unsafe { uacpi_initialize(0) };
    if status != UacpiStatus::Ok {
	panic!("Not okay");
    }

    let status = unsafe { uacpi_namespace_load() };
    if status != UacpiStatus::Ok {
	panic!("Not okay");
    }

    let status = unsafe { uacpi_namespace_initialize() };
    if status != UacpiStatus::Ok {
	panic!("Not okay");
    }

    let status = unsafe { uacpi_finalize_gpe_initialization() };
    if status != UacpiStatus::Ok {
	panic!("Not okay");
    }
}
