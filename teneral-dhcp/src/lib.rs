//! Teneral's DHCPv4 front-end.
//!
//! A front-end is a translation between a wire protocol and a
//! question asked of the binding store, and it holds no per-network
//! state of its own. That is not a style rule: the store is the single
//! source of truth for what a client is answered with, and a
//! front-end that remembered a network's mask or its lease time would
//! be a second place to look when a client got an answer nobody
//! expected.
//!
//! The crate is in three layers, and the seams between them are
//! chosen so that the part the repository's tests cannot reach is as
//! small as it can be made:
//!
//! - [`wire`] is bytes to a parsed request, and a set of answer
//!   parameters back to bytes. No store, no socket, no clock. Every
//!   malformed-packet test and every option-encoding test lives here.
//! - the decision layer is a parsed request plus the interface it
//!   arrived on, asked of the store, producing an answer or a
//!   silence. It is where a network is resolved from an interface and
//!   where a lease is written.
//! - the datapath is the sockets, and the frame that reaches a client
//!   with no address.
//!
//! Only the first of those exists so far.
//!
//! The reason for that shape is that `make test` runs as an
//! unprivileged uid with no capabilities, so it can bind neither port
//! 67 nor an `AF_PACKET` socket. Anything decided inside the datapath
//! is decided where this repository's tests cannot see it, so every
//! judgement is pushed above that line and what is left untested is
//! "the bytes went out of the right fd", which the lab measures with
//! a real client instead.
//!
//! The layers are modules rather than a flattened set of re-exports
//! because which layer a type belongs to is part of the design: a
//! type that moved from `wire` to the decision layer would be a type
//! that had acquired a dependency on the store, and that is worth
//! reading as a change at the call site.

pub mod wire;
