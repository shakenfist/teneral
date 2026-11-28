//! The fixtures the tests in this module are built from.
//!
//! One datagram builder, with a field per BOOTP field, so that a
//! malformed input is written as "a DISCOVER that parses, with this
//! one thing wrong with it". That matters more here than it usually
//! does: a corpus whose inputs were each hand-assembled would be a
//! corpus where an input that stopped reaching the guard it was
//! written for would still pass.
//!
//! There is also a reader for an encoded answer's options, which
//! walks the bytes rather than asking a decoder. The order the options
//! are written in is a measured property of the bytes, and a decoder
//! that holds them in a map keyed by code cannot report it.

use std::net::Ipv4Addr;

use crate::wire::{BOOT_REQUEST, COOKIE_OFFSET, FIXED_BYTES, MAGIC_COOKIE, OPTION_END, OPTION_PAD};

/// Where `chaddr` ends and `sname` begins.
const CHADDR_END: usize = 44;

/// The hardware address every fixture presents.
pub const CLIENT_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0xaa, 0xbb, 0xcc];

/// The transaction every fixture opens.
pub const TRANSACTION: u32 = 0x17e8_5f20;

/// The address the interface a request arrived on holds.
pub const SERVER_ADDRESS: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);

/// The prefix configured beside it.
pub const PREFIX_LENGTH: u8 = 24;

/// An address inside that subnet, to offer.
pub const OFFERED_ADDRESS: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 128);

/// Twelve hours: the schema's default lease time, and the one the
/// measurements the option set comes from were taken against.
pub const LEASE_SECONDS: i64 = 43200;

/// Option 53's values, as a client sends them.
pub const DISCOVER: u8 = 1;
/// A REQUEST.
pub const REQUEST: u8 = 3;
/// An OFFER, which no client sends.
pub const OFFER: u8 = 2;
/// An INFORM.
pub const INFORM: u8 = 8;

/// A datagram, field by field, so that any one of them can be wrong
/// on purpose.
#[derive(Clone, Debug)]
pub struct Datagram {
    /// `op`.
    pub opcode: u8,
    /// `htype`.
    pub hardware_type: u8,
    /// `hlen`, which is deliberately independent of `chaddr` below:
    /// the guard this module exists for is about the two disagreeing.
    pub hlen: u8,
    /// `xid`.
    pub transaction: u32,
    /// The broadcast flag.
    pub broadcast: bool,
    /// `ciaddr`.
    pub client_address: Ipv4Addr,
    /// `giaddr`.
    pub relay_address: Ipv4Addr,
    /// The first octets of `chaddr`; the rest of the field is zero.
    pub chaddr: Vec<u8>,
    /// The four octets where the magic cookie belongs.
    pub cookie: [u8; 4],
    /// The options area, including its terminator, exactly as it
    /// should appear.
    pub options: Vec<u8>,
}

impl Datagram {
    /// A DISCOVER that parses: the base every malformed input is a
    /// single change away from.
    pub fn discover() -> Self {
        Datagram {
            opcode: BOOT_REQUEST,
            hardware_type: 1,
            hlen: 6,
            transaction: TRANSACTION,
            broadcast: false,
            client_address: Ipv4Addr::UNSPECIFIED,
            relay_address: Ipv4Addr::UNSPECIFIED,
            chaddr: CLIENT_MAC.to_vec(),
            cookie: MAGIC_COOKIE,
            options: options(&[
                option(53, &[DISCOVER]),
                option(61, &[0x01, 0x52, 0x54, 0x00, 0xaa, 0xbb, 0xcc]),
                option(55, &[1, 3, 6, 28]),
            ]),
        }
    }

    /// The datagram as bytes.
    pub fn bytes(&self) -> Vec<u8> {
        let mut frame = Vec::new();
        // `op`, `htype`, `hlen` and `hops`.
        frame.extend_from_slice(&[self.opcode, self.hardware_type, self.hlen, 0]);
        frame.extend_from_slice(&self.transaction.to_be_bytes());
        frame.extend_from_slice(&0_u16.to_be_bytes());
        frame.extend_from_slice(&if self.broadcast { 0x8000_u16 } else { 0 }.to_be_bytes());
        frame.extend_from_slice(&self.client_address.octets());
        frame.extend_from_slice(&Ipv4Addr::UNSPECIFIED.octets());
        frame.extend_from_slice(&Ipv4Addr::UNSPECIFIED.octets());
        frame.extend_from_slice(&self.relay_address.octets());
        frame.extend_from_slice(&self.chaddr);
        // `chaddr` padded to its field, then `sname` and `file`.
        frame.resize(CHADDR_END, 0);
        frame.resize(COOKIE_OFFSET, 0);
        frame.extend_from_slice(&self.cookie);
        frame.extend_from_slice(&self.options);
        frame
    }

    /// The datagram as bytes, truncated to `length`.
    pub fn truncated_to(&self, length: usize) -> Vec<u8> {
        let mut frame = self.bytes();
        frame.truncate(length);
        frame
    }
}

/// One option: code, length, value.
pub fn option(code: u8, value: &[u8]) -> Vec<u8> {
    let mut encoded = vec![code, u8::try_from(value.len()).unwrap()];
    encoded.extend_from_slice(value);
    encoded
}

/// An options area: the options, then the terminator.
pub fn options(each: &[Vec<u8>]) -> Vec<u8> {
    let mut area: Vec<u8> = each.concat();
    area.push(OPTION_END);
    area
}

/// The option codes an encoded message carries, in the order its bytes
/// carry them.
///
/// Walked rather than decoded, because the order is what is being
/// asserted and no decoder that keys options by code can report it.
/// Panics on a malformed options area, which in a test is the right
/// answer: the input is this crate's own output.
pub fn option_codes(frame: &[u8]) -> Vec<u8> {
    let mut codes = Vec::new();
    let mut area = &frame[FIXED_BYTES..];
    loop {
        match area.first() {
            None | Some(&OPTION_END) => return codes,
            Some(&OPTION_PAD) => area = &area[1..],
            Some(&code) => {
                let length = usize::from(area[1]);
                codes.push(code);
                area = &area[2 + length..];
            }
        }
    }
}

/// The value an encoded message gives one option, if it carries it.
///
/// Panics as `option_codes` does, and for the same reason.
pub fn option_value(frame: &[u8], wanted: u8) -> Option<Vec<u8>> {
    let mut area = &frame[FIXED_BYTES..];
    loop {
        match area.first() {
            None | Some(&OPTION_END) => return None,
            Some(&OPTION_PAD) => area = &area[1..],
            Some(&code) => {
                let length = usize::from(area[1]);
                if code == wanted {
                    return Some(area[2..2 + length].to_vec());
                }
                area = &area[2 + length..];
            }
        }
    }
}
