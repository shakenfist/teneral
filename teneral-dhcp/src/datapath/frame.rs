//! An Ethernet frame built by hand, for a client with no address.
//!
//! A reply to a client that does not have an address yet is addressed
//! to the address it is being offered. Ordinary IP routing cannot
//! deliver that: there is no route to an address nobody holds, and
//! address resolution cannot find a machine that does not yet answer
//! for it. Below the IP layer there is a way -- a frame addressed to
//! the hardware address the client does hold, carrying an IP packet
//! addressed to the address it does not -- and this is where that
//! frame is written.
//!
//! The server is supplying the address resolution that the protocol's
//! own ordering makes impossible. That is the whole of why the three
//! headers are built here rather than left to the kernel.
//!
//! # Nothing here indexes and nothing here wraps
//!
//! A frame is assembled by pushing fields in the order the protocols
//! list them, never by writing into a buffer at a computed offset. The
//! two checksums are the one place that would ordinarily want an
//! offset, because each covers bytes that surround it, and each is
//! handled by computing the sum over the pieces and then appending
//! them with the sum between -- so there is no position in this file
//! that could be wrong and no slice that could be out of range.
//!
//! Every length is narrowed with a conversion that is checked. The
//! payload is this front-end's own answer, bounded far below what the
//! fields can carry, so the refusals below are backstops rather than
//! live paths; a backstop that reports is worth more than one that
//! truncates.

use std::net::Ipv4Addr;

use crate::datapath::ETHERNET_PROTOCOL_IPV4;
use crate::datapath::error::{Error, Result};
use crate::datapath::interface::ETHERNET_ADDRESS_BYTES;

/// The octets an Ethernet header occupies: two addresses and a
/// protocol.
const ETHERNET_HEADER_BYTES: usize = 14;

/// The octets an IPv4 header occupies, with no options.
const IPV4_HEADER_BYTES: usize = 20;

/// The octets a UDP header occupies.
const UDP_HEADER_BYTES: usize = 8;

/// Version four, and a header of five thirty-two-bit words.
const VERSION_AND_HEADER_LENGTH: u8 = 0x45;

/// No differentiated services: this is a reply on a local link.
const DIFFERENTIATED_SERVICES: u8 = 0;

/// The hops a reply is allowed, which is one and then some. A DHCP
/// reply never leaves the link it was asked on -- a relayed request is
/// not answered at all -- so this is a conventional value rather than
/// a load-bearing one.
const TIME_TO_LIVE: u8 = 64;

/// The IP protocol number UDP carries.
const PROTOCOL_UDP: u8 = 17;

/// What a frame carries, and who it is addressed to.
///
/// A structure rather than a long argument list, so that no caller can
/// transpose a source for a destination: each field is named at the
/// call site.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame<'a> {
    /// The hardware address the interface sends from.
    pub(crate) source_hardware_address: [u8; ETHERNET_ADDRESS_BYTES],
    /// The hardware address the client holds, which is the only
    /// address it can be reached at.
    pub(crate) destination_hardware_address: [u8; ETHERNET_ADDRESS_BYTES],
    /// The interface's own address, which is also the server
    /// identifier the payload carries.
    pub(crate) source: Ipv4Addr,
    /// The address the client is being given, which it does not hold
    /// yet.
    pub(crate) destination: Ipv4Addr,
    /// The port the server answers from.
    pub(crate) source_port: u16,
    /// The port the client listens on.
    pub(crate) destination_port: u16,
    /// The reply itself.
    pub(crate) payload: &'a [u8],
}

impl Frame<'_> {
    /// The frame, as it goes on the interface.
    pub(crate) fn encode(&self) -> Result<Vec<u8>> {
        let udp_length = length(UDP_HEADER_BYTES, self.payload.len())?;
        let total_length = length(IPV4_HEADER_BYTES, usize::from(udp_length))?;

        // The IPv4 header in two pieces, with its checksum between
        // them: everything before the checksum field, and the two
        // addresses after it.
        let [total_high, total_low] = total_length.to_be_bytes();
        let leading = [
            VERSION_AND_HEADER_LENGTH,
            DIFFERENTIATED_SERVICES,
            total_high,
            total_low,
            // No identification and no fragmentation. A reply this
            // short is never fragmented, and an identification is only
            // meaningful to something reassembling fragments.
            0,
            0,
            0,
            0,
            TIME_TO_LIVE,
            PROTOCOL_UDP,
        ];
        let addresses = [self.source.octets(), self.destination.octets()].concat();
        let header_checksum = ones_complement(
            leading
                .into_iter()
                .chain([0, 0])
                .chain(addresses.iter().copied()),
        );

        // The UDP header in two pieces for the same reason, and its
        // checksum covers a pseudo-header of the addresses above as
        // well as its own bytes and the payload.
        let [source_high, source_low] = self.source_port.to_be_bytes();
        let [destination_high, destination_low] = self.destination_port.to_be_bytes();
        let [udp_high, udp_low] = udp_length.to_be_bytes();
        let datagram = [
            source_high,
            source_low,
            destination_high,
            destination_low,
            udp_high,
            udp_low,
        ];
        let pseudo_header = [0, PROTOCOL_UDP, udp_high, udp_low];
        let datagram_checksum = ones_complement(
            addresses
                .iter()
                .copied()
                .chain(pseudo_header)
                .chain(datagram)
                .chain([0, 0])
                .chain(self.payload.iter().copied()),
        );
        // A zero checksum means "not computed" in a UDP datagram, so a
        // sum that comes to zero is sent as its other representation
        // rather than as a claim that there is no checksum.
        let datagram_checksum = match datagram_checksum {
            0 => u16::MAX,
            computed => computed,
        };

        let mut frame = Vec::with_capacity(
            ETHERNET_HEADER_BYTES + IPV4_HEADER_BYTES + UDP_HEADER_BYTES + self.payload.len(),
        );
        frame.extend_from_slice(&self.destination_hardware_address);
        frame.extend_from_slice(&self.source_hardware_address);
        frame.extend_from_slice(&ETHERNET_PROTOCOL_IPV4.to_be_bytes());
        frame.extend_from_slice(&leading);
        frame.extend_from_slice(&header_checksum.to_be_bytes());
        frame.extend_from_slice(&addresses);
        frame.extend_from_slice(&datagram);
        frame.extend_from_slice(&datagram_checksum.to_be_bytes());
        frame.extend_from_slice(self.payload);

        Ok(frame)
    }
}

/// A header's length plus what follows it, as the field that describes
/// it holds it.
fn length(header: usize, rest: usize) -> Result<u16> {
    let length = header.saturating_add(rest);

    u16::try_from(length).map_err(|_| Error::ReplyTooLong { length })
}

/// The internet checksum of a run of octets.
///
/// The ones-complement sum of the octets read as sixteen-bit words,
/// complemented. An odd number of octets is padded with a zero, which
/// is what the specification says to do and what the callers above
/// never need: every run they pass is even.
///
/// The sum cannot overflow. Each octet contributes at most sixty-five
/// thousand and the longest run any caller passes is a frame's worth,
/// so the total stays four orders of magnitude below the type's
/// ceiling; the saturation is there because this layer does not panic
/// to prove an arithmetic fact.
fn ones_complement<I>(octets: I) -> u16
where
    I: IntoIterator<Item = u8>,
{
    let mut sum: u32 = 0;
    let mut high = true;

    for octet in octets {
        let word = if high {
            u32::from(octet) << 8
        } else {
            u32::from(octet)
        };
        sum = sum.saturating_add(word);
        high = !high;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff).saturating_add(sum >> 16);
    }

    !u16::try_from(sum & 0xffff).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hardware address of the interface in these tests.
    const SERVER_MAC: [u8; ETHERNET_ADDRESS_BYTES] = [0x52, 0x54, 0x00, 0x00, 0x00, 0x01];

    /// The hardware address of the client in these tests.
    const CLIENT_MAC: [u8; ETHERNET_ADDRESS_BYTES] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

    #[test]
    fn a_frame_is_addressed_to_the_hardware_address_the_client_holds() {
        let frame = offer(&[0xaa; 300]).encode().expect("a frame");

        assert_eq!(
            frame.get(..6),
            Some(CLIENT_MAC.as_slice()),
            "the destination comes first, and it is the client's own"
        );
        assert_eq!(
            frame.get(6..12),
            Some(SERVER_MAC.as_slice()),
            "then the interface's"
        );
        assert_eq!(
            frame.get(12..14),
            Some([0x08, 0x00].as_slice()),
            "and then the protocol the frame carries"
        );
    }

    #[test]
    fn the_ip_packet_is_addressed_to_the_address_the_client_does_not_hold_yet() {
        let frame = offer(&[0xaa; 300]).encode().expect("a frame");
        let packet = frame.get(14..).expect("an IP packet follows the header");

        assert_eq!(packet.get(..2), Some([0x45, 0x00].as_slice()));
        assert_eq!(
            packet.get(12..16),
            Some([10, 0, 0, 1].as_slice()),
            "from the interface's own address"
        );
        assert_eq!(
            packet.get(16..20),
            Some([10, 0, 0, 128].as_slice()),
            "to the address being offered, which routing could not deliver"
        );
        assert_eq!(
            packet.get(9),
            Some(&PROTOCOL_UDP),
            "and it carries a UDP datagram"
        );
    }

    #[test]
    fn the_ports_are_the_servers_and_the_clients() {
        let frame = offer(&[0xaa; 300]).encode().expect("a frame");
        let datagram = frame.get(34..).expect("a UDP datagram follows the headers");

        assert_eq!(datagram.get(..2), Some([0, 67].as_slice()));
        assert_eq!(datagram.get(2..4), Some([0, 68].as_slice()));
    }

    #[test]
    fn the_lengths_describe_what_the_frame_holds() {
        let payload = [0xaa; 300];
        let frame = offer(&payload).encode().expect("a frame");

        assert_eq!(
            frame.len(),
            ETHERNET_HEADER_BYTES + IPV4_HEADER_BYTES + UDP_HEADER_BYTES + payload.len()
        );
        let packet = frame.get(14..).expect("an IP packet");
        assert_eq!(
            packet.get(2..4).map(be),
            Some(u16::try_from(IPV4_HEADER_BYTES + UDP_HEADER_BYTES + payload.len()).unwrap()),
            "the IPv4 total length counts its own header"
        );
        let datagram = frame.get(34..).expect("a UDP datagram");
        assert_eq!(
            datagram.get(4..6).map(be),
            Some(u16::try_from(UDP_HEADER_BYTES + payload.len()).unwrap()),
            "and the UDP length counts its own header and not the IP one"
        );
    }

    /// The point of a checksum is that a receiver recomputing it over
    /// the header, the field included, gets zero. That is the property
    /// asserted rather than the bytes, because asserting the bytes
    /// would be asserting this file against itself.
    #[test]
    fn the_header_checksum_is_what_a_receiver_recomputes_to_zero() {
        let frame = offer(&[0xaa; 300]).encode().expect("a frame");
        let header = frame.get(14..34).expect("an IPv4 header");

        assert_eq!(
            ones_complement(header.iter().copied()),
            0,
            "a receiver's sum over the whole header comes to zero"
        );
    }

    #[test]
    fn the_datagram_checksum_covers_the_addresses_above_it() {
        let payload = [0xaa; 300];
        let frame = offer(&payload).encode().expect("a frame");
        let addresses = frame.get(26..34).expect("the IPv4 addresses");
        let datagram = frame.get(34..).expect("a UDP datagram");
        let [high, low] = u16::try_from(UDP_HEADER_BYTES + payload.len())
            .unwrap()
            .to_be_bytes();

        assert_eq!(
            ones_complement(
                addresses
                    .iter()
                    .copied()
                    .chain([0, PROTOCOL_UDP, high, low])
                    .chain(datagram.iter().copied())
            ),
            0,
            "a receiver's sum over the pseudo-header and the datagram comes to zero"
        );
    }

    #[test]
    fn a_payload_too_long_for_the_length_fields_is_refused_rather_than_truncated() {
        let payload = vec![0; usize::from(u16::MAX)];
        let refusal = offer(&payload)
            .encode()
            .expect_err("a length field cannot describe it");

        assert!(matches!(refusal, Error::ReplyTooLong { .. }), "{refusal}");
    }

    /// A frame carrying `payload` from this file's server to its
    /// client.
    fn offer(payload: &[u8]) -> Frame<'_> {
        Frame {
            source_hardware_address: SERVER_MAC,
            destination_hardware_address: CLIENT_MAC,
            source: Ipv4Addr::new(10, 0, 0, 1),
            destination: Ipv4Addr::new(10, 0, 0, 128),
            source_port: 67,
            destination_port: 68,
            payload,
        }
    }

    /// A two-octet field, read as the network writes it.
    fn be(field: &[u8]) -> u16 {
        u16::from_be_bytes([field[0], field[1]])
    }
}
