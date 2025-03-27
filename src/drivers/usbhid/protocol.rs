use alloc::vec::Vec;
use nom::{
    IResult,
    Parser,
    multi::many_m_n,
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
