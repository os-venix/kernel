use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use nom::{
    IResult,
    Parser,
    bits::{
	bits,
	streaming::{
	    bool,
	    take,
	},
    },
    branch::alt,
    bytes::complete::tag,
    combinator::not,
    multi::many1,
    number::{
	complete::{u8, u16},
	Endianness,
    },
    sequence::preceded,
};

#[allow(dead_code)]
#[derive(Clone)]
pub struct ConfigurationDescriptor {
    pub total_length: u16,
    pub num_interfaces: u8,
    pub configuration_value: u8,
    pub configuration_string: u8,
    pub self_powered: bool,
    pub remote_wakeup: bool,
    pub max_power: u8,
}

#[derive(Clone)]
pub enum EndpointDirection {
    Out,
    In,
}

#[derive(Clone)]
pub enum EndpointUsageType {
    Data,
    Feedback,
    ImplicitFeedbackData,
}

#[derive(Clone)]
pub enum EndpointSynchType {
    None,
    Async,
    Adaptive,
    Synchronous,
}

#[derive(Clone)]
pub enum EndpointTransferType {
    Control,
    Isochronous,
    Bulk,
    Interrupt,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct EndpointDescriptor {
    pub direction: EndpointDirection,
    pub endpoint_number: u8,
    pub usage_type: EndpointUsageType,
    pub synch_type: EndpointSynchType,
    pub transfer_type: EndpointTransferType,
    pub max_packet_size: u16,
    pub interval: u8,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct GenericDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub remaining_bytes: Box<[u8]>,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct InterfaceDescriptor {
    pub interface_number: u8,
    pub alternate_setting: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub interface_string: u8,
    pub endpoints: BTreeMap<u8, EndpointDescriptor>,
    pub other_descriptors: Vec<GenericDescriptor>,
}

enum Descriptor {
    EndpointDescriptor(EndpointDescriptor),
    GenericDescriptor(GenericDescriptor),
}

fn parse_configuration_descriptor_inner(input: &[u8]) -> IResult<&[u8], ConfigurationDescriptor> {
    let (input, total_length) = u16(Endianness::Little)(input)?;
    let (input, num_interfaces) = u8(input)?;
    let (input, configuration_value) = u8(input)?;
    let (input, configuration_string) = u8(input)?;
    let (input, (_, self_powered, remote_wakeup)) = bits((bool::<_, nom::error::Error<_>>, bool, bool))(input)?;
    let (input, max_power) = u8(input)?;

    Ok((input, ConfigurationDescriptor {
	total_length,
	num_interfaces,
	configuration_value,
	configuration_string,
	self_powered,
	remote_wakeup,
	max_power
    }))
}

pub fn parse_configuration_descriptor(input: &[u8]) -> IResult<&[u8], ConfigurationDescriptor> {
    preceded(
	tag([9, 2].as_slice()),  // Descriptor has 9 bytes, and is descriptor type 2 - CONFIGURATION
	parse_configuration_descriptor_inner).parse(input)
}

fn parse_interface_descriptor_inner(input: &[u8]) -> IResult<&[u8], (InterfaceDescriptor, u8)> {
    let (input, interface_number) = u8(input)?;
    let (input, alternate_setting) = u8(input)?;
    let (input, num_endpoints) = u8(input)?;
    let (input, class) = u8(input)?;
    let (input, subclass) = u8(input)?;
    let (input, protocol) = u8(input)?;
    let (input, interface_string) = u8(input)?;

    Ok((input, (InterfaceDescriptor {
	interface_number,
	alternate_setting,
	class,
	subclass,
	protocol,
	interface_string,
	endpoints: BTreeMap::new(),
	other_descriptors: Vec::new(),
    }, num_endpoints)))
}

fn parse_endpoint_descriptor_inner(input: &[u8]) -> IResult<&[u8], Descriptor> {
    let (input, (direction, _, endpoint_number)): (&[u8], (bool, u8, u8)) = bits(
	(bool::<_, nom::error::Error<_>>, take(3usize), take(4usize)))(input)?;
    let (input, (_, usage_type, synch_type, transfer_type)): (&[u8], (u8, u8, u8, u8)) = bits(
	(take::<_, _, _, nom::error::Error<_>>(2usize), take(2usize), take(2usize), take(2usize)))(input)?;
    let (input, max_packet_size) = u16(Endianness::Little)(input)?;
    let (input, interval) = u8(input)?;

    let direction = if direction { EndpointDirection::In } else { EndpointDirection::Out };
    let usage_type = match usage_type {
	0 => EndpointUsageType::Data,
	1 => EndpointUsageType::Feedback,
	2 => EndpointUsageType::ImplicitFeedbackData,
	_ => panic!(),
    };
    let synch_type = match synch_type {
	0 => EndpointSynchType::None,
	1 => EndpointSynchType::Async,
	2 => EndpointSynchType::Adaptive,
	3 => EndpointSynchType::Synchronous,
	_ => panic!(),
    };
    let transfer_type = match transfer_type {
	0 => EndpointTransferType::Control,
	1 => EndpointTransferType::Isochronous,
	2 => EndpointTransferType::Bulk,
	3 => EndpointTransferType::Interrupt,
	_ => panic!(),
    };

    Ok((input, Descriptor::EndpointDescriptor(EndpointDescriptor {
	direction,
	endpoint_number,
	usage_type,
	synch_type,
	transfer_type,
	max_packet_size,
	interval,
    })))
}

fn parse_endpoint_descriptor(input: &[u8]) -> IResult<&[u8], Descriptor> {    
    preceded(
	tag([7, 5].as_slice()),
	parse_endpoint_descriptor_inner).parse(input)
}

fn parse_other_descriptor(input: &[u8]) -> IResult<&[u8], Descriptor> {
    let (input, length) = u8(input)?;
    let (input, descriptor_type) = u8(input)?;
    let (input, remaining_bytes) = nom::bytes::complete::take(length as usize - 2)(input)?;

    Ok((input, Descriptor::GenericDescriptor(GenericDescriptor {
	length,
	descriptor_type,
	remaining_bytes: Box::from(remaining_bytes),
    })))
}

fn parse_rest_of_interface_inner(input: &[u8]) -> IResult<&[u8], Descriptor> {
    // Make sure it isn't an interface descriptor, we should stop when we get to those.
    // Reason being, each ID is then followed by any pertinent descriptors that relate to it -
    // endpoint descriptors, HID descriptor for HID devices, and so forth.
    // So when we reach the next one, the current one's done
    not(tag([9, 4].as_slice())).parse(input)?;

    alt((parse_endpoint_descriptor, parse_other_descriptor)).parse(input)
}

fn parse_rest_of_interface(input: &[u8]) -> IResult<&[u8], (BTreeMap<u8, EndpointDescriptor>, Vec<GenericDescriptor>)> {
    let (input, descriptors) = many1(parse_rest_of_interface_inner).parse(input)?;
    
    let mut endpoint_map: BTreeMap<u8, EndpointDescriptor> = BTreeMap::new();
    let mut generic_descriptors: Vec<GenericDescriptor> = Vec::new();

    for descriptor in descriptors {
	match descriptor {
	    Descriptor::EndpointDescriptor(e) => {
		endpoint_map.insert(e.endpoint_number, e.clone());
	    },
	    Descriptor::GenericDescriptor(g) => generic_descriptors.push(g),
	}
    }

    Ok((input, (endpoint_map, generic_descriptors)))
}

fn parse_interface_descriptor(input: &[u8]) -> IResult<&[u8], InterfaceDescriptor> {
    let (input, (mut interface_descriptor, num_endpoints)) = preceded(
	tag([9, 4].as_slice()),
	parse_interface_descriptor_inner).parse(input)?;
    let (input, (endpoints, other_descriptors)) = parse_rest_of_interface(input)?;

    interface_descriptor.endpoints = endpoints;
    interface_descriptor.other_descriptors = other_descriptors;

    Ok((input, interface_descriptor))
}

pub fn parse_configuration_descriptors(input: &[u8]) -> IResult<&[u8], (ConfigurationDescriptor, Vec<InterfaceDescriptor>)> {
    let (input, configuration_descriptor) = parse_configuration_descriptor(input)?;
    let (input, interface_descriptors) = many1(parse_interface_descriptor).parse(input)?;

    Ok((input, (configuration_descriptor, interface_descriptors)))
}
