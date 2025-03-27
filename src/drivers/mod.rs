pub mod hpet;
pub mod pcie;
mod ide;
mod console;
mod usb;
mod usbhid;

pub fn init() {
    hpet::init();
    pcie::init();
    ide::init();
    console::init();
    usb::init();
    usbhid::init();
}
