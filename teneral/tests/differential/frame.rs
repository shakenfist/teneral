//! A captured frame read field by field, for the comparison.
//!
//! The comparison is field-wise rather than byte-wise, and this is
//! what makes it possible: both sides of a case are read by the same
//! reader, so a difference it reports is a difference in the bytes and
//! not a difference in how the two were understood.
//!
//! It has to be a reader of its own rather than the front-end's. The
//! front-end parses a *request* -- `wire::parse` refuses a message
//! whose `op` says it came from a server, deliberately, since a reply
//! arriving on port 67 is another server's traffic -- so it cannot be
//! pointed at either half of a captured pair. And a reply is exactly
//! what has to be read here.
//!
//! Nothing in this file indexes a slice. The rig's test target has no
//! libtest harness, so `allow-unwrap-in-tests` does not reach it and
//! it obeys `unwrap_used` like ordinary code; a fixture is also bytes
//! somebody may edit by hand, so a short frame has to be a message
//! rather than a panic.

use std::net::Ipv4Addr;

/// The octets an Ethernet header occupies.
const ETHERNET_HEADER_BYTES: usize = 14;

/// The four octets that say an options area is DHCP's.
const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];

/// The option code that pads the options area.
const OPTION_PAD: u8 = 0;

/// The option code that ends it.
const OPTION_END: u8 = 255;

/// The octets of fixed BOOTP header before the cookie.
const BOOTP_FIXED_BYTES: usize = 236;

/// The port a server answers on.
const SERVER_PORT: u16 = 67;

/// The port a client listens on.
const CLIENT_PORT: u16 = 68;

/// One frame of a captured exchange, read apart.
#[derive(Debug)]
pub struct Frame {
    /// Where the frame was addressed, below the IP layer.
    pub ethernet_destination: Vec<u8>,
    /// Where the packet inside it was addressed.
    pub ip_source: Ipv4Addr,
    pub ip_destination: Ipv4Addr,
    /// The BOOTP message the datagram carries.
    pub datagram: Vec<u8>,
    pub op: u8,
    pub htype: u8,
    pub hlen: u8,
    pub hops: u8,
    pub xid: [u8; 4],
    pub secs: u16,
    pub flags: [u8; 2],
    pub ciaddr: Ipv4Addr,
    pub yiaddr: Ipv4Addr,
    pub siaddr: Ipv4Addr,
    pub giaddr: Ipv4Addr,
    /// All sixteen octets of it, which is what the field is.
    pub chaddr: Vec<u8>,
    pub sname: Vec<u8>,
    pub file: Vec<u8>,
    /// The options, in the order the bytes carry them. Empty where
    /// there is no magic cookie at all.
    pub options: Vec<(u8, Vec<u8>)>,
}

impl Frame {
    /// The value one option carries, if this frame carries it.
    pub fn option(&self, code: u8) -> Option<&[u8]> {
        self.options
            .iter()
            .find(|(carried, _)| *carried == code)
            .map(|(_, value)| value.as_slice())
    }

    /// The option codes, in order.
    pub fn option_codes(&self) -> Vec<u8> {
        self.options.iter().map(|(code, _)| *code).collect()
    }
}

/// Read one Ethernet frame carrying an IPv4 UDP datagram carrying a
/// BOOTP message.
pub fn read(octets: &[u8]) -> Result<Frame, String> {
    let ethernet_destination = slice(octets, 0, 6, "the Ethernet destination")?;
    let protocol = slice(octets, 12, 2, "the EtherType")?;
    if protocol != [0x08, 0x00] {
        return Err(format!(
            "carries EtherType 0x{:02x}{:02x} rather than IPv4's 0x0800",
            protocol.first().copied().unwrap_or(0),
            protocol.get(1).copied().unwrap_or(0)
        ));
    }

    let version_and_length = byte(octets, ETHERNET_HEADER_BYTES, "the IPv4 version")?;
    let header_bytes = usize::from(version_and_length & 0x0f) * 4;
    if header_bytes < 20 {
        return Err(format!(
            "claims an IPv4 header of {header_bytes} octets, and twenty is the smallest there is"
        ));
    }
    let ip_protocol = byte(octets, ETHERNET_HEADER_BYTES + 9, "the IPv4 protocol")?;
    if ip_protocol != 17 {
        return Err(format!(
            "carries IP protocol {ip_protocol} rather than UDP's 17"
        ));
    }
    let ip_source = address(octets, ETHERNET_HEADER_BYTES + 12, "the IPv4 source")?;
    let ip_destination = address(octets, ETHERNET_HEADER_BYTES + 16, "the IPv4 destination")?;

    let udp = ETHERNET_HEADER_BYTES
        .checked_add(header_bytes)
        .ok_or_else(|| "an IPv4 header length that does not fit an offset".to_owned())?;
    let udp_source_port = be16(octets, udp, "the UDP source port")?;
    let udp_destination_port = be16(octets, udp + 2, "the UDP destination port")?;
    // The protocol's own ports, in one direction or the other. A
    // fixture is a frame of a DHCP exchange, so a frame that is not one
    // has been taken from the wrong place in a capture -- which is a
    // mistake worth catching here rather than as a puzzling difference
    // in a field further down.
    if (udp_source_port, udp_destination_port) != (SERVER_PORT, CLIENT_PORT)
        && (udp_source_port, udp_destination_port) != (CLIENT_PORT, SERVER_PORT)
    {
        return Err(format!(
            "goes from port {udp_source_port} to port {udp_destination_port}, and a DHCPv4 \
             frame goes between {SERVER_PORT} and {CLIENT_PORT}"
        ));
    }

    let datagram = octets
        .get(udp + 8..)
        .ok_or_else(|| "stops before the UDP payload begins".to_owned())?
        .to_vec();

    let op = byte(&datagram, 0, "op")?;
    let htype = byte(&datagram, 1, "htype")?;
    let hlen = byte(&datagram, 2, "hlen")?;
    let hops = byte(&datagram, 3, "hops")?;
    let xid = four(&datagram, 4, "xid")?;
    let secs = be16(&datagram, 8, "secs")?;
    let flags = two(&datagram, 10, "flags")?;
    let ciaddr = address(&datagram, 12, "ciaddr")?;
    let yiaddr = address(&datagram, 16, "yiaddr")?;
    let siaddr = address(&datagram, 20, "siaddr")?;
    let giaddr = address(&datagram, 24, "giaddr")?;
    let chaddr = slice(&datagram, 28, 16, "chaddr")?;
    let sname = slice(&datagram, 44, 64, "sname")?;
    let file = slice(&datagram, 108, 128, "file")?;

    let options = match datagram.get(BOOTP_FIXED_BYTES..BOOTP_FIXED_BYTES + 4) {
        Some(cookie) if cookie == MAGIC_COOKIE => {
            let area = datagram
                .get(BOOTP_FIXED_BYTES + 4..)
                .ok_or_else(|| "stops at the magic cookie".to_owned())?;
            read_options(area)?
        }
        // No cookie means a plain BOOTP message, which carries no
        // options at all. That is a thing a case measures rather than
        // an error.
        _ => Vec::new(),
    };

    Ok(Frame {
        ethernet_destination,
        ip_source,
        ip_destination,
        datagram,
        op,
        htype,
        hlen,
        hops,
        xid,
        secs,
        flags,
        ciaddr,
        yiaddr,
        siaddr,
        giaddr,
        chaddr,
        sname,
        file,
        options,
    })
}

/// The option records of an options area, in order.
///
/// Pad is skipped and End stops the walk, which is the protocol's own
/// framing. A record that runs past the end is an error rather than a
/// short read: a fixture is a frame as it was captured, so the bytes
/// after it are not padding that happens to be missing.
fn read_options(area: &[u8]) -> Result<Vec<(u8, Vec<u8>)>, String> {
    let mut options = Vec::new();
    let mut at = 0_usize;
    while let Some(code) = area.get(at).copied() {
        if code == OPTION_END {
            break;
        }
        if code == OPTION_PAD {
            at += 1;
            continue;
        }
        let length = usize::from(
            area.get(at + 1)
                .copied()
                .ok_or_else(|| format!("option {code} at offset {at} has no length octet"))?,
        );
        let value = area
            .get(at + 2..at + 2 + length)
            .ok_or_else(|| {
                format!("option {code} at offset {at} claims {length} octets and runs off the end")
            })?
            .to_vec();
        options.push((code, value));
        at += 2 + length;
    }
    Ok(options)
}

fn byte(octets: &[u8], at: usize, what: &str) -> Result<u8, String> {
    octets
        .get(at)
        .copied()
        .ok_or_else(|| format!("stops before {what} at offset {at}"))
}

fn slice(octets: &[u8], at: usize, length: usize, what: &str) -> Result<Vec<u8>, String> {
    octets
        .get(at..at + length)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| format!("stops inside {what} at offset {at}"))
}

fn two(octets: &[u8], at: usize, what: &str) -> Result<[u8; 2], String> {
    let found = slice(octets, at, 2, what)?;

    found
        .try_into()
        .map_err(|_| format!("{what} is not two octets"))
}

fn four(octets: &[u8], at: usize, what: &str) -> Result<[u8; 4], String> {
    let found = slice(octets, at, 4, what)?;

    found
        .try_into()
        .map_err(|_| format!("{what} is not four octets"))
}

fn be16(octets: &[u8], at: usize, what: &str) -> Result<u16, String> {
    Ok(u16::from_be_bytes(two(octets, at, what)?))
}

fn address(octets: &[u8], at: usize, what: &str) -> Result<Ipv4Addr, String> {
    Ok(Ipv4Addr::from(four(octets, at, what)?))
}
