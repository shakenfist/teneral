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
//!    write emits is already durable, written in the same transaction
//!    as the write it describes; the methods that read that history
//!    back and the bound that truncates it are not on these traits
//!    yet, and the gap is named here rather than left to be
//!    discovered.

use async_trait::async_trait;

use crate::cursor::Cursor;
use crate::error::Result;
use crate::identity::Epoch;
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
/// `interface_owner`, `cross_network_answers`.
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
    async fn sweep_expired_leases(&self, network: &str) -> Result<(Option<Cursor>, Vec<Lease>)>;
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
/// from an oversight: `cursor`, `networks`, `explain`.
///
/// - `cursor` is the position this view reads at, which spans every
///   network because the order does.
/// - `networks` lists the networks themselves. Its answer *is* the
///   set of networks, so there is no network it could be scoped by;
///   every other read here names one.
/// - `explain` starts where a packet starts, with the interface it
///   arrived on, and resolving that to a network is the first thing
///   it does. Its answer therefore contains a network rather than
///   taking one, for the same reason
///   [`BindingStore::interface_owner`] does -- and reporting which
///   network an interface resolved to is half of what makes it a
///   diagnostic, because an interface attached to the wrong network
///   looks exactly like a binding that does not match.
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
}
