use alloc::vec::Vec;
use nom::{
    IResult,
    Parser,
    multi::{
	count,
	many_m_n,
    },
    number::{
	complete::{u8, u16},
	Endianness,
    },
};

#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct HidDescriptorDescriptor {
    pub descriptor_type: u8,
    pub length: u16,
}

#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct HidDescriptor {
    pub version: u16,
    pub country_code: u8,
    pub descriptors: Vec<HidDescriptorDescriptor>,
}

#[allow(dead_code)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Key {
    AsciiKey(char),
    #[default]
    Unknown,
}

#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct BootKeyPresses {
    pub lctl: bool,
    pub lshift: bool,
    pub lalt: bool,
    pub lsuper: bool,
    pub rctl: bool,
    pub rshift: bool,
    pub ralt: bool,
    pub rgui: bool,

    pub keys: Vec<Key>,
}

fn parse_hid_descriptor_descriptor(input: &[u8]) -> IResult<&[u8], HidDescriptorDescriptor> {
    let (input, descriptor_type) = u8(input)?;
    let (input, length) = u16(Endianness::Little)(input)?;

    Ok((input, HidDescriptorDescriptor {
	descriptor_type,
	length,
    }))
}

pub fn parse_hid_descriptor(input: &[u8]) -> IResult<&[u8], HidDescriptor> {
    let (input, version) = u16(Endianness::Little)(input)?;
    let (input, country_code) = u8(input)?;
    let (input, num_descriptors) = u8(input)?;
    let (input, descriptors) = many_m_n(num_descriptors as usize, num_descriptors as usize, parse_hid_descriptor_descriptor).parse(input)?;

    Ok((input, HidDescriptor {
	version,
	country_code,
	descriptors,
    }))
}

fn parse_key(input: &[u8]) -> IResult<&[u8], Key> {
    let (input, keypress) = u8(input)?;

    let key = match keypress {
        0x04 => Key::AsciiKey('a'),
        0x05 => Key::AsciiKey('b'),
        0x06 => Key::AsciiKey('c'),
        0x07 => Key::AsciiKey('d'),
        0x08 => Key::AsciiKey('e'),
        0x09 => Key::AsciiKey('f'),
        0x0A => Key::AsciiKey('g'),
        0x0B => Key::AsciiKey('h'),
        0x0C => Key::AsciiKey('i'),
        0x0D => Key::AsciiKey('j'),
        0x0E => Key::AsciiKey('k'),
        0x0F => Key::AsciiKey('l'),
        0x10 => Key::AsciiKey('m'),
        0x11 => Key::AsciiKey('n'),
        0x12 => Key::AsciiKey('o'),
        0x13 => Key::AsciiKey('p'),
        0x14 => Key::AsciiKey('q'),
        0x15 => Key::AsciiKey('r'),
        0x16 => Key::AsciiKey('s'),
        0x17 => Key::AsciiKey('t'),
        0x18 => Key::AsciiKey('u'),
        0x19 => Key::AsciiKey('v'),
        0x1A => Key::AsciiKey('w'),
        0x1B => Key::AsciiKey('x'),
        0x1C => Key::AsciiKey('y'),
        0x1D => Key::AsciiKey('z'),

        0x1E => Key::AsciiKey('1'),
        0x1F => Key::AsciiKey('2'),
        0x20 => Key::AsciiKey('3'),
        0x21 => Key::AsciiKey('4'),
        0x22 => Key::AsciiKey('5'),
        0x23 => Key::AsciiKey('6'),
        0x24 => Key::AsciiKey('7'),
        0x25 => Key::AsciiKey('8'),
        0x26 => Key::AsciiKey('9'),
        0x27 => Key::AsciiKey('0'),

        0x28 => Key::AsciiKey('\n'),   // Enter
        0x2C => Key::AsciiKey(' '),    // Space
        0x2D => Key::AsciiKey('-'),
        0x2E => Key::AsciiKey('='),
        0x2F => Key::AsciiKey('['),
        0x30 => Key::AsciiKey(']'),
        0x31 => Key::AsciiKey('\\'),
        0x33 => Key::AsciiKey(';'),
        0x34 => Key::AsciiKey('\''),
        0x35 => Key::AsciiKey('`'),
        0x36 => Key::AsciiKey(','),
        0x37 => Key::AsciiKey('.'),
        0x38 => Key::AsciiKey('/'),

        _ => Key::Unknown,
    };

    Ok((input, key))
}

pub fn parse_boot_buffer(input: &[u8]) -> IResult<&[u8], BootKeyPresses> {
    // TODO - handle modifiers
    let (input, _) = u8(input)?;
    let (input, _) = u8(input)?;

    let (input, keypresses) = count(parse_key, 6).parse(input)?;

    Ok((input, BootKeyPresses {
	lctl: false,
	lshift: false,
	lalt: false,
	lsuper: false,
	rctl: false,
	rshift: false,
	ralt: false,
	rgui: false,

	keys: keypresses,
    }))
}
