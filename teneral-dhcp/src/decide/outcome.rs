//! What happened to a request, and what a daemon should say about it.
//!
//! Two values rather than one, because they answer different
//! questions. The [`Outcome`] is what the client got, which is what a
//! test asserts and what a counter would count. The [`Note`]s are what
//! an operator reads, and most requests produce none at all.
//!
//! The notes are values and not lines written to a stream. A library
//! that wrote to standard error would be one whose consumer could not
//! choose where its output went -- and, more to the point here,
//! "exactly one line names the relay" is a property, so it has to be
//! something a test can count rather than something it would have to
//! capture a stream to see.
//!
//! Which faults earn a note is a judgement and it is made once, here:
//! a note is for something an operator could act on, or something
//! they would want to know had happened. A request that selects
//! another server on the same link is neither -- it is the protocol
//! working -- so it is an outcome with no note. A client declining an
//! address this server just handed it is the clearest evidence a
//! server ever gets that its belief about a network is wrong, so it
//! always has one.

use std::fmt;
use std::net::Ipv4Addr;

use teneral_store::SelectorKind;

use crate::wire::AnswerKind;

/// What one request came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decided {
    /// What the client got, or why it got nothing.
    pub outcome: Outcome,
    /// What the daemon should log, in the order it happened. Empty for
    /// an ordinary request.
    pub notes: Vec<Note>,
}

/// What the client got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// An answer of this kind was handed to the egress.
    Sent(AnswerKind),
    /// A lease was given up and the protocol asks for no reply, so
    /// nothing was sent.
    Revoked {
        /// The address that went back to the network it was leased in.
        address: Ipv4Addr,
    },
    /// Nothing was sent, for this reason.
    Silent(Silence),
}

/// Why a request was not answered.
///
/// A silence is a decision like any other, so each of these is a
/// reason and not a failure. Two of them -- a release and a decline --
/// are what the protocol asks for; the rest are a request this
/// front-end does not serve, a configuration fault, or a network with
/// nothing to offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Silence {
    /// The request came through a relay, and a relayed request is not
    /// answered.
    ///
    /// Answering one correctly means replying to the relay and taking
    /// the subnet from `giaddr` rather than from the arrival
    /// interface, which is a subnet model this front-end does not
    /// have. Answering it incorrectly means a lease in the wrong
    /// subnet, which is worse than not answering.
    Relayed {
        /// The relay the request came through.
        relay: Ipv4Addr,
    },
    /// No network has claimed the interface the request arrived on.
    ///
    /// Everything an answer carries is derived from the network that
    /// claimed the interface, so there is nothing to answer from. The
    /// fix is an operator's.
    InterfaceNotClaimed,
    /// The client presented nothing this store could be asked about.
    ///
    /// Not the same as a client presenting nothing at all, which the
    /// parser already refuses: this is a client whose attributes exist
    /// and are not values a selector can hold.
    ClientNotIdentifiable,
    /// The request named a different server, so it is not this
    /// server's to answer.
    AnotherServerSelected {
        /// The server the client believes it is talking to.
        server: Ipv4Addr,
    },
    /// The network had no address to offer this client.
    NothingToOffer,
    /// A client checking after a reboot asked about an address, and
    /// this network has no record of that client at all.
    ///
    /// The one silence here that a specification requires rather than
    /// this project choosing. A REQUEST with no server identifier and
    /// no `ciaddr`, naming the address the client remembers, is a
    /// client rebooting and asking whether what it remembers is still
    /// its own. RFC 2131 section 4.3.2 is normative: where the server
    /// has no record of the client it MUST remain silent, and the
    /// section gives its own reason -- the peaceful coexistence of
    /// non-communicating servers on one wire.
    ///
    /// What that MUST prevents is not pedantic. Two servers on one
    /// bridge is what a migration looks like, and a server that
    /// refused every other server's clients as they rebooted would
    /// break the network it had just been added to. So the division
    /// is between having no record of the client, which is this, and
    /// having a record that disagrees with what the client believes,
    /// which is a refusal.
    ///
    /// No line goes with it. The section permits a warning to the
    /// administrator and does not require one, and a line per reboot
    /// of every client belonging to every other server on the link is
    /// the noise that buries a fault.
    NoRecordOfThisClient,
    /// The address the client gave up is not one it holds in this
    /// network.
    ///
    /// The ordinary cause is a client that holds the same address in a
    /// different network and released it on the wrong interface, which
    /// is exactly the confusion this project exists to not make.
    NotThisClientsAddress {
        /// The address the client named.
        address: Ipv4Addr,
    },
}

/// Something worth a line in the daemon's log.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Note {
    /// A relayed request was dropped.
    ///
    /// Emitted once per relay on an interface rather than once per
    /// packet: a relay pointed at this server sends every request its
    /// clients make, and a line each would bury everything else in
    /// the log.
    RelayedRequestDropped {
        /// The relay, which is the whole of what makes this
        /// actionable.
        relay: Ipv4Addr,
    },
    /// A request arrived on an interface no network has claimed.
    InterfaceNotClaimed {
        /// The namespace the interface is named within.
        namespace: String,
        /// The interface, which is what an operator attaches.
        ifname: String,
    },
    /// An attribute the client presented is not one the store would
    /// hold as a selector value, so it was not presented to the
    /// lookup.
    ///
    /// The same ruling the parser makes about an option whose value
    /// will not decode: the client loses the attribute and keeps its
    /// lease. A binding configured against that attribute will not
    /// match, which is why the fact is logged rather than swallowed.
    AttributeNotPresented {
        /// Which attribute.
        kind: SelectorKind,
        /// Why the store would not take it.
        detail: String,
    },
    /// The client presented no attribute the store would hold, so
    /// there was nothing to ask about it.
    ClientNotIdentifiable,
    /// An option arrived framed correctly with a value the decoder
    /// would not read.
    ///
    /// Carried up from the parser, which drops the option and answers
    /// anyway. The line is the difference between "the client did not
    /// send this" and "the client sent this and we could not read it".
    OptionNotUnderstood {
        /// The option code whose value was dropped.
        code: u8,
    },
    /// A network could not offer this client an address.
    NothingToOffer {
        /// The network that was asked.
        network: String,
        /// What the store said, which separates a network with no pool
        /// from a pool that is full.
        detail: String,
    },
    /// An address a client asked for was refused, and the client was
    /// told so.
    AddressRefused {
        /// The network the request was decided in.
        network: String,
        /// The address the client asked for.
        address: Ipv4Addr,
        /// Why it may not have it.
        detail: String,
    },
    /// A client reported that an address this server handed out is
    /// already in use by something else.
    ///
    /// Always logged. A decline is the clearest evidence a server ever
    /// gets that its belief about a network is wrong, and this phase's
    /// response to one -- revoke the lease, so the address goes back
    /// to the pool and may be offered again -- loops if the conflict
    /// is permanent. The line is how an operator finds out.
    AddressDeclined {
        /// The network the address was leased in.
        network: String,
        /// The address the client says is in use.
        address: Ipv4Addr,
    },
    /// A client gave up an address it does not hold in this network.
    AddressNotHeldHere {
        /// The network the interface belongs to.
        network: String,
        /// The address the client named.
        address: Ipv4Addr,
    },
}

impl fmt::Display for Note {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Note::RelayedRequestDropped { relay } => write!(
                formatter,
                "a request relayed by {relay} was dropped: a relayed request is not answered"
            ),
            Note::InterfaceNotClaimed { namespace, ifname } => write!(
                formatter,
                "a request arrived on {ifname} in the namespace {namespace}, which no network has claimed"
            ),
            Note::AttributeNotPresented { kind, detail } => write!(
                formatter,
                "a client's {kind} was not presented to the lookup: {detail}"
            ),
            Note::ClientNotIdentifiable => write!(
                formatter,
                "a client presented nothing that can be a selector value, so it was not answered"
            ),
            Note::OptionNotUnderstood { code } => write!(
                formatter,
                "option {code} arrived with a value that would not decode and was dropped"
            ),
            Note::NothingToOffer { network, detail } => write!(
                formatter,
                "the network {network} had no address for a client: {detail}"
            ),
            Note::AddressRefused {
                network,
                address,
                detail,
            } => write!(
                formatter,
                "{address} was refused to a client in the network {network}: {detail}"
            ),
            Note::AddressDeclined { network, address } => write!(
                formatter,
                "a client declined {address} in the network {network} as already in use, and the \
                 lease was revoked -- the address may be offered again"
            ),
            Note::AddressNotHeldHere { network, address } => write!(
                formatter,
                "a client gave up {address}, which it does not hold in the network {network}"
            ),
        }
    }
}
