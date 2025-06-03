use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};

pub mod acpi;
pub mod block;
pub mod syscall;
pub mod vfs;
pub mod ioctl;

// CPU init
pub fn init() {
    // Enable SSE
    let mut cr0_flags = Cr0::read();
    cr0_flags.remove(Cr0Flags::EMULATE_COPROCESSOR);
    cr0_flags.insert(Cr0Flags::MONITOR_COPROCESSOR);

    let mut cr4_flags = Cr4::read();
    cr4_flags.insert(Cr4Flags::OSFXSR);
    cr4_flags.insert(Cr4Flags::OSXMMEXCPT_ENABLE);

    unsafe {
	Cr0::write(cr0_flags);
	Cr4::write(cr4_flags);
    }
}
