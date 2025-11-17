use core::ffi::c_int;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct SigAction {
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

pub fn parse_sigaction(sigaction: SigAction) -> SignalHandler {
    unsafe {
	SignalHandler {
	    handler: sigaction.sa_handler,
	    mask: sigaction.sa_mask,
	    handler_type: if (sigaction.sa_mask & (1 << 4)) != 0 { HandlerType::SigAction } else { HandlerType::Handler },
	    flags: sigaction.sa_flags as u64,
	}
    }
}

pub fn create_sigaction(sighandler: SignalHandler) -> SigAction {
    SigAction {
	sa_handler: sighandler.handler,
	sa_mask: sighandler.mask,
	sa_flags: sighandler.flags as c_int,
    }
}
