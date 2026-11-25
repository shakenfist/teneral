//! Teneral's control plane: the schema, the server over the binding
//! store, and the client a consumer links.
//!
//! The daemon is configured through this interface and through nothing
//! else. There is no configuration file, which is what keeps the
//! binding store the single source of truth, so everything an operator
//! or a consumer can say to teneral is said here.
//!
//! # The shape of it
//!
//! [`proto`] holds the generated types, compiled from
//! `proto/teneral.v1.proto` when this crate is built. That file is the
//! whole of the wire and is worth reading before anything else here:
//! both halves come from it, so a field the server writes and one the
//! client reads cannot drift apart.
//!
//! [`wire`] is the framing the two halves share -- a varint length and
//! then a message, with a bound checked against the length before any
//! body is read. [`server`] is the one listener. [`client`] is what a
//! consumer links. [`authz`] decides which capability a request needs
//! and which network it is scoped by, one exhaustive function each,
//! and [`refusal`] is the vocabulary everything is refused with.
//!
//! The watch stream is in two halves that are not in the module list
//! above because nothing outside this crate links them: one task per
//! daemon reads the store's history and broadcasts it, and every
//! stream filters that one reading for its own scope, because a read
//! of the history is a view and a view is a thread. The join between a
//! consumer's first read and the events that arrive while it is making
//! it is the most delicate thing in the crate, and the argument for
//! the order those two happen in is written where the code is. The
//! bound on how many views are open at once lives beside it, because
//! the stream is what opens them in bursts.
//!
//! [`resource`] holds one converter per resource kind, and it is one
//! rather than two because a resource reaches this crate twice over:
//! typed, from a read, and as the JSON text an event carries. Two
//! spellings of one resource is the defect that arrangement produces,
//! and it stays invisible until a consumer applies a streamed event
//! over a read it was already holding.
//!
//! Server and client are in one crate with no feature gating them
//! apart. A feature matrix is a thing `--all-targets` does not test,
//! and a consumer that links the client and gets the server with it
//! pays compile time and nothing else.
//!
//! # Why this is a library
//!
//! The daemon in this repository is the thin thing: it parses flags,
//! opens the store, serves the socket and shuts down cleanly. The
//! product whose interface is a browser is a separate binary in a
//! separate repository, and what it links is the client here. That is
//! the arrangement worth defending: a consumer that reimplemented the
//! model would hold a second copy of the rules that keep one network's
//! answers out of another's, in the component least likely to be
//! tested with two networks.
//!
//! What discharges part of that risk from here is in the types. No
//! resource can be named without naming its network, and every answer
//! repeats the network it was true of, so a translator that drops a
//! scope produces a mismatch its own tests can see.
//!
//! # What is not here
//!
//! Nothing in this crate decides what an answer is. It asks the store,
//! which takes the network as a parameter of every operation, so a
//! handler cannot answer from anything the store would not have
//! scoped. The one thing this layer owns that the store does not is
//! the question of who may ask: the socket's mode, the peer the kernel
//! reports, the credential presented once per connection, and the
//! capability each request needs.

pub mod authz;
pub mod client;
pub mod error;
mod operations;
pub mod proto;
pub mod refusal;
pub mod resource;
pub mod server;
#[cfg(test)]
mod testing;
mod views;
mod watch;
pub mod wire;

pub use authz::{Authenticated, named_network, required_capability};
pub use error::{Error, Result};
pub use server::{Listener, ServerConfig, owning_uid};
pub use wire::MAX_MESSAGE_BYTES;

/// The version of the protocol this build speaks.
///
/// One number for the whole wire rather than one per message. A
/// connection agrees on it at the handshake or is refused naming both
/// sides, which is what makes a mismatch a sentence an operator can
/// read instead of a decode failure halfway through a stream.
///
/// It moves when a change would make an older client wrong -- a field
/// whose meaning changes, an operation whose refusals change -- and
/// not when one is added, because an added field is ignored by a
/// client that does not know it and an added operation is refused as
/// unknown by a daemon that does not serve it.
pub const PROTOCOL_VERSION: u32 = 1;

/// Every version this build serves, for the refusal that has to name
/// them.
///
/// A list because the refusal names what the daemon speaks rather than
/// a single number a client would have to guess around; there is one
/// entry today, and the shape is what makes serving two a change to
/// this constant rather than to the refusal.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[u32] = &[PROTOCOL_VERSION];
