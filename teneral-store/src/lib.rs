//! Teneral's binding store: the source of truth for what the daemon
//! answers with.
//!
//! The store is a crate of its own because its surface is an
//! interface other things link rather than an arrangement internal to
//! the daemon, and because one of its rules is only mechanical across
//! a crate boundary: every operation takes the network it applies to,
//! and a caller that wanted to skip the network would have to add a
//! public method to do it. Nothing in here hands out a database
//! connection, and no SQL leaves the crate.
//!
//! What exists so far is the foundation the rest is built on: a
//! database that opens, migrates itself forward, refuses to open one
//! a newer build wrote, carries an identity of its own, and keeps an
//! event log whose sequence numbers are never issued twice. The
//! resources, the lookup path and the interface they are reached
//! through come next.

mod db;
mod error;
mod identity;
mod schema;

#[cfg(test)]
mod test_support;

pub use db::Database;
pub use error::{Error, Result};
pub use identity::Epoch;
pub use schema::SCHEMA_VERSION;
