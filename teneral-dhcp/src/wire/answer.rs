//! What goes back, and the bytes it becomes.
//!
//! The option set an answer carries is measured rather than chosen:
//! it is what a client that asked for nothing was observed to receive,
//! which is nine options -- message type, server identifier, lease
//! time, renewal and rebinding times, subnet mask, broadcast address,
//! router and nameserver -- in that order, with the router and the
//! nameserver defaulting to the server's own address.
//!
//! Every one of them comes from the only honest source. The message
//! type is the answer being built. The server identifier, the router
//! and the nameserver are the address of the interface the request
//! arrived on. The mask and the broadcast address are that address
//! and the prefix configured beside it. The lease time is the
//! network's, and the renewal and rebinding times are a half and
//! seven-eighths of it, which are the measured fractions.
//!
//! Reading an interface's address and prefix is observation, not
//! per-network state: nothing is remembered between packets. Which is
//! why [`ServerFacts`] is a parameter of every answer rather than
//! something this layer holds.
//!
//! # What is deliberately not here
//!
//! A router or a nameserver that is not this machine, a domain name,
//! an MTU, or any option outside the nine. Each of those is a row in
//! a table that does not exist yet, and the shape of that table
//! should be settled by the first consumer that names an option it
//! needs rather than by guessing. Nothing in the encoder below is
//! arranged to make that harder: the options are written one call per
//! option, in a sequence a reader can reorder.

use std::net::Ipv4Addr;

use crate::wire::error::{Error, Result};
use crate::wire::request::Request;
use crate::wire::{
    BOOT_REPLY, CHADDR_BYTES, FILE_BYTES, FIXED_BYTES, MAGIC_COOKIE, OPTION_END, SNAME_BYTES,
};

/// Option 53, the message type.
const OPTION_MESSAGE_TYPE: u8 = 53;
/// Option 54, the server identifier.
const OPTION_SERVER_IDENTIFIER: u8 = 54;
/// Option 51, how long the address is held for.
const OPTION_LEASE_TIME: u8 = 51;
/// Option 58, when the client should start renewing.
const OPTION_RENEWAL_TIME: u8 = 58;
/// Option 59, when the client should start looking for another server.
const OPTION_REBINDING_TIME: u8 = 59;
/// Option 1, the subnet mask.
const OPTION_SUBNET_MASK: u8 = 1;
/// Option 28, the broadcast address.
const OPTION_BROADCAST_ADDRESS: u8 = 28;
/// Option 3, the router.
const OPTION_ROUTER: u8 = 3;
/// Option 6, the nameserver.
const OPTION_DOMAIN_NAME_SERVER: u8 = 6;

/// The bits an IPv4 address has, which is the longest prefix there is.
const ADDRESS_BITS: u8 = 32;

/// The options area of the largest answer this layer builds: nine
/// options, one of them a single octet and the rest four, and the
/// terminator. Used to size the buffer once rather than to bound
/// anything.
const LARGEST_OPTIONS_BYTES: usize = 3 + 8 * 6 + 1;

/// Which answer this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerKind {
    /// What a DISCOVER is answered with: here is an address, and this
    /// is what you would get with it.
    Offer,
    /// What a REQUEST or an INFORM is answered with.
    Acknowledgement,
    /// A refusal: the address the client asked for is not one it may
    /// have here. A client that receives one starts again from
    /// DISCOVER.
    Refusal,
}

impl AnswerKind {
    /// The option 53 value RFC 2131 gives this answer.
    fn code(self) -> u8 {
        match self {
            AnswerKind::Offer => 2,
            AnswerKind::Acknowledgement => 5,
            AnswerKind::Refusal => 6,
        }
    }
}

/// What the interface a request arrived on says about this server.
///
/// Four of the nine options come out of this and nothing else, which
/// is the point of gathering it into one value: an answer built from
/// an interface's address and an unrelated prefix would be an answer
/// nobody could reconstruct afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerFacts {
    address: Ipv4Addr,
    prefix_length: u8,
}

impl ServerFacts {
    /// The address and prefix an interface was observed to hold.
    ///
    /// A prefix longer than an address has bits is refused rather than
    /// clamped. The value comes from the kernel, so it being wrong is
    /// a fault somewhere above this layer -- and a mask computed from
    /// a clamped prefix would be wrong rather than absent, which a
    /// client would then carry for the life of its lease.
    pub fn new(address: Ipv4Addr, prefix_length: u8) -> Result<Self> {
        if prefix_length > ADDRESS_BITS {
            return Err(Error::PrefixTooLong { prefix_length });
        }
        Ok(ServerFacts {
            address,
            prefix_length,
        })
    }

    /// The interface's address, which is also the server identifier,
    /// the router and the nameserver.
    pub fn address(&self) -> Ipv4Addr {
        self.address
    }

    /// The prefix, in bits.
    pub fn prefix_length(&self) -> u8 {
        self.prefix_length
    }

    /// The subnet mask the prefix describes.
    ///
    /// `checked_shl` rather than a shift, because a prefix of zero
    /// would shift a 32-bit value by 32 and that is undefined for the
    /// operator and a panic in debug builds. There is no prefix above
    /// 32 to worry about: the constructor refused it.
    pub fn netmask(&self) -> Ipv4Addr {
        let shift = u32::from(ADDRESS_BITS.saturating_sub(self.prefix_length));
        Ipv4Addr::from_bits(u32::MAX.checked_shl(shift).unwrap_or(0))
    }

    /// The broadcast address of the subnet the prefix describes.
    pub fn broadcast(&self) -> Ipv4Addr {
        Ipv4Addr::from_bits(self.address.to_bits() | !self.netmask().to_bits())
    }
}

/// How long an address is given out for, and the two times derived
/// from it.
///
/// A type rather than a number because the narrowing is real: the
/// store holds the configured value as a signed 64-bit count of
/// seconds and the option is an unsigned 32-bit one, so the refusal
/// belongs where the width of the field is known rather than at the
/// call site that happened to do the conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseSeconds(u32);

impl LeaseSeconds {
    /// The lease time a network is configured with.
    ///
    /// Zero and below are refused along with anything the option
    /// cannot carry. A lease of no seconds is not a shorter lease, it
    /// is an answer no client can use, and the store's own constraint
    /// already says a lease time is positive -- so a value arriving
    /// here outside the range means something between the two has
    /// gone wrong and should be visible rather than rounded.
    pub fn new(seconds: i64) -> Result<Self> {
        match u32::try_from(seconds) {
            Ok(0) | Err(_) => Err(Error::LeaseTimeNotRepresentable { seconds }),
            Ok(seconds) => Ok(LeaseSeconds(seconds)),
        }
    }

    /// The lease time itself, for option 51.
    pub fn seconds(self) -> u32 {
        self.0
    }

    /// Option 58: when the client should start renewing, at half the
    /// lease. A measured fraction, not a chosen one.
    pub fn renewal(self) -> u32 {
        self.0 / 2
    }

    /// Option 59: when the client should start looking for another
    /// server, at seven-eighths of the lease. Also measured.
    ///
    /// Computed in 64 bits and narrowed, so that the multiplication
    /// cannot wrap for a lease near the top of the type. Seven
    /// eighths of a `u32` fits in a `u32`, so the conversion back
    /// cannot fail; the saturation is there because this layer does
    /// not panic to prove an arithmetic fact.
    pub fn rebinding(self) -> u32 {
        u32::try_from(u64::from(self.0) * 7 / 8).unwrap_or(u32::MAX)
    }
}

/// An answer, and the parameters the bytes are built from.
///
/// Built through the constructors below rather than by setting
/// fields, because which parameters an answer carries depends on
/// which answer it is: a refusal has no address and no lease time, and
/// an answer to an INFORM has the configuration and no lease. Each
/// constructor also takes the request it answers, which is how the
/// fields RFC 2131 requires a reply to echo come to be echoed without
/// a caller remembering to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    kind: AnswerKind,
    transaction: u32,
    broadcast: bool,
    hardware_type: u8,
    hardware_address: Vec<u8>,
    relay_address: Ipv4Addr,
    client_address: Ipv4Addr,
    server: ServerFacts,
    lease: Option<LeaseSeconds>,
}

impl Answer {
    /// An OFFER: an address a client may have, and the configuration
    /// that comes with it.
    pub fn offer(
        request: &Request,
        server: ServerFacts,
        address: Ipv4Addr,
        lease: LeaseSeconds,
    ) -> Self {
        Answer::echoing(AnswerKind::Offer, request, server, address, Some(lease))
    }

    /// An ACK: the client has the address, for this long.
    pub fn acknowledge(
        request: &Request,
        server: ServerFacts,
        address: Ipv4Addr,
        lease: LeaseSeconds,
    ) -> Self {
        Answer::echoing(
            AnswerKind::Acknowledgement,
            request,
            server,
            address,
            Some(lease),
        )
    }

    /// An ACK for an INFORM: the configuration, and no lease.
    ///
    /// The client already has its address and did not ask for one, so
    /// there is nothing to lease and `yiaddr` stays unspecified. A
    /// lease time here would be a lease on an address this server
    /// never gave out and may not even hold a record of.
    pub fn inform(request: &Request, server: ServerFacts) -> Self {
        Answer::echoing(
            AnswerKind::Acknowledgement,
            request,
            server,
            Ipv4Addr::UNSPECIFIED,
            None,
        )
    }

    /// A NAK: whatever the client asked for, it may not have it here.
    ///
    /// Carries the message type and the server identifier and nothing
    /// else. A refusal that carried a mask and a router would be
    /// describing a network the client is being told it is not on.
    pub fn refuse(request: &Request, server: ServerFacts) -> Self {
        Answer::echoing(
            AnswerKind::Refusal,
            request,
            server,
            Ipv4Addr::UNSPECIFIED,
            None,
        )
    }

    /// The fields every answer takes from the request it answers.
    ///
    /// One place, so that an answer with the wrong transaction or the
    /// wrong hardware address is not a mistake a later constructor can
    /// make.
    fn echoing(
        kind: AnswerKind,
        request: &Request,
        server: ServerFacts,
        client_address: Ipv4Addr,
        lease: Option<LeaseSeconds>,
    ) -> Self {
        Answer {
            kind,
            transaction: request.transaction,
            broadcast: request.broadcast,
            hardware_type: request.hardware_type,
            hardware_address: request.hardware_address.clone(),
            relay_address: request.relay_address,
            client_address,
            server,
            lease,
        }
    }

    /// Which answer this is.
    pub fn kind(&self) -> AnswerKind {
        self.kind
    }

    /// The address being offered or acknowledged, or the unspecified
    /// address where there is none.
    ///
    /// The datapath needs it: a reply to a client that has no address
    /// yet is unicast to the address it is being given, which only
    /// works below the IP layer.
    pub fn client_address(&self) -> Ipv4Addr {
        self.client_address
    }

    /// The hardware address the reply is for, which is the only
    /// address a client with no IP address can be reached at.
    pub fn hardware_address(&self) -> &[u8] {
        &self.hardware_address
    }

    /// Whether the client asked for a broadcast reply.
    pub fn broadcast(&self) -> bool {
        self.broadcast
    }

    /// The datagram, as it goes on the wire.
    ///
    /// Infallible, and that is a property of the inputs rather than of
    /// optimism: a `Request` carries a hardware address of at most
    /// sixteen octets, a `ServerFacts` carries a prefix of at most
    /// thirty-two bits, and every option written below is a fixed
    /// width. There is no length here that a byte cannot describe and
    /// no offset that could be wrong, because the fields are pushed in
    /// the order RFC 2131 lists them rather than written into a buffer
    /// at computed positions.
    pub fn encode(&self) -> Vec<u8> {
        let mut frame = Vec::with_capacity(FIXED_BYTES + LARGEST_OPTIONS_BYTES);

        // The fixed BOOTP header, field by field, in order. `op`,
        // `htype`, `hlen`, `hops`: `hlen` describes the address
        // written below rather than being copied from the request --
        // the two agree, and the one that is right is the one that
        // matches the bytes -- and `hops` is a relay's business, which
        // a server sets none of.
        frame.extend_from_slice(&[
            BOOT_REPLY,
            self.hardware_type,
            hardware_length(&self.hardware_address),
            0,
        ]);
        frame.extend_from_slice(&self.transaction.to_be_bytes());
        // `secs` is the client's count of how long it has been trying.
        frame.extend_from_slice(&0_u16.to_be_bytes());
        frame.extend_from_slice(&self.flags().to_be_bytes());
        // `ciaddr` is the client's own, and a reply does not set it.
        frame.extend_from_slice(&Ipv4Addr::UNSPECIFIED.octets());
        frame.extend_from_slice(&self.client_address.octets());
        // `siaddr` names a boot server, which this front-end has none
        // of.
        frame.extend_from_slice(&Ipv4Addr::UNSPECIFIED.octets());
        // `giaddr` is echoed so that a relay could route the reply.
        frame.extend_from_slice(&self.relay_address.octets());
        // `chaddr` is sixteen octets whatever `hlen` says, which is
        // the defect this module guards against seen from the other
        // side: a reply that wrote only `hlen` octets here would
        // misalign every field after it.
        frame.extend_from_slice(&self.hardware_address);
        pad(
            &mut frame,
            CHADDR_BYTES.saturating_sub(self.hardware_address.len()),
        );
        // `sname` and `file`, both empty.
        pad(&mut frame, SNAME_BYTES + FILE_BYTES);
        frame.extend_from_slice(&MAGIC_COOKIE);

        // The options, in the order they were measured in. Nine for an
        // answer that carries a lease, six for an answer to an INFORM
        // and two for a refusal.
        push_byte(&mut frame, OPTION_MESSAGE_TYPE, self.kind.code());
        push_address(&mut frame, OPTION_SERVER_IDENTIFIER, self.server.address());
        if let Some(lease) = self.lease {
            push_seconds(&mut frame, OPTION_LEASE_TIME, lease.seconds());
            push_seconds(&mut frame, OPTION_RENEWAL_TIME, lease.renewal());
            push_seconds(&mut frame, OPTION_REBINDING_TIME, lease.rebinding());
        }
        if self.kind != AnswerKind::Refusal {
            push_address(&mut frame, OPTION_SUBNET_MASK, self.server.netmask());
            push_address(
                &mut frame,
                OPTION_BROADCAST_ADDRESS,
                self.server.broadcast(),
            );
            push_address(&mut frame, OPTION_ROUTER, self.server.address());
            push_address(&mut frame, OPTION_DOMAIN_NAME_SERVER, self.server.address());
        }
        frame.push(OPTION_END);

        frame
    }

    /// The BOOTP flags word.
    ///
    /// The broadcast bit is carried back as the client set it. The
    /// flag is read as a request for broadcast and never as permission
    /// to unicast -- a client that cleared it was observed accepting a
    /// broadcast reply in every transaction measured -- so what is in
    /// the reply's flags is a record of what the client asked for and
    /// not a decision about how the reply is sent. That decision
    /// belongs to the datapath.
    fn flags(&self) -> u16 {
        if self.broadcast { 0x8000 } else { 0 }
    }
}

/// `hlen` for an address of this length.
///
/// A `Request`'s hardware address is at most sixteen octets, so the
/// conversion cannot fail; the saturation is there because this layer
/// does not panic to prove that.
fn hardware_length(address: &[u8]) -> u8 {
    u8::try_from(address.len()).unwrap_or(u8::MAX)
}

/// Append `count` zero octets.
fn pad(frame: &mut Vec<u8>, count: usize) {
    frame.extend(std::iter::repeat_n(0_u8, count));
}

/// Append a one-octet option.
///
/// Three helpers rather than one taking a slice, so that no option's
/// length byte is derived from a runtime length at all: each of them
/// writes a width that is a literal beside the value it describes.
fn push_byte(frame: &mut Vec<u8>, code: u8, value: u8) {
    frame.extend_from_slice(&[code, 1, value]);
}

/// Append an option whose value is one IPv4 address.
fn push_address(frame: &mut Vec<u8>, code: u8, value: Ipv4Addr) {
    frame.extend_from_slice(&[code, 4]);
    frame.extend_from_slice(&value.octets());
}

/// Append an option whose value is a count of seconds.
fn push_seconds(frame: &mut Vec<u8>, code: u8, value: u32) {
    frame.extend_from_slice(&[code, 4]);
    frame.extend_from_slice(&value.to_be_bytes());
}
