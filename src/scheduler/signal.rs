use core::ffi::c_int;

#[repr(C)]
struct SigAction {
    sa_handler: usize,
    sa_mask: u64,
    sa_flags: c_int,
}

#[derive(Clone)]
pub enum HandlerType {
    Handler,
    SigAction,
}

#[derive(Clone)]
pub struct SignalHandler {
    handler: usize,  // The address to start
    mask: u64,  // The list of signals to mask off when in the handler
    handler_type: HandlerType,
    flags: u64,  // This will get fleshed out in due course}
}

pub fn parse_sigaction(sigaction: u64) -> SignalHandler {
    let sa = sigaction as *const SigAction;

    unsafe {
	SignalHandler {
	    handler: (*sa).sa_handler,
	    mask: (*sa).sa_mask,
	    handler_type: if ((*sa).sa_mask & (1 << 4)) != 0 { HandlerType::SigAction } else { HandlerType::Handler },
	    flags: (*sa).sa_flags as u64,
	}
    }
}
