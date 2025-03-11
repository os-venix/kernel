use alloc::string::{String, ToString};
use core::ffi::{c_char, c_void, CStr};

#[repr(C)]
#[derive(PartialEq, Eq)]
enum UacpiStatus {    
    UACPI_STATUS_OK = 0,
    UACPI_STATUS_MAPPING_FAILED = 1,
    UACPI_STATUS_OUT_OF_MEMORY = 2,
    UACPI_STATUS_BAD_CHECKSUM = 3,
    UACPI_STATUS_INVALID_SIGNATURE = 4,
    UACPI_STATUS_INVALID_TABLE_LENGTH = 5,
    UACPI_STATUS_NOT_FOUND = 6,
    UACPI_STATUS_INVALID_ARGUMENT = 7,
    UACPI_STATUS_UNIMPLEMENTED = 8,
    UACPI_STATUS_ALREADY_EXISTS = 9,
    UACPI_STATUS_INTERNAL_ERROR = 10,
    UACPI_STATUS_TYPE_MISMATCH = 11,
    UACPI_STATUS_INIT_LEVEL_MISMATCH = 12,
    UACPI_STATUS_NAMESPACE_NODE_DANGLING = 13,
    UACPI_STATUS_NO_HANDLER = 14,
    UACPI_STATUS_NO_RESOURCE_END_TAG = 15,
    UACPI_STATUS_COMPILED_OUT = 16,
    UACPI_STATUS_HARDWARE_TIMEOUT = 17,
    UACPI_STATUS_TIMEOUT = 18,
    UACPI_STATUS_OVERRIDDEN = 19,
    UACPI_STATUS_DENIED = 20,

    // All errors that have bytecode-related origin should go here
    UACPI_STATUS_AML_UNDEFINED_REFERENCE = 0x0EFF0000,
    UACPI_STATUS_AML_INVALID_NAMESTRING = 0x0EFF0001,
    UACPI_STATUS_AML_OBJECT_ALREADY_EXISTS = 0x0EFF0002,
    UACPI_STATUS_AML_INVALID_OPCODE = 0x0EFF0003,
    UACPI_STATUS_AML_INCOMPATIBLE_OBJECT_TYPE = 0x0EFF0004,
    UACPI_STATUS_AML_BAD_ENCODING = 0x0EFF0005,
    UACPI_STATUS_AML_OUT_OF_BOUNDS_INDEX = 0x0EFF0006,
    UACPI_STATUS_AML_SYNC_LEVEL_TOO_HIGH = 0x0EFF0007,
    UACPI_STATUS_AML_INVALID_RESOURCE = 0x0EFF0008,
    UACPI_STATUS_AML_LOOP_TIMEOUT = 0x0EFF0009,
    UACPI_STATUS_AML_CALL_STACK_DEPTH_LIMIT = 0x0EFF000A,
}

#[repr(C)]
enum UacpiInitLevel {
    UACPI_INIT_LEVEL_EARLY,
    UACPI_INIT_LEVEL_SUBSYSTEM_INITIALIZED,
    UACPI_INIT_LEVEL_NAMESPACE_LOADED,
    UACPI_INIT_LEVEL_NAMESPACE_INITIALIZED,
}

#[repr(C)]
enum UacpiLogLevel {
    UACPI_LOG_ERROR = 1,
    UACPI_LOG_WARN,
    UACPI_LOG_INFO,
    UACPI_LOG_TRACE,
    UACPI_LOG_DEBUG,
}

#[repr(C)]
struct UacpiPciAddress {
    segment: u16,
    bus: u8,
    device: u8,
    funciton: u8,
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_initialize(uacpi_init_level: UacpiInitLevel) -> UacpiStatus {
    UacpiStatus::UACPI_STATUS_OK
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_deinitialize() { }

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_device_open(address: UacpiPciAddress, handle: *mut c_void) -> UacpiStatus {
    unimplemented!();
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_device_close(handle: *const c_void) -> UacpiStatus {
    unimplemented!();
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_read8(handle: *const c_void, offset: usize, out: *mut u8) -> UacpiStatus {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_read16(handle: *const c_void, offset: usize, out: *mut u16) -> UacpiStatus {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_pci_read32(handle: *const c_void, offset: usize, out: *mut u32) -> UacpiStatus {
    unimplemented!()
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
extern "C" fn uacpi_kernel_io_map(base: u64, len: usize, handle: *mut c_void) -> UacpiStatus {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_unmap(handle: *const c_void) {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_read8(handle: *const c_void, offset: usize, out: *mut u8) -> UacpiStatus {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_read16(handle: *const c_void, offset: usize, out: *mut u16) -> UacpiStatus {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_read32(handle: *const c_void, offset: usize, out: *mut u32) -> UacpiStatus {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_write8(handle: *const c_void, offset: usize, out: u8) -> UacpiStatus {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_write16(handle: *const c_void, offset: usize, out: u16) -> UacpiStatus {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_io_write32(handle: *const c_void, offset: usize, out: u32) -> UacpiStatus {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_map(phys_addr: u64, len: usize) -> *const c_void {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_unmap(addr: *const c_void, len: usize) {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_alloc(size: usize) -> *const c_void {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_free(mem: *const c_void) {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_get_nanoseconds_since_boot() -> u64 {
    unimplemented!()
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
extern "C" fn uacpi_kernel_create_mutex() -> *const c_void {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_free_mutex(handle: *const c_void) {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_create_event() -> *const c_void {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_free_event(handle: *const c_void) {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_get_thread_id() -> *const c_void {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_acquire_mutex(handle: *const c_void, msec: u16) -> UacpiStatus {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_release_mutex(handle: *const c_void) {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_wait_for_event(handle: *const c_void, msec: u16) -> bool {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_signal_event(handle: *const c_void) {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_reset_event(handle: *const c_void) {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
// TODO: struct UacpiFirmwareRequest
extern "C" fn uacpi_kernel_handle_firmware_request(request: *const c_void) -> UacpiStatus {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_install_interrupt_handler(irq: u32, handler: *const c_void, ctx: *const c_void, out_handle: *const c_void) -> UacpiStatus {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_uninstall_interrupt_handler(handler: *const c_void, out_handle: *const c_void) -> UacpiStatus {
    unimplemented!()
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_create_spinlock() -> *const c_void {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_free_spinlock(handle: *const c_void) {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_lock_spinlock(handle: *const c_void) -> u32 {
    unimplemented!()
}
#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_unlock_spinlock(handle: *const c_void) {
    unimplemented!()
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
    let rust_string = String::from_utf8_lossy(cstr.to_bytes()).to_string();

    match level {
	UacpiLogLevel::UACPI_LOG_ERROR => log::error!("{}", rust_string),
	UacpiLogLevel::UACPI_LOG_WARN => log::warn!("{}", rust_string),
	UacpiLogLevel::UACPI_LOG_INFO => log::info!("{}", rust_string),
	UacpiLogLevel::UACPI_LOG_TRACE => log::trace!("{}", rust_string),
	UacpiLogLevel::UACPI_LOG_DEBUG => log::debug!("{}", rust_string),
    }
}

#[no_mangle]
#[allow(dead_code)]
extern "C" fn uacpi_kernel_get_rsdp() {
    unimplemented!()
}


extern "C" {
    fn uacpi_initialize(flags: u64) -> UacpiStatus;
}

pub fn init() {
    let status = unsafe { uacpi_initialize(0) };
    if status != UacpiStatus::UACPI_STATUS_OK {
	panic!("Not okay");
    }
}
