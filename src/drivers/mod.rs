pub mod hpet;
pub mod pcie;
mod ide;
mod usb;
mod usbhid;

pub fn init() {
    hpet::init();
    pcie::init();
    ide::init();
    usb::init();
    usbhid::init();
}
