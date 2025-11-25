mod uhci;
pub mod usbdevice;
pub mod protocol;

pub fn init() {
    uhci::init();
}
