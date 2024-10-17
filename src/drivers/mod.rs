mod hpet;
mod pcie;

pub fn init() {
    hpet::init();
    pcie::init();
}
