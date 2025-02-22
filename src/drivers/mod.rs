pub mod hpet;
mod pcie;
mod ide;
mod console;

pub fn init() {
    hpet::init();
    pcie::init();
    ide::init();
    console::init();
}
