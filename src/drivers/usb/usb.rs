struct Usb {

}

unsafe impl Send for Usb { }
unsafe impl Sync for Usb { }

impl driver::Bus for Usb {
    fn name(&self) -> String {
	String::from("USB")
    }

    fn enumerate(&mut self) -> Vec<Box<dyn driver::DeviceTypeIdentifier>> {
	Vec::new()
    }
}
