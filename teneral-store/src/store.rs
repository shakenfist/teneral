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
//!    The two the specification names by hand are both here and both
//!    are one method: [`BindingStore::create_binding`] refuses an
//!    address a lease holds, and
//!    [`BindingStore::create_binding_revoking_lease`] takes the lease
//!    and writes the binding in one transaction. Note that two of
//!    these methods compare an identity rather than a position --
//!    [`BindingStore::renew_lease`] compares the client, and
//!    [`BindingStore::create_binding_revoking_lease`] compares the
//!    lease id -- because those are the tokens their callers have.
//!    Each says so where it is declared.
//! 4. **The network is a parameter of every method**, not a column
//!    an implementation is trusted to filter on. The exceptions are
//!    enumerated in each trait's doc comment below, because a reader
//!    needs to be able to tell a deliberate one from an oversight.
//! 5. **History retention belongs to the store.** The event every
//!    write emits is durable, written in the same transaction as the
//!    write it describes -- and so is the truncation that keeps the
//!    history finite: the bound is applied inside the write that
//!    triggered it, and the low-water mark it moves is a row in the
//!    database rather than a counter in memory.
//!    [`ReadHandle::history`] is where that history is read, and a
//!    position that has fallen outside the window is told so rather
//!    than handed an empty answer, which is the whole reason the mark
//!    is kept. A ring buffer in the layer above would lose its history
//!    at every restart and so turn every restart into a resync for
//!    every consumer, which is the resume path wasted entirely.

use async_trait::async_trait;
use tokio::sync::watch;

use crate::actor::Actor;
use crate::cursor::Cursor;
use crate::error::Result;
use crate::history::{Digest, History, Snapshot};
use crate::identity::{Epoch, StoreIdentity};
use crate::model::{AddressRange, Attachment, Binding, Explanation, Lease, Network};
use crate::selector::{ClientIdentity, Selector};

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
/// **Every mutating method takes the actor that is making the write**,
/// and the store stamps it on the event and on the row in the same
/// statements that write them. `docs/api-design.md` asks that every
/// write record who made it, and the log alone cannot answer that
/// question for long: the retained history is bounded, so the event
/// naming who last moved a binding is evicted while the binding
/// itself stays. Both are therefore kept, and the two answer
/// different questions -- the log says who made a write that has
/// since been superseded, and the row says who made the write that
/// produced the state something is answering with now.
///
/// The actor is a parameter rather than something a store is
/// configured with, because one store serves every connection: an
/// actor held on the store would be whichever request set it last.
/// [`BindingStore::mint_epoch`] appends no event and writes no
/// resource row, so it stamps the identity row instead, and
/// [`BindingStore::identity`] is what reads that back -- a column
/// nothing selects answers nothing.
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
/// `interface_owner`, `cross_network_answers`, `live_views`,
/// `subscribe`, `identity`, `mint_epoch`.
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
/// - `cross_network_answers` counts the answers this store refused
///   for naming the wrong network. The quantity belongs to the store
///   rather than to a network in it, because each reading *is* a
///   disagreement between two networks and attributing it to one of
///   them would hide which pair it was. The error names both, which
///   is where that detail belongs.
/// - `live_views` counts the views held of the whole store. A view is
///   scoped by a position rather than by a network, so the count of
///   them cannot be attributed to one either.
/// - `subscribe` reports where the one order has reached, and that
///   order spans every network. A per-network signal would be a
///   signal about a cursor whose other half is not per-network.
/// - `mint_epoch` replaces the half of every cursor that qualifies
///   every network's positions at once, so there is no network whose
///   epoch it could be asked to replace, and `identity` reads back
///   what that mint recorded.
#[async_trait]
pub trait BindingStore: Send + Sync {
    /// The epoch a consumer's remembered position has to match for
    /// the store to be able to continue it.
    ///
    /// Serves demand 1: it is the half of every cursor that is not a
    /// sequence number.
    ///
    /// A value rather than a borrow, because
    /// [`BindingStore::mint_epoch`] replaces it. A borrow would be a
    /// borrow of state another caller can change, and an
    /// implementation keeping it behind a lock could not hand one out
    /// at all. What this answers with is the epoch as it stood when
    /// it was asked, which is the whole of what a comparison needs.
    fn epoch(&self) -> Epoch;

    /// What the store says about itself: the epoch it issues
    /// positions in, and who replaced that epoch when.
    ///
    /// Not part of any demand, and the companion to
    /// [`BindingStore::mint_epoch`]. A consumer told that the epoch
    /// changed knows only that it has to start again; an operator
    /// asking why wants the other two values, and wants them from the
    /// database rather than from a log that may have rotated away.
    ///
    /// It reads the file rather than reporting what this process
    /// remembers, so a store whose epoch another process minted
    /// answers with the mint rather than with its own open.
    async fn identity(&self) -> Result<StoreIdentity>;

    /// The schema version of the database behind this store.
    ///
    /// Not part of any demand. It is here because an operator
    /// diagnosing a refusal to open, or a consumer deciding whether a
    /// feature exists, has no other way to ask.
    fn schema_version(&self) -> u32;

    /// How many answers this store has refused because the row it
    /// read belonged to a network other than the one asked about.
    ///
    /// Not part of any demand, and the most useful number the store
    /// exports. In a correct store it is zero for the life of the
    /// process, which makes a single non-zero reading unambiguous and
    /// in need of no baseline to interpret -- a better alarm than any
    /// rate. It is a count of answers *refused*, not of wrong answers
    /// served: the store returns an error and increments this rather
    /// than handing over a row from the wrong network, because a
    /// mechanism that noticed a cross-network leak and served it
    /// anyway would have converted a bug into a metric.
    fn cross_network_answers(&self) -> u64;

    /// How many views of this store are held right now.
    ///
    /// Not part of any demand. It is on the trait rather than on an
    /// implementation because the thing it is read for is a bound,
    /// and the bound belongs to the layer above: a server holding a
    /// `dyn BindingStore` has to be able to read the number it is
    /// bounding. Each view costs a thread for as long as it is held,
    /// which is what makes the number worth asking for, and it
    /// returns to zero when every consumer has let go.
    ///
    /// It counts every view this store has handed out, whatever
    /// opened it. A bound enforced on one layer's own count and
    /// reported against this one is the arrangement that notices the
    /// day the two disagree.
    fn live_views(&self) -> usize;

    /// A receiver that moves whenever the store's order advances.
    ///
    /// Serves demand 1 from the other side: the store assigns the
    /// order, so the store is the only thing that can say when it
    /// moved. Without this a consumer could only poll the history,
    /// and a view is a thread -- polling would cost a thread per poll
    /// per consumer, which makes this a matter of correctness and
    /// cost rather than of convenience.
    ///
    /// It carries the latest position and not the events. A receiver
    /// that missed three movements sees only the newest, and that is
    /// wanted rather than tolerated: this is a signal to go and read,
    /// and what to read is answered by the history after the position
    /// the reader itself last reached.
    ///
    /// **The position it reports has already been committed.** A
    /// notification that outran its own transaction would send every
    /// reader to a position nothing can see yet, and the symptom
    /// would be an intermittently empty read rather than an error.
    fn subscribe(&self) -> watch::Receiver<Cursor>;

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
    ///
    /// [`ReadHandle::interface_owner`] answers the same question and
    /// is not a duplicate of this one, so neither should be removed
    /// as one. This is the question the datapath asks for every
    /// packet it answers, which is why it is here: it needs an answer
    /// cheaply and with no position attached, and a view costs a
    /// thread for as long as it is held -- a thread per packet. The
    /// one on a view answers as of a position, alongside whatever
    /// else the caller is reading at it.
    async fn interface_owner(&self, namespace: &str, ifname: &str) -> Result<Option<Attachment>>;

    /// Create a network called `network`.
    ///
    /// Serves demands 1 and 3. The create is itself a compare-and-set
    /// against the resource not existing: a name already in use is
    /// refused rather than overwritten, and the refusal says which
    /// name.
    async fn create_network(&self, actor: &Actor, network: &str) -> Result<Cursor>;

    /// Remove the network `network`, which must still be at `expected`.
    ///
    /// Serves demands 1, 3 and 4. A network that still has interfaces
    /// attached or ranges configured is refused rather than emptied,
    /// and the refusal names what is in the way: deleting a network
    /// and silently detaching its interfaces would hand those
    /// interfaces to whichever network claimed them next.
    async fn delete_network(
        &self,
        actor: &Actor,
        network: &str,
        expected: &Cursor,
    ) -> Result<Cursor>;

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
        actor: &Actor,
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
        actor: &Actor,
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
        actor: &Actor,
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
        actor: &Actor,
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
        actor: &Actor,
        network: &str,
        name: &str,
        expected: &Cursor,
    ) -> Result<Cursor>;

    /// Configure `network`'s answer for the client identified by
    /// `selector`.
    ///
    /// Serves demands 1, 3 and 4. The create is a compare-and-set
    /// against the binding not existing: a selector this network
    /// already has a binding for is refused rather than overwritten,
    /// because two configured answers for one client is a question
    /// with two answers and the operator has to say which they meant.
    ///
    /// The same selector in another network is not a conflict and
    /// never was. A selector means nothing outside a network -- the
    /// same hardware address in two networks is two unrelated clients
    /// -- which is why it is a parameter beside the network here and
    /// not a resource with an identity of its own.
    async fn create_binding(
        &self,
        actor: &Actor,
        network: &str,
        selector: &Selector,
        address: std::net::Ipv4Addr,
        hostname: Option<&str>,
    ) -> Result<Cursor>;

    /// Change what `network` answers the client identified by
    /// `selector`, which must still be at `expected`.
    ///
    /// Serves demands 1, 3 and 4. The selector itself cannot be
    /// changed: it is the binding's identity, so a binding whose
    /// selector moved is a different binding, and a consumer watching
    /// the history has to be told that one resource went away and
    /// another arrived rather than that one changed shape.
    async fn update_binding(
        &self,
        actor: &Actor,
        network: &str,
        selector: &Selector,
        expected: &Cursor,
        address: std::net::Ipv4Addr,
        hostname: Option<&str>,
    ) -> Result<Cursor>;

    /// Remove `network`'s binding for `selector`, which must still be
    /// at `expected`.
    ///
    /// Serves demands 1, 3 and 4.
    async fn delete_binding(
        &self,
        actor: &Actor,
        network: &str,
        selector: &Selector,
        expected: &Cursor,
    ) -> Result<Cursor>;

    /// Configure `network`'s answer for `selector`, taking the lease
    /// that holds `address` away from its holder in the same
    /// operation.
    ///
    /// Serves demands 1, 3 and 4, and it is the second of the two
    /// operations the specification names as single ones. It is one
    /// operation because both writes are in one transaction: a
    /// refused binding rolls the revocation back with it, so there is
    /// no state in which an address was taken from a client and the
    /// binding that justified taking it was not written.
    ///
    /// `holder` is the lease id, which is the identity
    /// [`BindingStore::create_binding`]'s refusal named. The lease id
    /// changes when and only when the address changes holder, so a
    /// renewal in between does not invalidate the instruction and a
    /// reassignment does -- which is the distinction an operator
    /// taking an address from a named client cares about, and the
    /// reason the token is the id rather than the position of the
    /// lease's last write.
    async fn create_binding_revoking_lease(
        &self,
        actor: &Actor,
        network: &str,
        selector: &Selector,
        address: std::net::Ipv4Addr,
        hostname: Option<&str>,
        holder: &Cursor,
    ) -> Result<Cursor>;

    /// Record a client presenting `identity` as holding `address` in
    /// `network` for `duration` seconds.
    ///
    /// Serves demands 1, 3 and 4. This is the path for an address
    /// something else chose -- a reservation an operator configured,
    /// or an address a consumer allocates elsewhere and wants the
    /// store to agree with -- so the address does not have to fall
    /// inside any of the network's ranges. A network with no range at
    /// all is an ordinary configuration here, and a reservation
    /// outside a pool is how an operator holds an address the pool
    /// can never hand out.
    ///
    /// The compare-and-set is on the holder. An address this client
    /// already holds is renewed and keeps its lease id; one whose
    /// lease has lapsed is taken afresh with a new id, the holding
    /// that ended not being the one that begins; one another client
    /// holds is refused, naming the holder, because moving an address
    /// between clients is something a caller has to ask for.
    ///
    /// Returns the position the write was assigned and the lease as
    /// it now stands. The lease carries that same position, and both
    /// are returned because each answers a different question: the
    /// position orders the write against a stream the caller may be
    /// watching, and the lease says which identity and which lease id
    /// the address was recorded under without a second read at a
    /// later position.
    async fn acquire_lease(
        &self,
        actor: &Actor,
        network: &str,
        identity: &ClientIdentity,
        address: std::net::Ipv4Addr,
        duration: i64,
    ) -> Result<(Cursor, Lease)>;

    /// Give a client presenting `identity` an address this store
    /// chooses in `network`, for `duration` seconds.
    ///
    /// Serves demands 1, 3 and 4. The order the candidates are
    /// considered in is specified -- the address the client already
    /// holds, then the one it held until its lease lapsed, then the
    /// one it asked for, then one from the pool -- and the choice of
    /// the *lowest free* address within the pool is this project's,
    /// the specification saying nothing about which address a pool
    /// offers. [`crate::Lease`] and the store's own documentation
    /// carry the citation and the reasoning.
    ///
    /// `requested` is what the client asked for, if it asked. It is
    /// honoured only at its place in that order and only if the
    /// address is allocatable, so a client cannot talk its way past a
    /// lease or a reservation by asking.
    ///
    /// Returns the position and the lease, for the reason given on
    /// [`BindingStore::acquire_lease`].
    async fn allocate_lease(
        &self,
        actor: &Actor,
        network: &str,
        identity: &ClientIdentity,
        requested: Option<std::net::Ipv4Addr>,
        duration: i64,
    ) -> Result<(Cursor, Lease)>;

    /// Extend the lease on `address` in `network` by `duration`
    /// seconds, keeping its lease id.
    ///
    /// Serves demands 1, 3 and 4, and its compare-and-set is on
    /// identity rather than on a position -- which is the form demand
    /// 3 asks for and the only one this caller can satisfy. A renewal
    /// arrives as a client's request on the wire, carrying what the
    /// client presents and no position in the store's order. So the
    /// refusals are the two an identity can decide: an address held
    /// by another client is refused and names it, and an address
    /// whose lease has lapsed is refused as having no live lease,
    /// because the address was free in the interim and may have been
    /// given away. A client in that position acquires again, and
    /// acquiring is what issues a new lease id.
    async fn renew_lease(
        &self,
        actor: &Actor,
        network: &str,
        address: std::net::Ipv4Addr,
        identity: &ClientIdentity,
        duration: i64,
    ) -> Result<Cursor>;

    /// Take the lease on `address` in `network` away, which must
    /// still be at `expected`.
    ///
    /// Serves demands 1, 3 and 4. One method for two callers, because
    /// they are the same write: an operator taking an address back,
    /// and a front-end handling a client that has given one up. Both
    /// have the position, having read the lease in order to decide.
    ///
    /// A lapsed lease may be revoked. Nothing holds the address, but
    /// the row is still there, and removing it is how a caller says
    /// so deliberately rather than waiting for a sweep.
    async fn revoke_lease(
        &self,
        actor: &Actor,
        network: &str,
        address: std::net::Ipv4Addr,
        expected: &Cursor,
    ) -> Result<Cursor>;

    /// Record the expiry of every lease in `network` whose instant has
    /// passed, and remove the rows.
    ///
    /// Serves demands 1, 4 and 5, and it is worth being plain about
    /// what it is *not*. Nothing in this store waits for it. Every
    /// read evaluates expiry itself and every allocation refuses an
    /// address a live lease holds, so a sweeper that never runs
    /// leaves the store's answers correct and only its history quiet
    /// -- which is the point: a sweeper that dies cannot cause a
    /// double allocation. Its one job is to turn expiries into events
    /// so that a consumer watching the history is told an address was
    /// given up rather than left to compare an instant against its
    /// own clock. Nothing written later should come to depend on this
    /// having run.
    ///
    /// Returns the position of the last event it wrote and the leases
    /// it removed. The position is `None` when nothing had expired: a
    /// sweep that removed nothing wrote nothing, and reporting a
    /// position for it would invent one.
    async fn sweep_expired_leases(
        &self,
        actor: &Actor,
        network: &str,
    ) -> Result<(Option<Cursor>, Vec<Lease>)>;

    /// Give this store a new epoch, and say where the new order
    /// starts.
    ///
    /// Serves demand 1, and it is the operator's remedy for a store
    /// restored from a file-level backup. Such a restore brings the
    /// old epoch back together with a sequence counter wound back to
    /// the backup, so every position a consumer remembers is one this
    /// store may issue again -- different events under numbers a
    /// consumer has already seen, which is the failure the epoch is
    /// named for and the one case it cannot catch by itself. Minting
    /// makes the restored store honestly a different store: every
    /// cursor from the old order stops matching, so every consumer is
    /// told the epoch changed and reconverges from a snapshot.
    /// Minting *is* "everybody resync", expressed as data rather than
    /// as an instruction nobody can enforce.
    ///
    /// **The sequence counter does not restart.** What comes back is
    /// the position this store has reached, now in the new epoch, and
    /// the numbers carry on from there: a position from the old order
    /// is never issued again in the new one, so the two cannot be
    /// confused even by something that compared sequences and ignored
    /// epochs.
    ///
    /// The actor and the instant are recorded on the identity row,
    /// which is the one place a mint can be attributed: it appends no
    /// event, and the history that would have carried one is bounded
    /// anyway, while this answer has to outlive every consumer that
    /// resynced because of it. It is the most consequential write an
    /// operator can make -- every consumer holding a position is told
    /// to start again -- and [`BindingStore::identity`] is what asks
    /// the question back.
    async fn mint_epoch(&self, actor: &Actor) -> Result<Cursor>;
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
/// from an oversight: `cursor`, `networks`, `interface_owner`,
/// `explain`, `history_everywhere`.
///
/// - `cursor` is the position this view reads at, which spans every
///   network because the order does.
/// - `networks` lists the networks themselves. Its answer *is* the
///   set of networks, so there is no network it could be scoped by;
///   every other read here names one.
/// - `interface_owner` answers which network has claimed an
///   interface, so its answer is a network and there is none to ask
///   it within. It is the same question
///   [`BindingStore::interface_owner`] answers and not the same
///   method, for the reason both doc comments give: one is asked per
///   packet and must cost no view, and this one is asked as of a
///   position, beside a caller's other reads.
/// - `explain` starts where a packet starts, with the interface it
///   arrived on, and resolving that to a network is the first thing
///   it does. Its answer therefore contains a network rather than
///   taking one, for the same reason
///   [`BindingStore::interface_owner`] does -- and reporting which
///   network an interface resolved to is half of what makes it a
///   diagnostic, because an interface attached to the wrong network
///   looks exactly like a binding that does not match.
/// - `history_everywhere` reads the history of every network at once,
///   which is a scope a watch may legitimately ask for. It is the one
///   method here with a network-named twin: [`ReadHandle::history`]
///   is the scoped read and is what a watch on one network uses.
///   Composing the wide answer out of the narrow one would mean
///   merging several networks' orders in the layer least able to
///   check that it merged them correctly, so the store does it. Every
///   event still names its network, and the interleaving still says
///   nothing about causality between them.
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
    /// [`ReadHandle::interface_owner`] is how a caller asks the
    /// question that has no network at this same position, and
    /// [`BindingStore::interface_owner`] is how the datapath asks it
    /// without opening a view at all.
    async fn attachment(
        &self,
        network: &str,
        namespace: &str,
        ifname: &str,
    ) -> Result<Option<Attachment>>;

    /// Which network, if any, has claimed `ifname` in `namespace`, at
    /// this handle's position.
    ///
    /// Store-global, for the reason in this trait's doc comment. The
    /// twin of [`BindingStore::interface_owner`] rather than a
    /// duplicate of it, and the difference is which question each is
    /// asked: that one is the datapath's, per packet, where a view
    /// would cost a thread apiece; this one is asked by a caller that
    /// is reading other things at the same position and needs the
    /// answer to agree with them. A caller that resolved an interface
    /// here and then read the attachment through this handle is
    /// answered about one instant, where two separate reads could
    /// describe two states that were never true together.
    async fn interface_owner(&self, namespace: &str, ifname: &str) -> Result<Option<Attachment>>;

    /// Every range `network` owns, lowest address first.
    ///
    /// Ordered by address rather than by name because every consumer
    /// of this list is looking for the range an address falls in, and
    /// an order the store did not fix is one each caller would sort
    /// again.
    async fn address_ranges(&self, network: &str) -> Result<Vec<AddressRange>>;

    /// `network`'s range called `name`, or `None`.
    async fn address_range(&self, network: &str, name: &str) -> Result<Option<AddressRange>>;

    /// Every binding `network` has configured, by selector.
    ///
    /// Serves demand 4, and in the order of the key rather than of
    /// the precedence between selector kinds: precedence decides a
    /// lookup, where a listing is what an operator reads to see what
    /// is configured.
    async fn bindings(&self, network: &str) -> Result<Vec<Binding>>;

    /// `network`'s binding for `selector`, or `None`.
    ///
    /// The same selector in another network is a different binding
    /// and is not returned here, which is the whole of why a selector
    /// is a parameter beside a network and not an identity on its
    /// own.
    async fn binding(&self, network: &str, selector: &Selector) -> Result<Option<Binding>>;

    /// Which of `network`'s bindings applies to a client presenting
    /// `identity`.
    ///
    /// This is the lookup the whole store is shaped around. The
    /// client's attributes are consulted in a precedence order this
    /// project fixed and published -- no standard specifies one --
    /// and the first that has a binding configured in this network
    /// wins.
    ///
    /// The binding that comes back carries the selector it was found
    /// under, which is how a consumer is told *which* selector
    /// matched. That is not a nicety. Because the order is this
    /// project's own invention, a consumer has no way to check that
    /// its understanding of the order agrees with the daemon's except
    /// by being told which rule was applied -- so an operator who
    /// configured a reservation against a hardware address and is
    /// told that a vendor class matched has found a precedence
    /// problem before a user does.
    ///
    /// `None` means this network has no binding for anything the
    /// client presented, which is a complete answer and not a
    /// failure: a network may legitimately configure nothing for a
    /// client it has never seen.
    async fn lookup_binding(
        &self,
        network: &str,
        identity: &ClientIdentity,
    ) -> Result<Option<Binding>>;

    /// Which of `network`'s bindings applies to a client presenting
    /// `identity` on an interface the caller asserts belongs to
    /// `network`.
    ///
    /// Serves demand 4, and it is the one read in the store where the
    /// cross-network check can fire on data rather than on a defect.
    /// The network comes from the caller -- a path that names it, a
    /// request's context -- while the interface's network comes from
    /// the store, so the two can disagree. A request whose arrival
    /// interface resolved to one network while the answer would have
    /// come from another is exactly the thing
    /// [`BindingStore::cross_network_answers`] counts, and it is
    /// refused rather than answered.
    ///
    /// An interface no network has claimed is reported as not found,
    /// because nothing crossed between networks and the fault is a
    /// different one. Note the contrast with
    /// [`ReadHandle::attachment`], which answers `None` for an
    /// interface another network holds: that read asks whether a
    /// network has an interface, and no is a legitimate answer, where
    /// this one asserts that it does in order to be answered for a
    /// client on it.
    async fn lookup_binding_on_interface(
        &self,
        network: &str,
        namespace: &str,
        ifname: &str,
        identity: &ClientIdentity,
    ) -> Result<Option<Binding>>;

    /// What the store would answer a client presenting `identity` on
    /// this interface, with no packet involved.
    ///
    /// Store-global, for the reason in this trait's doc comment. The
    /// answer says which network the interface resolved to, which
    /// binding would be returned, which selector matched it, and
    /// which selectors would have been consulted in what order -- so
    /// an operator or a test can ask the question the project's
    /// failure mode otherwise only answers hours later inside
    /// somebody's guest.
    ///
    /// It is the same code path as the lookup above with the packet
    /// replaced by its parameters, and deliberately so: a diagnostic
    /// that reimplemented the matching could agree with the
    /// documentation while the datapath did something else, which
    /// would make it worse than having none.
    async fn explain(
        &self,
        namespace: &str,
        ifname: &str,
        identity: &ClientIdentity,
    ) -> Result<Explanation>;

    /// Every lease in `network`, lowest address first.
    ///
    /// Serves demand 4. Expired leases are included, each saying so,
    /// because an address that is free and still remembered is the
    /// thing an operator most needs to see and because hiding them
    /// would make a sweep's effect invisible.
    async fn leases(&self, network: &str) -> Result<Vec<Lease>>;

    /// The lease on `address` in `network`, or `None` if there has
    /// never been one or it has been revoked or swept.
    ///
    /// Serves demand 4. A lease whose instant has passed is returned,
    /// marked expired, rather than treated as absent: nothing holds
    /// the address, and that is a different fact from nothing having
    /// held it.
    async fn lease(&self, network: &str, address: std::net::Ipv4Addr) -> Result<Option<Lease>>;

    /// Every lease in `network` held by a client presenting
    /// `identity`.
    ///
    /// Serves demand 4, and it is the query the resource model asks
    /// for by name -- "answering 'what does this client hold' is the
    /// thing the existing consumer cannot do today". It is a store
    /// operation rather than a filter a caller applies to a list of
    /// everything, which is the difference between an indexed read
    /// inside one network and a listing a caller could accidentally
    /// take across two.
    ///
    /// The answer is a list because a lease is keyed by address: a
    /// client moving from one address to another holds both for a
    /// moment, and that is the case the key exists to allow. It is
    /// ordered by the precedence of the attribute each lease was
    /// recorded against and then by address, so a caller that wants
    /// one lease may take the first and get the same one every time.
    ///
    /// Expired leases are included, flagged. A lapsed lease is how
    /// the store remembers which address a client used to have, which
    /// is a step of the address selection order, so hiding it would
    /// hide the input to that step.
    async fn leases_held_by(&self, network: &str, identity: &ClientIdentity) -> Result<Vec<Lease>>;

    /// Everything `network` holds, as one event per resource, read at
    /// this handle's position.
    ///
    /// Serves demands 2, 4 and 5, and it is the first half of the
    /// contract a watching consumer is held to. The snapshot is read
    /// at a single position -- it is this view, so it is a consistent
    /// view and not a walk that races mutations -- and it carries that
    /// position with it. A stream that then resumes at exactly that
    /// position has no gap to lose an event in and no overlap to
    /// deduplicate, because the snapshot is everything up to and
    /// including it and a resume asks for what came after.
    ///
    /// Every event names its network, including here where the caller
    /// named one. A handler that learned to infer it is wrong the day
    /// somebody widens the question.
    ///
    /// A snapshot is of one network because everything in it is; a
    /// consumer watching several takes one per network from the same
    /// handle, which is the same position for all of them and so the
    /// same guarantee. A network this view does not hold is refused
    /// rather than answered with an empty snapshot: the act a consumer
    /// performs on a complete snapshot is to delete whatever the store
    /// did not mention, so "there is no such network" and "this
    /// network has nothing in it" must not share an answer.
    async fn snapshot(&self, network: &str) -> Result<Snapshot>;

    /// What happened in `network` after `after`, or why it cannot be
    /// told.
    ///
    /// Serves demands 1, 4 and 5, and it is the second half of the
    /// watch contract. A position inside the retained history is
    /// answered with every event after it, in order, **including the
    /// intermediate states**: a lease created and then released while
    /// a consumer was away produces both events and not neither,
    /// because a consumer that was holding the earlier state would
    /// otherwise never see it end.
    ///
    /// A position that cannot be resumed from is answered with which
    /// of the three refusals applies -- the events after it have been
    /// evicted, its epoch belongs to another store, or it is a
    /// position this store has never issued. Each is a value rather
    /// than an error with a sentence in it, because the layer above
    /// turns each into its own `resync` reason, and a consumer that
    /// had to parse a message to tell them apart would stop being
    /// able to the day the message was reworded. [`History`] is where
    /// each is described.
    ///
    /// The events run up to this handle's own position, which the
    /// answer carries, so reading a snapshot and then resuming from
    /// its position joins exactly: no event is delivered twice and
    /// none is skipped.
    ///
    /// The order events come back in is the order the store applied
    /// them, and within one network that order is meaningful. Across
    /// networks it is **not causality**: two networks' events are
    /// interleaved by nothing more than when they arrived, and a
    /// consumer that reads "this happened because of that" out of two
    /// adjacent positions in different networks has invented a
    /// relationship the store never claimed. This layer can only say
    /// so, which is why it says so here.
    async fn history(&self, network: &str, after: &Cursor) -> Result<History>;

    /// What happened anywhere in the store after `after`, or why it
    /// cannot be told.
    ///
    /// Store-global, for the reason in this trait's doc comment: the
    /// scope of a watch may be every network, and merging the
    /// per-network reads to serve it belongs here rather than above.
    /// Everything [`ReadHandle::history`] says applies, and the
    /// warning about causality applies most of all -- this is the read
    /// whose answer interleaves networks, so it is the one from which
    /// a consumer could most easily invent a relationship between
    /// them.
    async fn history_everywhere(&self, after: &Cursor) -> Result<History>;

    /// One value for `network`'s configuration, at this position.
    ///
    /// Serves demand 4. A consumer holding a mirror compares this
    /// instead of diffing everything, and a mismatch means "resync"
    /// rather than "investigate" -- which is the direct replacement
    /// for re-deriving what the daemon should have done and comparing
    /// that with what it did.
    ///
    /// It covers what an operator configured and not the leases in the
    /// network, and it is opaque. [`Digest`] says why for both, and
    /// the property that matters is that a change in another network
    /// cannot move it: a value derived from the store's position would
    /// satisfy every other requirement here and fail that one.
    ///
    /// A network this view does not hold is refused rather than given
    /// a digest of nothing, for the reason
    /// [`ReadHandle::snapshot`] is: a consumer whose network has been
    /// deleted has not been told that its mirror is still correct.
    async fn digest(&self, network: &str) -> Result<Digest>;
}
