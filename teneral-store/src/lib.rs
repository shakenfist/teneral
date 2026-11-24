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
//! [`BindingStore`] is the interface, [`ReadHandle`] is a view of the
//! store held at one position in its history, and [`SqliteStore`] is
//! the implementation that exists today. The shape of all three comes
//! from what the API specification asks of a storage layer, and the
//! traits' own documentation says which of those demands each method
//! group serves.
//!
//! The store's interface is asynchronous, and the blocking it does is
//! confined to threads the store itself owns rather than handed to a
//! runtime's shared blocking pool. A caller that wrapped one of these
//! methods in `tokio::task::spawn_blocking` would have misread the
//! interface: there is nothing left for such a helper to move. Nor
//! could this crate make that call itself. It depends on tokio for
//! channels alone, and `spawn_blocking` does not exist without a
//! runtime feature, so the dependency's own feature list is what
//! holds the rule rather than anybody's vigilance.
//!
//! What exists so far is the foundation and the configured resources
//! a lookup is scoped by: a database that opens, migrates itself
//! forward, refuses to open one a newer build wrote, carries an
//! identity of its own, keeps an event log whose sequence numbers are
//! never issued twice, and holds networks, the interfaces attached to
//! them and the address ranges they own. Bindings, leases and the
//! reads over the history come next.

mod clock;
mod cursor;
mod db;
mod error;
mod identity;
mod model;
mod ops;
mod schema;
mod sqlite;
mod store;

#[cfg(test)]
mod behaviour;
#[cfg(test)]
mod test_support;

pub use clock::{Clock, SystemClock};
pub use cursor::Cursor;
pub use error::{Error, Result};
pub use identity::Epoch;
pub use model::{AddressRange, Attachment, Network};
pub use schema::SCHEMA_VERSION;
pub use sqlite::{SnapshotView, SqliteStore};
pub use store::{BindingStore, ReadHandle};
