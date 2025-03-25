mod hid;
mod uhci;
pub mod usb;
pub mod protocol;

pub fn init() {
    uhci::init();
    usb::init();
    hid::init();
}
