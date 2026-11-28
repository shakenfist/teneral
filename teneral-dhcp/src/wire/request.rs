//! What arrived, once it is safe to look at.
//!
//! The order the checks below run in is the whole of this file's
//! argument, so it is written out at each step rather than left to be
//! reconstructed. In short: the size, then `hlen`, then the cookie,
//! then the framing of the options area, and only then the decoder.
//! Every one of those is a refusal a client can provoke, and none of
//! them needs a parse to have happened first.
//!
//! # Two kinds of thing wrong with an options area
//!
//! The walk over the records is what makes them distinguishable, and
//! they are not the same kind of fault.
//!
//! A **framing** error -- a record whose length runs past the end of
//! the datagram, or a code with no length byte after it -- means the
//! boundaries of the option stream cannot be trusted. Nothing after
//! the break can be located, so the datagram is refused.
//!
//! A **value** the decoder declines is not that. The record's
//! boundaries are known, so it can be dropped and everything around
//! it read. It costs the option and not the answer: a Windows machine
//! whose host name is in a local codepage sends a host name that is
//! not UTF-8, and what that should cost it is the name, not its
//! lease. The codes dropped that way are recorded on the request, so
//! a caller that wanted one can say it was unreadable rather than
//! absent.
//!
//! Nothing teneral needs is lost silently, because every option it
//! acts on is asked for by name: a message type that will not decode
//! is `NoMessageType` and the datagram is refused, and a server
//! identifier that will not decode is a server identifier the layer
//! above does not find.

use std::net::Ipv4Addr;

use dhcproto::v4::{DhcpOption, Message, MessageType, OptionCode};
use dhcproto::{Decodable, Decoder};

use crate::wire::error::{Error, Result};
use crate::wire::{
    BOOT_REQUEST, CHADDR_BYTES, COOKIE_OFFSET, FIXED_BYTES, HLEN_OFFSET, MAGIC_COOKIE,
    MAX_DATAGRAM_BYTES, OPTION_END, OPTION_PAD,
};

/// Which of a client's messages this is.
///
/// The five a client sends to a server, and no others. A message type
/// outside this set is refused by [`parse`] rather than carried as an
/// unknown, because there is nothing above this layer that could
/// decide what to do with one.
///
/// This enum rather than the decoder's own is what keeps that
/// dependency inside this module: a caller matching on it would be a
/// caller a version bump could break, for a type with sixteen
/// variants of which five are ever read here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
    /// "Is there a server, and what would it give me?"
    Discover,
    /// "I am taking this address" -- either accepting an offer or
    /// renewing what the client already holds. Which of the two it is
    /// is told apart by the server identifier option, which this
    /// layer carries and does not interpret.
    Request,
    /// "The address you gave me is already in use by something else."
    Decline,
    /// "I am finished with this address."
    Release,
    /// "I have an address already; tell me the rest of the
    /// configuration."
    Inform,
}

/// A client's message, as much of it as anything above this layer
/// needs.
///
/// The fields are public because this is a record of what arrived.
/// What [`parse`] established about each of them is in its
/// documentation, and those are the invariants the layers above are
/// entitled to rely on -- in particular that `hardware_address` is
/// between one and sixteen octets and is not all zero, which is why
/// nothing above here has to think about `hlen` again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// Which message it is.
    pub kind: RequestKind,
    /// The transaction the client opened, which its reply must echo.
    pub transaction: u32,
    /// Whether the client asked for a broadcast reply.
    ///
    /// Read as a request for broadcast and never as permission to
    /// unicast: a client that cleared the flag was observed accepting
    /// a broadcast reply in every transaction measured, so the clear
    /// flag says nothing about what it will accept.
    pub broadcast: bool,
    /// `htype`, the kind of hardware address below, echoed into the
    /// reply.
    pub hardware_type: u8,
    /// `chaddr`, truncated to the `hlen` octets the client declared.
    ///
    /// Between one and sixteen octets, and not entirely zero. Both
    /// are established before the accessor that produces it is called
    /// at all.
    pub hardware_address: Vec<u8>,
    /// `ciaddr`: the address the client already holds, or the
    /// unspecified address if it holds none.
    pub client_address: Ipv4Addr,
    /// `giaddr`: the relay the message came through, or the
    /// unspecified address if it came direct.
    ///
    /// Carried rather than acted on. A relayed request is not
    /// answered, and the layer that decides that logs it; refusing it
    /// here would lose the address the log line needs.
    pub relay_address: Ipv4Addr,
    /// Option 50: an address the client asked for by name.
    pub requested_address: Option<Ipv4Addr>,
    /// Option 54: the server the client believes it is talking to.
    ///
    /// Its presence is what tells a REQUEST that accepts an offer
    /// from a REQUEST that renews a lease, which is a distinction the
    /// layer above makes.
    pub server_identifier: Option<Ipv4Addr>,
    /// Option 61: the identifier the client chose for itself, which
    /// outranks its hardware address when identifying it.
    pub client_identifier: Option<Vec<u8>>,
    /// Option 77: the class the client's own operator configured it
    /// with, naming a role that many clients may share.
    ///
    /// Bytes rather than text, as the option is on the wire. There is
    /// no canonical written form for it, so deciding what an operator
    /// would have typed is a choice the layer above makes, and this
    /// field is what arrived rather than a reading of it.
    pub user_class: Option<Vec<u8>>,
    /// Option 60: the class the client's software reports, naming a
    /// product or a firmware family.
    ///
    /// Bytes, for the reason above.
    pub vendor_class: Option<Vec<u8>>,
    /// Option 12: the name the client asked to be known by.
    pub hostname: Option<String>,
    /// The options the datagram framed and the decoder would not read.
    ///
    /// Empty for every well-formed request. A code in here is the
    /// difference between "the client did not send this" and "the
    /// client sent this and we could not read it", which is a
    /// distinction worth a log line and is not one a caller could
    /// recover any other way.
    pub not_understood: Vec<OptionNotUnderstood>,
}

/// An option that was framed correctly and whose value the decoder
/// declined.
///
/// A record on the request rather than a refusal of it. The option's
/// own boundaries are known, so it can be dropped without losing the
/// options around it -- and the alternative, refusing the datagram,
/// would mean a client with an unreadable host name got no address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OptionNotUnderstood {
    /// The option code whose value would not decode.
    pub code: u8,
}

/// Read a datagram.
///
/// The checks run in the order below, and the order is the point:
///
/// 1. the length, because parsing is the cost an attacker is trying
///    to impose and because every later step reads a field that must
///    be there;
/// 2. `hlen`, which is the third byte of the message and so needs no
///    parse -- this is the only order in which the guard can precede
///    the accessor it exists for;
/// 3. the magic cookie, which the decoder does not check, so that a
///    BOOTP message is refused by name rather than read as DHCP;
/// 4. the framing of the options area, because the decoder stops at
///    the first option it cannot read and reports success;
/// 5. the decoder, with any option whose value it declines dropped
///    and recorded, and then the typed options.
pub fn parse(datagram: &[u8]) -> Result<Request> {
    let length = datagram.len();
    if length > MAX_DATAGRAM_BYTES {
        return Err(Error::TooLarge { length });
    }

    // `hlen` before anything else reads a field, and read out of the
    // buffer rather than out of a parsed message. A datagram too short
    // to hold three bytes fails here as a truncation, which it is.
    let hlen = *datagram
        .get(HLEN_OFFSET)
        .ok_or(Error::TooShort { length })?;
    if hlen == 0 {
        return Err(Error::NoHardwareLength);
    }
    if usize::from(hlen) > CHADDR_BYTES {
        return Err(Error::HardwareLengthOverflowsField { hlen });
    }

    // The fixed part and the options area, split without indexing so
    // that a truncation anywhere in the header is one refusal.
    let (fixed, options) = datagram
        .split_at_checked(FIXED_BYTES)
        .ok_or(Error::TooShort { length })?;
    let cookie: [u8; 4] = fixed
        .get(COOKIE_OFFSET..)
        .and_then(|tail| tail.try_into().ok())
        .ok_or(Error::TooShort { length })?;
    if cookie != MAGIC_COOKIE {
        return Err(Error::NotTheMagicCookie { found: cookie });
    }

    let framed = framed_options(options)?;
    let (message, not_understood) = decode_dropping_unreadable(datagram, &framed)?;

    let opcode = u8::from(message.opcode());
    if opcode != BOOT_REQUEST {
        return Err(Error::NotABootRequest { opcode });
    }

    // Safe here and nowhere earlier: the guard above is the whole of
    // what makes this accessor total in the version of the decoder
    // this project resolves.
    let hardware_address = message.chaddr().to_vec();
    if hardware_address.iter().all(|octet| *octet == 0) {
        return Err(Error::HardwareAddressUnset);
    }

    let kind = match message.opts().msg_type().ok_or(Error::NoMessageType)? {
        MessageType::Discover => RequestKind::Discover,
        MessageType::Request => RequestKind::Request,
        MessageType::Decline => RequestKind::Decline,
        MessageType::Release => RequestKind::Release,
        MessageType::Inform => RequestKind::Inform,
        other => {
            return Err(Error::NotAClientMessage {
                message_type: u8::from(other),
            });
        }
    };

    Ok(Request {
        kind,
        transaction: message.xid(),
        broadcast: message.flags().broadcast(),
        hardware_type: u8::from(message.htype()),
        hardware_address,
        client_address: message.ciaddr(),
        relay_address: message.giaddr(),
        requested_address: match message.opts().get(OptionCode::RequestedIpAddress) {
            Some(DhcpOption::RequestedIpAddress(address)) => Some(*address),
            _ => None,
        },
        server_identifier: match message.opts().get(OptionCode::ServerIdentifier) {
            Some(DhcpOption::ServerIdentifier(address)) => Some(*address),
            _ => None,
        },
        client_identifier: match message.opts().get(OptionCode::ClientIdentifier) {
            Some(DhcpOption::ClientIdentifier(identifier)) => Some(identifier.clone()),
            _ => None,
        },
        user_class: match message.opts().get(OptionCode::UserClass) {
            Some(DhcpOption::UserClass(class)) => Some(class.clone()),
            _ => None,
        },
        vendor_class: match message.opts().get(OptionCode::ClassIdentifier) {
            Some(DhcpOption::ClassIdentifier(class)) => Some(class.clone()),
            _ => None,
        },
        hostname: match message.opts().get(OptionCode::Hostname) {
            Some(DhcpOption::Hostname(name)) => Some(name.clone()),
            _ => None,
        },
        not_understood,
    })
}

/// The most options that may be dropped before the datagram is
/// refused instead.
///
/// See `decode_dropping_unreadable` for why there is a cap at all and
/// why it is this number.
const MOST_OPTIONS_DROPPED: usize = 4;

/// One option record, as the datagram framed it.
///
/// The length byte is kept rather than recomputed from the value, so
/// that rebuilding the area below involves no arithmetic on a length
/// at all.
struct FramedOption {
    code: u8,
    length: u8,
    value: Vec<u8>,
}

/// Walk the options area and return the records it frames, in order.
///
/// This exists because the decoder is lenient in a way a server
/// cannot be. Its options loop stops at the first record it cannot
/// read and returns the message as though that record and everything
/// after it were never sent, so a datagram whose third option declares
/// a length past the end of the buffer decodes successfully and
/// quietly short. A server answering from that is answering a request
/// it did not read -- it might, for instance, allocate from the pool
/// because it never saw the requested address option.
///
/// Walking first is what makes the two kinds of fault
/// distinguishable. A framing error is refused here, because the
/// records after the break cannot be located. A value the decoder
/// declines is dropped in `decode_dropping_unreadable` below, because
/// this walk has already located everything around it.
///
/// No value is interpreted here, and that is deliberate: the records
/// this returns are the datagram's own framing and not an opinion
/// about it.
///
/// The end of the buffer ends the options. RFC 2131 asks for an end
/// option and every client measured sends one, but a datagram that
/// simply stops is unambiguous, and refusing one would refuse a client
/// over a byte nothing reads.
fn framed_options(mut area: &[u8]) -> Result<Vec<FramedOption>> {
    let mut records = Vec::new();
    loop {
        let Some((code, tail)) = area.split_first() else {
            return Ok(records);
        };
        match *code {
            OPTION_END => return Ok(records),
            OPTION_PAD => area = tail,
            code => {
                let (declared, tail) = tail
                    .split_first()
                    .ok_or(Error::OptionLengthMissing { code })?;
                let length = *declared;
                let (value, rest) = tail.split_at_checked(usize::from(length)).ok_or(
                    Error::OptionRunsPastTheEnd {
                        code,
                        declared: usize::from(length),
                        remaining: tail.len(),
                    },
                )?;
                records.push(FramedOption {
                    code,
                    length,
                    value: value.to_vec(),
                });
                area = rest;
            }
        }
    }
}

/// Decode the datagram, dropping any option whose value the decoder
/// declines, and say which codes were dropped.
///
/// Two facts about the decoder shape this, and both were checked
/// against its source rather than assumed.
///
/// An option code the crate has no type for is **not** dropped: it
/// comes back as an unknown variant carrying the code and the raw
/// bytes, keyed by that code. So nothing here fires on a vendor or
/// private-use option, only on a value that was offered to a type and
/// refused.
///
/// A value that *is* refused, though, takes every option after it with
/// it: the options loop ends at the first error and reports success
/// with what it had. That is why this re-decodes rather than filtering
/// the result. The offending records are removed from the options area
/// and the whole datagram is decoded again, which keeps the decoder's
/// own behaviour -- the concatenation rule for a value split across
/// several records, in particular -- rather than reimplementing a
/// record at a time. Each pass identifies exactly one code, because
/// the first framed code missing from the result is the one the loop
/// stopped at.
///
/// The passes are capped, and the cap is the point rather than tidiness.
/// A pass decodes the whole datagram again, so without a bound the work
/// one packet costs is a function of what its sender chose to put in it:
/// a full datagram of two-byte records whose values are all declined
/// would buy dozens of decodes for one packet, on the one thread that
/// serves every network in this process. The cap makes that a constant.
/// Four, because a client that offers one or two options this decoder
/// declines is the ordinary case this whole division exists for -- a
/// host name in a local codepage -- and one offering five is not a
/// client whose lease anybody is protecting.
fn decode_dropping_unreadable(
    datagram: &[u8],
    framed: &[FramedOption],
) -> Result<(Message, Vec<OptionNotUnderstood>)> {
    let mut buffer = datagram.to_vec();
    let mut not_understood: Vec<OptionNotUnderstood> = Vec::new();

    loop {
        let message =
            Message::decode(&mut Decoder::new(&buffer)).map_err(|error| Error::Malformed {
                detail: error.to_string(),
            })?;
        let understood: Vec<u8> = message
            .opts()
            .iter()
            .map(|(code, _)| u8::from(*code))
            .collect();

        let dropped = framed
            .iter()
            .map(|record| record.code)
            .filter(|code| !not_understood.iter().any(|option| option.code == *code))
            .find(|code| !understood.contains(code));

        match dropped {
            None => return Ok((message, not_understood)),
            Some(code) => {
                not_understood.push(OptionNotUnderstood { code });
                if not_understood.len() > MOST_OPTIONS_DROPPED {
                    return Err(Error::TooManyUnreadableOptions {
                        limit: MOST_OPTIONS_DROPPED,
                    });
                }
                buffer = rebuilt_without(&buffer, framed, &not_understood);
            }
        }
    }
}

/// The datagram again, with the named options left out of its options
/// area.
///
/// Rebuilt from the records the walk found rather than edited in
/// place, so there is no offset to get wrong. The fixed header is
/// copied as it stands; padding is not carried over, carrying no
/// meaning.
fn rebuilt_without(
    datagram: &[u8],
    framed: &[FramedOption],
    without: &[OptionNotUnderstood],
) -> Vec<u8> {
    let mut rebuilt: Vec<u8> = datagram.iter().take(FIXED_BYTES).copied().collect();
    for record in framed {
        if without.iter().any(|option| option.code == record.code) {
            continue;
        }
        rebuilt.extend_from_slice(&[record.code, record.length]);
        rebuilt.extend_from_slice(&record.value);
    }
    rebuilt.push(OPTION_END);
    rebuilt
}
