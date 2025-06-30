use num_enum::TryFromPrimitive;

#[repr(u64)]
#[derive(Debug, TryFromPrimitive)]
pub enum IoCtl {
    TCGETS = 0x5401,
    TCSETS = 0x5402,
    TIOCGPGRP = 0x540F,
    TIOCSPGRP = 0x5410,
    TIOCGWINSZ = 0x5413,
}
