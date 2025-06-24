use num_enum::TryFromPrimitive;

#[repr(u64)]
#[derive(Debug, TryFromPrimitive)]
pub enum IoCtl {
    TIOCGPGRP = 0x540F,
    TIOCSPGRP = 0x5410,
    TIOCGWINSZ = 0x5413,
}
