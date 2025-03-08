mod uhci;
mod usb;

pub fn init() {
    uhci::init();
    usb::init();
}
