//! The interface the store is reached through.
//!
//! Two traits, because a store and a consistent view of one are
//! different things with different lifetimes: [`BindingStore`] is the
//! store, and [`ReadHandle`] is a view of it frozen at a position in
//! its history.
//!
//! Everything about their shape comes from the five demands the API
//! specification makes of the storage layer, in
//! `docs/api-design.md`. They are restated here against the methods
//! that serve them, so that a second implementation can tell a shape
//! that was chosen from one that merely happened:
//!
//! 1. **A total order over mutations, assigned by the store.** Every
//!    mutating method on [`BindingStore`] returns the [`Cursor`] it
//!    was assigned. A write that returned nothing would leave the
//!    layer above to invent an order, and with more than one writer
//!    there is no consistent order to invent.
//! 2. **Reads consistent with a point in that order.**
//!    [`BindingStore::read`] hands back a [`ReadHandle`], which
//!    reports the cursor it reads at and answers every question from
//!    that one position for as long as it is held.
//! 3. **Compare-and-set on identity.** The methods that change or
//!    remove an existing resource take the cursor the caller read it
//!    at and refuse if the resource has moved on. A read followed by
//!    a write would be correct only until something got in between.
//! 4. **The network is a parameter of every method**, not a column
//!    an implementation is trusted to filter on. The exceptions are
//!    enumerated in each trait's doc comment below, because a reader
//!    needs to be able to tell a deliberate one from an oversight.
//! 5. **History retention belongs to the store.** The event every
//!    write emits is already durable, written in the same transaction
//!    as the write it describes; the methods that read that history
//!    back and the bound that truncates it are not on these traits
//!    yet, and the gap is named here rather than left to be
//!    discovered.

use async_trait::async_trait;

use crate::cursor::Cursor;
use crate::error::Result;
use crate::identity::Epoch;
use crate::model::{AddressRange, Attachment, Network};

/// The binding store: everything that changes it, and the two reads
/// that cannot be answered from inside one network.
///
/// Mutating methods return the cursor their write was assigned. That
/// is not a convenience. Each write also appends an event describing
/// its result, in the same transaction, so a write that could not
/// name its cursor would be a write that had emitted no event -- and
/// a store whose history silently misses a change is worse than one
/// that refused the change outright.
///
/// Reads that name a network are on [`ReadHandle`] rather than here,
/// so that the ordinary way to read several things is to read them
/// all at one position. The two reads that remain here answer
/// questions that have no network to be scoped by, and both are in
/// the enumerated list below.
///
/// Implementations are `Send + Sync` and every method takes `&self`:
/// a store is shared, and the serialisation of writes is the
/// implementation's business rather than something a caller arranges
/// with a lock it remembered to take.
///
/// Store-global methods, enumerated so that a reader can tell them
/// from an oversight: `epoch`, `schema_version`, `read`,
/// `interface_owner`.
///
/// - `epoch` and `schema_version` describe the file. An epoch per
///   network would qualify nothing, because the sequence numbers it
///   qualifies are one order over every network together; a schema
///   belongs to a database and not to a network inside it.
/// - `read` opens a view at a position in that same cross-network
///   order, so it is scoped by a cursor rather than by a network.
/// - `interface_owner` is the one method whose answer *is* a network,
///   which is why it cannot take one. It is the resolution step the
///   datapath performs before anything is scoped: a packet arrives on
///   an interface, and this is what turns that into the network whose
///   configuration may be consulted. Note how it differs from the
///   shape demand 4 warns against -- a method that takes an address
///   and lets the implementation scope by network. This one takes no
///   network and returns the one it found, so a caller cannot ask it
///   about a network and be given another's answer.
#[async_trait]
pub trait BindingStore: Send + Sync {
    /// The epoch a consumer's remembered position has to match for
    /// the store to be able to continue it.
    ///
    /// Serves demand 1: it is the half of every cursor that is not a
    /// sequence number.
    fn epoch(&self) -> &Epoch;

    /// The schema version of the database behind this store.
    ///
    /// Not part of any demand. It is here because an operator
    /// diagnosing a refusal to open, or a consumer deciding whether a
    /// feature exists, has no other way to ask.
    fn schema_version(&self) -> u32;

    /// Open a view of the store at a single position in its order.
    ///
    /// Serves demand 2. The handle may be held across the caller's
    /// awaits and answers every question from the position it reports
    /// until it is dropped, so a consumer can read a snapshot of
    /// several resources and then resume a stream at exactly the
    /// cursor the snapshot was taken at, with no gap and no overlap.
    ///
    /// A handle costs the store a thread for its lifetime, so it is
    /// something to open, use and drop rather than to keep by.
    async fn read(&self) -> Result<Box<dyn ReadHandle>>;

    /// Which network, if any, has claimed the interface `ifname` in
    /// the namespace `namespace`.
    ///
    /// Store-global, for the reason in this trait's doc comment: the
    /// answer is a network, so there is no network to ask within.
    /// `None` means no network has claimed it, which is a different
    /// thing from a network having claimed it and configured nothing.
    async fn interface_owner(&self, namespace: &str, ifname: &str) -> Result<Option<Attachment>>;

    /// Create a network called `network`.
    ///
    /// Serves demands 1 and 3. The create is itself a compare-and-set
    /// against the resource not existing: a name already in use is
    /// refused rather than overwritten, and the refusal says which
    /// name.
    async fn create_network(&self, network: &str) -> Result<Cursor>;

    /// Remove the network `network`, which must still be at `expected`.
    ///
    /// Serves demands 1, 3 and 4. A network that still has interfaces
    /// attached or ranges configured is refused rather than emptied,
    /// and the refusal names what is in the way: deleting a network
    /// and silently detaching its interfaces would hand those
    /// interfaces to whichever network claimed them next.
    async fn delete_network(&self, network: &str, expected: &Cursor) -> Result<Cursor>;

    /// Claim the interface `ifname` in `namespace` for `network`.
    ///
    /// Serves demands 1, 3 and 4, and it is the most important
    /// refusal in the store. An interface belongs to exactly one
    /// network; a claim on one another network already holds is
    /// refused by the database itself, and the refusal names the
    /// network holding it. That refusal is where a cross-network leak
    /// would otherwise be *configured* rather than coded, and it is
    /// what lets the datapath treat a packet's arrival interface as a
    /// complete answer to which network the packet belongs to.
    async fn attach_interface(
        &self,
        network: &str,
        namespace: &str,
        ifname: &str,
    ) -> Result<Cursor>;

    /// Release `network`'s claim on `ifname` in `namespace`, which
    /// must still be at `expected`.
    ///
    /// Serves demands 1, 3 and 4. An interface another network holds
    /// is not found rather than detached, because the caller asked
    /// within its own network and is entitled to an answer about its
    /// own network only.
    async fn detach_interface(
        &self,
        network: &str,
        namespace: &str,
        ifname: &str,
        expected: &Cursor,
    ) -> Result<Cursor>;

    /// Give `network` a range called `name` running from `first` to
    /// `last` inclusive.
    ///
    /// Serves demands 1, 3 and 4. Refused if the network already has
    /// a range of that name, or if the span would overlap one of that
    /// network's existing ranges -- two ranges in one network that
    /// both contain an address make "the lowest free address" a
    /// question with two answers. An identical span in a *different*
    /// network is not a conflict and is not refused; two networks
    /// using the same addresses is the case this store exists to get
    /// right.
    async fn create_address_range(
        &self,
        network: &str,
        name: &str,
        first: std::net::Ipv4Addr,
        last: std::net::Ipv4Addr,
    ) -> Result<Cursor>;

    /// Move the ends of `network`'s range `name`, which must still be
    /// at `expected`.
    ///
    /// Serves demands 1, 3 and 4. The compare-and-set is what makes
    /// this safe to offer at all: an operator resizing a pool read it
    /// first, and a resize computed from a stale read can move the
    /// pool over addresses something else was told it owned.
    async fn update_address_range(
        &self,
        network: &str,
        name: &str,
        expected: &Cursor,
        first: std::net::Ipv4Addr,
        last: std::net::Ipv4Addr,
    ) -> Result<Cursor>;

    /// Remove `network`'s range `name`, which must still be at
    /// `expected`.
    ///
    /// Serves demands 1, 3 and 4.
    async fn delete_address_range(
        &self,
        network: &str,
        name: &str,
        expected: &Cursor,
    ) -> Result<Cursor>;
}

/// A view of the store frozen at one position in its order.
///
/// This is demand 2, and it is a handle rather than a set of getters
/// deliberately. Several independent reads of a changing store agree
/// most of the time, which is the worst possible property: a consumer
/// that assembled a snapshot from them would see a network's ranges
/// as they were before a change and its attachments as they were
/// after, and nothing in the result would say so. Every question put
/// to one handle is answered from the position it reports.
///
/// Dropping the handle ends the view. A handle costs the store a
/// thread while it lives, which is the price of being able to hold a
/// consistent view across a caller's awaits at all, and the reason it
/// is not something to keep open between requests.
///
/// Store-global methods, enumerated so that a reader can tell them
/// from an oversight: `cursor`, `networks`.
///
/// - `cursor` is the position this view reads at, which spans every
///   network because the order does.
/// - `networks` lists the networks themselves. Its answer *is* the
///   set of networks, so there is no network it could be scoped by;
///   every other read here names one.
#[async_trait]
pub trait ReadHandle: Send {
    /// The position in the store's order that every answer from this
    /// handle is consistent with.
    ///
    /// Serves demand 2, and it is the half that a consumer resuming a
    /// stream needs: this is the cursor to resume at, and it is
    /// exactly the position the answers below describe.
    fn cursor(&self) -> &Cursor;

    /// Every network in the store, at this handle's position.
    ///
    /// Store-global, for the reason in this trait's doc comment.
    async fn networks(&self) -> Result<Vec<Network>>;

    /// The network `network`, or `None` if there is no such network
    /// at this handle's position.
    async fn network(&self, network: &str) -> Result<Option<Network>>;

    /// Every interface `network` has claimed, at this position.
    ///
    /// Serves demand 4: the listing is scoped by the network asked
    /// for, and every row returned names the network it came from so
    /// that a layer above can check the two agree.
    async fn attachments(&self, network: &str) -> Result<Vec<Attachment>>;

    /// `network`'s claim on `ifname` in `namespace`, or `None`.
    ///
    /// `None` when another network holds the interface, because the
    /// question was asked within one network and an answer from
    /// another one would be the leak this store is built to prevent.
    /// [`BindingStore::interface_owner`] is how a caller asks the
    /// question that has no network.
    async fn attachment(
        &self,
        network: &str,
        namespace: &str,
        ifname: &str,
    ) -> Result<Option<Attachment>>;

    /// Every range `network` owns, lowest address first.
    ///
    /// Ordered by address rather than by name because every consumer
    /// of this list is looking for the range an address falls in, and
    /// an order the store did not fix is one each caller would sort
    /// again.
    async fn address_ranges(&self, network: &str) -> Result<Vec<AddressRange>>;

    /// `network`'s range called `name`, or `None`.
    async fn address_range(&self, network: &str, name: &str) -> Result<Option<AddressRange>>;
}
