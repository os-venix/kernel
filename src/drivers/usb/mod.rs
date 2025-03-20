mod hid;
mod uhci;
pub mod usb;

pub fn init() {
    uhci::init();
    usb::init();
    hid::init();
}
