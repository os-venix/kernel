pub mod hpet;
mod pcie;
mod ide;
mod console;
mod usb;

pub fn init() {
    hpet::init();
    pcie::init();
    ide::init();
    console::init();
    usb::init();
}
