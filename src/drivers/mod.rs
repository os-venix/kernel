mod hpet;
mod pcie;
mod ide;

pub fn init() {
    hpet::init();
    pcie::init();
    ide::init();
}
