//! A request and the interface it arrived on, against the binding
//! store.
//!
//! This is the layer that decides things. Which network a packet
//! belongs to, which client is asking, whether that client may have
//! the address it named, and whether the answer is an offer, an
//! acknowledgement, a refusal or a silence -- all of it is settled
//! here, above the sockets, because `make test` cannot open an
//! `AF_PACKET` socket and has one interface. Anything decided below
//! this line is decided where this repository's tests cannot see it,
//! so nothing is.
//!
//! # What this layer does not implement
//!
//! Address selection, pools and reservations. The store already
//! chooses an address for a client that has none, in the order RFC
//! 2131 section 4.3.1 specifies, honouring an address the client asked
//! for only at its place in that order, refusing one a binding
//! reserves for somebody else and bringing back one the client held
//! until its lease lapsed. It also already matches a client's
//! attributes against the bindings configured in a network, in a
//! published precedence order, and reports which selector matched.
//!
//! So this layer assembles what the packet presents and asks. A second
//! implementation of any of that would be the thing this project is
//! shaped to avoid: the two would agree in every test and diverge in
//! somebody's guest.
//!
//! # The network comes from the interface and from nothing else
//!
//! Two networks may legitimately hold the same address, which is the
//! property the whole project exists for. The only thing that tells
//! them apart is the interface a packet arrived on, so every question
//! this layer asks the store names the network that interface resolved
//! to, and the lookup it uses for a reservation
//! ([`teneral_store::ReadHandle::lookup_binding_on_interface`]) is the
//! one that refuses -- rather than answers -- when the network it was
//! given and the network the interface belongs to disagree.
//!
//! Nothing here is keyed by an address or a hardware address on its
//! own. There is no cache between packets at all, with one exception
//! that is not an answer: the record of which relays have already been
//! reported, which exists so that a misconfigured relay costs one log
//! line rather than one per packet, and which is keyed by the
//! interface as well as by the relay.
//!
//! # Answers, silences, and the things a daemon should say
//!
//! [`Decider::answer`] returns a [`Decided`], which says what went to
//! the client -- or why nothing did -- and carries the [`Note`]s the
//! daemon logs. The notes are values rather than lines written to
//! standard error, for two reasons. A library that wrote to a stream
//! would be a library whose log destination its consumer could not
//! choose; and "a relayed request is dropped with exactly one line
//! naming the relay" is a property, so it has to be something a test
//! can count.
//!
//! # The seam below this layer
//!
//! [`Egress`] is one method: put these bytes on this interface,
//! addressed to this client. Everything the real implementation has to
//! decide -- an `AF_PACKET` frame for a client with no address, UDP
//! with a named egress for one that has, the broadcast flag read as a
//! request for broadcast and never as permission to unicast -- is
//! decidable from the [`Reply`] it is handed. So this layer is tested
//! against a recorder, and the socket layer is a single
//! implementation of one method.

mod decider;
mod egress;
mod error;
mod identity;
mod outcome;

#[cfg(test)]
mod behaviour;
#[cfg(test)]
mod test_support;

pub use decider::{Arrival, Decider};
pub use egress::{Egress, Reply};
pub use error::{Error, Result};
pub use outcome::{Decided, Note, Outcome, Silence};

/// The actor every write from this front-end is recorded against.
///
/// `Actor::Daemon` takes a literal rather than a runtime string
/// because the parts of the daemon are known when it is built, and an
/// actor assembled from configuration would be an attribution an
/// operator could choose. This is the one the store's own
/// documentation already anticipated as "a lease the datapath
/// recorded".
pub(crate) const ACTOR: teneral_store::Actor = teneral_store::Actor::Daemon("dhcpv4");
