//! The history a consumer reads, the bound that keeps it finite, and
//! the digest that saves a consumer from reading it at all.
//!
//! Every write in this crate appends an event in its own transaction,
//! so the history exists already. What is here is the other half: the
//! reads over it, the three answers a remembered position can get, and
//! the truncation that stops the log growing without end.
//!
//! # The contract a consumer is held to
//!
//! A consumer with no position takes a [`Snapshot`]: the current state
//! of a network as one event per resource, read at a single position
//! in the store's order and carrying that position. It is a consistent
//! view rather than a walk that races mutations -- the view's
//! transaction is what makes that true -- and the position it carries
//! is the one to resume at. There is then no gap for an event to be
//! lost in and no overlap for a consumer to deduplicate, because
//! resuming asks for the events *after* that position and the snapshot
//! is everything up to and including it.
//!
//! A consumer with a position reads [`History`]. Three of its four
//! answers are refusals to resume, and they are separate answers
//! rather than one error with a sentence to parse, because the layer
//! above has to act on them differently and a consumer must never
//! learn which happened by reading a message:
//!
//! - **Resumed.** The position is inside the retained history. Every
//!   event after it, in order, and the position the stream has now
//!   reached.
//! - **Evicted.** The events after that position have been truncated.
//!   The consumer cannot be caught up and must take a snapshot.
//! - **The epoch changed.** The position belongs to a different
//!   store's order, so its sequence number means nothing here.
//! - **Never issued.** The epoch matches and the position is *ahead*
//!   of everything this store has ever assigned. See below: this is
//!   what a restore from a backup looks like from the inside, and it
//!   is the one case where an empty answer would be a confident lie.
//!
//! The events a resume delivers include the intermediate states. A
//! lease created and then released while a consumer was away produces
//! both events and not neither: collapsing them would be cheaper and
//! would make the stream unusable as a sequence, because a consumer
//! holding the earlier state would never see it end.
//!
//! # What the order is not
//!
//! Every event carries the network it belongs to, including when it
//! was read through a single network's history. A consumer that
//! learned to infer the network from the question it asked is wrong
//! the day somebody widens the question, and repeating it costs a
//! string.
//!
//! The order is total across the whole store, and **a consumer must
//! not read causality between networks from it**. Two networks'
//! events are interleaved by nothing more than the order they
//! arrived in; the sequence exists so that a stream can be resumed,
//! not so that something in one network can be shown to have caused
//! something in another. Within one network the order is meaningful
//! and is the order the store applied the changes in. This layer can
//! do nothing to enforce that beyond saying it, which is why it is
//! said here, on [`Event`] and on each method that returns one.
//!
//! # The bound
//!
//! The history is bounded by a count of events and by their age, with
//! the count as the operative bound -- the event rate between a home
//! network and a hypervisor fleet differs by orders of magnitude, so a
//! duration that held a useful window for one would hold minutes or
//! months for the other. [`Retention`] carries both and says what the
//! defaults are sized for.
//!
//! Two properties of the truncation are load-bearing.
//!
//! It happens **in the same transaction as the write that triggered
//! it**, which is what makes the bound durable rather than aspirational:
//! a store that evicted outside the write would have a window that a
//! crash could leave in any state, and a low-water mark that disagreed
//! with the log it describes.
//!
//! And it only ever deletes a **prefix** of the order, expressed as a
//! sequence floor. The retained events are therefore always a suffix,
//! which is what lets one number -- `evicted_through` -- describe
//! completely what is gone, and what makes "can this position be
//! resumed" a comparison rather than a search.
//!
//! One consequence is worth having in view, because it looks like a
//! dangling reference and is not. A resource row records the position
//! it was last written at, and nothing stops that event being evicted
//! while the row stays exactly as it is -- an address range nobody has
//! touched for a week is the ordinary case. The position goes on
//! working for what a row's cursor is for, which is "change this only
//! if it is still what I read", that being a comparison of two numbers
//! and not a lookup in the log. What it stops being is a position to
//! resume a stream from, and the answer a consumer gets if it tries is
//! the correct one: the events after it are gone, so take a snapshot.
//!
//! # The epoch, a restore, and the fourth answer
//!
//! A cursor's epoch is generated when the database is created and
//! lives in the file. A restore from a file-level backup therefore
//! brings the epoch back with it, along with a sequence counter that
//! has been wound back -- so the epoch, which exists to mark exactly
//! that event, cannot by itself detect the commonest way of causing
//! it. What detects it is a consumer presenting a position this store
//! has not issued, which is why that is an answer of its own rather
//! than folded into eviction. Folding it in would report a position
//! as having been evicted when nothing was ever there, and a store
//! that cannot tell "gone" from "never happened" has given up the
//! distinction the low-water mark is kept for.

use rusqlite::{Connection, Transaction, params};

use crate::cursor::Cursor;
use crate::error::{Error, Result};
use crate::identity::Epoch;
use crate::lease;
use crate::ops::{self, Reading};
use crate::selector::Selector;

/// The kind of resource an event describes.
///
/// The log's `kind` column is a string rather than a column per
/// resource, which is what makes a resource this schema does not hold
/// yet a migration and a variant rather than a reshaping of the one
/// table every consumer reads. This enum is the single list of those
/// strings: the writers stamp them through [`EventKind::as_str`] and
/// the reads parse them through [`EventKind::from_stored`], so a kind
/// a writer emits and a kind a reader expects cannot drift apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    /// A network was created or removed.
    Network,
    /// An interface was claimed by a network or released by it.
    Attachment,
    /// An address range was created, resized or removed.
    AddressRange,
    /// A binding was configured, changed or removed.
    Binding,
    /// A lease was taken, renewed, revoked or swept.
    Lease,
}

impl EventKind {
    /// Every kind, so that a caller filtering a stream can enumerate
    /// them rather than guess at the strings.
    pub const ALL: [EventKind; 5] = [
        EventKind::Network,
        EventKind::Attachment,
        EventKind::AddressRange,
        EventKind::Binding,
        EventKind::Lease,
    ];

    /// The kind as it is stored and as a consumer sees it.
    ///
    /// Hyphenated lower case rather than the Rust spelling, for the
    /// same reason a selector kind is: the value is in a database
    /// column and in an event payload, and neither is a place to read
    /// a Rust identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Network => "network",
            EventKind::Attachment => "attachment",
            EventKind::AddressRange => "address-range",
            EventKind::Binding => "binding",
            EventKind::Lease => "lease",
        }
    }

    /// The kind a stored string names.
    ///
    /// A value this code wrote always parses. One that does not means
    /// something else has been writing the log, which is worth saying
    /// rather than reporting as an event of some unknown kind that
    /// every consumer would then have to decide what to do with.
    pub(crate) fn from_stored(value: &str) -> Result<Self> {
        EventKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value)
            .ok_or_else(|| Error::Malformed {
                detail: format!("its history holds an event of the kind {value:?}"),
            })
    }

    /// What the store calls this kind when it has to say so to a
    /// person, which is not the same string as above.
    pub(crate) fn resource(self) -> &'static str {
        match self {
            EventKind::Network => ops::NETWORK,
            EventKind::Attachment => ops::ATTACHMENT,
            EventKind::AddressRange => ops::RANGE,
            EventKind::Binding => ops::BINDING,
            EventKind::Lease => ops::LEASE,
        }
    }
}

/// One change to one resource, or one resource as it currently stands.
///
/// An event carries the **whole post-state** of a resource rather than
/// a delta, which is what makes applying it idempotent and so makes
/// at-least-once delivery enough. A deletion is the same shape with no
/// state at all, that being the post-state of something removed.
///
/// It carries its network always, including when it was read from a
/// single network's history, and its position in the store's one
/// order. That order is not causality between networks: see this
/// module's documentation, because a consumer that reads causality out
/// of two adjacent positions in different networks has invented a
/// relationship the store never claimed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// Where this event sits in the store's order.
    pub cursor: Cursor,
    /// The network the resource belongs to.
    pub network: String,
    /// What kind of resource it is.
    pub kind: EventKind,
    /// The resource's identity within its network, as its own kind
    /// renders it: a name, a `namespace/ifname`, a `kind:value`
    /// selector, or an address.
    pub resource_id: String,
    /// The resource's whole state after the change, as JSON, or `None`
    /// if the resource is gone.
    ///
    /// JSON text rather than a parsed value because the store neither
    /// parses nor composes it: the engine builds it from the same row
    /// the write produced, so the escaping is the engine's and this
    /// crate needs no serialisation dependency to be correct about a
    /// network called `say "hi"`.
    pub state: Option<String>,
    /// When the event was recorded, as seconds since the Unix epoch,
    /// for an event read from the history.
    ///
    /// `None` for an event that came from a [`Snapshot`], and the
    /// asymmetry is honest rather than an omission: a resource row
    /// records the position it was last written at and not the instant,
    /// and a snapshot is a statement about now rather than about when
    /// each resource last changed. A consumer ordering events uses the
    /// cursor, which both kinds carry.
    pub recorded_at: Option<i64>,
}

/// The state of one network, read at a single position in the store's
/// order.
///
/// The events are one per resource and say that it exists rather than
/// that it has just changed; the layer above marks them as such on the
/// wire. They are in the order of the positions they were last written
/// at, which puts a network ahead of everything configured inside it,
/// a child not being writable before its parent exists.
///
/// The cursor is the whole point of the type. It is the position every
/// event here was read at, so a stream that resumes at exactly it has
/// no gap to lose an event in and no overlap to deduplicate -- and
/// because the two travel together, no caller can pair this snapshot
/// with some other position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// One event per resource in the network, existing rather than
    /// changed.
    pub events: Vec<Event>,
    /// The position the whole snapshot was read at.
    pub cursor: Cursor,
}

/// What a remembered position gets in answer.
///
/// Four answers, three of which are refusals, as distinct values
/// rather than as one error carrying a sentence. The layer above turns
/// each refusal into its own `resync` reason, and a consumer that had
/// to parse a message to tell them apart would be a consumer that
/// stops being able to the day the message is reworded.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum History {
    /// Carry on: the position was inside the retained history.
    Resumed {
        /// Every event after the position presented, in order,
        /// including the intermediate states.
        events: Vec<Event>,
        /// The position the stream has reached, which is where the
        /// next read resumes from.
        cursor: Cursor,
    },

    /// The events after that position are gone, so the consumer
    /// cannot be caught up and has to take a snapshot instead.
    ///
    /// Distinct from an empty answer, which is what a store without a
    /// low-water mark would have to give: "nothing has happened since
    /// your position" and "what happened since your position has been
    /// discarded" are different facts with different consequences, and
    /// a consumer told the first when the second is true holds a stale
    /// state and will never be corrected.
    Evicted {
        /// What the consumer presented.
        presented: Cursor,
        /// The highest position whose event has been truncated.
        /// Everything at or before this is gone for good.
        evicted_through: Cursor,
    },

    /// The position belongs to a different store's order, so its
    /// sequence number cannot be continued here.
    EpochChanged {
        /// The epoch the consumer presented.
        presented: Epoch,
        /// The epoch this store issues positions in.
        store: Epoch,
    },

    /// The epoch matches and the position is ahead of every position
    /// this store has issued.
    ///
    /// This is what a restore from a file-level backup looks like from
    /// the inside: the epoch comes back with the file, and the
    /// sequence counter comes back wound to where it was when the
    /// backup was taken. Reporting it as eviction would say that
    /// something was discarded when nothing was ever there, and
    /// reporting it as an empty resume would tell a consumer holding
    /// events this store has never issued that it is up to date.
    NeverIssued {
        /// What the consumer presented.
        presented: Cursor,
        /// The last position this store has issued.
        latest: Cursor,
    },
}

/// A cheap value that changes when a network's configuration changes.
///
/// A consumer holding a mirror compares one of these instead of
/// diffing everything, and a mismatch means "resync" rather than
/// "investigate" -- which is the direct replacement for re-deriving
/// what the daemon should have done and comparing that with what it
/// did.
///
/// Two things about it are worth knowing before it is relied on.
///
/// It covers what an operator **configured** in the network -- the
/// network itself, the interfaces attached to it, its address ranges
/// and its bindings -- and deliberately not its leases. A lease
/// changes whenever a client appears, so a digest including them would
/// change constantly and would answer "has anything happened" rather
/// than "do we agree about the configuration", which is the question
/// a mirror needs to ask. A consumer mirroring leases watches them.
///
/// And the value is **opaque**: compare two for equality and nothing
/// else. The construction is a position and a count, which is cheap
/// and needs no hash, but a consumer that unpicked it would be reading
/// an implementation detail -- and one that compared them for order
/// would be inventing a meaning that is not there.
///
/// Two of these are comparable only when they are the same network's.
/// It is not a hash of content: two networks configured identically
/// were configured at different positions, so their digests differ,
/// and a consumer comparing one network's digest with another's is
/// asking a question this value cannot answer. What it answers is
/// whether *this* network's configuration is still the one the
/// consumer last saw.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Digest(String);

impl Digest {
    /// The digest as it travels and as it is logged.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How much history the store keeps.
///
/// Bounded by a count of events and by their age, with the count as
/// the operative bound. The reason the count leads is in this module's
/// documentation: the rate differs by orders of magnitude between the
/// deployments this store serves, so a count is the bound that means
/// the same thing in both, and the age bound is there to stop a
/// store that went quiet from keeping events about states nobody will
/// ever ask to resume into.
///
/// Both are a property of the deployment rather than of the file, so
/// they are chosen when the store is opened and not written into the
/// database. What *is* written into the database is the consequence:
/// how far the log has been truncated, so that a restart does not
/// forget which positions it can no longer serve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retention {
    /// The most events the log keeps.
    events: u64,
    /// The oldest an event may be, in seconds.
    seconds: u64,
}

impl Retention {
    /// The default window: fifty thousand events, or a week,
    /// whichever bites first.
    ///
    /// The count is sized from the two deployments this store is
    /// written for and from what the smaller of them can hold. At the
    /// rate a hypervisor fleet produces -- a lease or a binding per
    /// machine event, in bursts -- fifty thousand events is a window
    /// measured in hours, so a consumer restarted in seconds or
    /// partitioned for minutes resumes, and one that has been away
    /// since yesterday resyncs. At a home network's rate it is months,
    /// which is where the age bound takes over. In storage it is some
    /// tens of megabytes of rows at the size this log's events run to,
    /// which the smallest host this daemon runs on can hold without
    /// being asked.
    ///
    /// The week is deliberately generous rather than tuned. Its job is
    /// to put a ceiling on a quiet store's log, not to shape the resume
    /// window, and a consumer that has been away a week is taking a
    /// snapshot whatever the number says. Nothing here is a promise to
    /// a consumer: the resume path is best-effort by construction, and
    /// resync is the normal answer rather than the error path, which is
    /// what makes these numbers safe to change later.
    pub const DEFAULT: Retention = Retention {
        events: 50_000,
        seconds: 7 * 24 * 60 * 60,
    };

    /// A window of `events` events and `seconds` seconds.
    ///
    /// Both have to be at least one. A bound of zero events would
    /// evict every event as it was written, which is a store whose
    /// every consumer resyncs for ever and whose history is a
    /// write-only table -- a configuration nobody wants and that
    /// nothing in the code below would otherwise refuse.
    pub fn new(events: u64, seconds: u64) -> Result<Self> {
        if events == 0 || seconds == 0 {
            return Err(Error::Invalid {
                what: "history retention",
                detail: format!(
                    "it keeps {events} event(s) for {seconds} second(s), and a history bounded at \
                     zero is one no consumer could ever resume from"
                ),
            });
        }

        Ok(Self { events, seconds })
    }

    /// The most events the log keeps.
    pub fn events(&self) -> u64 {
        self.events
    }

    /// The oldest an event may be, in seconds.
    pub fn seconds(&self) -> u64 {
        self.seconds
    }
}

impl Default for Retention {
    fn default() -> Self {
        Retention::DEFAULT
    }
}

// ---------------------------------------------------------------------
// Appending, and the bound that goes with it.
// ---------------------------------------------------------------------

/// The sequence the event just appended was given, with the bound
/// applied.
///
/// Every write learns its own position by calling this, which is the
/// point of its shape: applying the bound and learning the position
/// are one operation, so the truncation is inside the write's
/// transaction by construction rather than by each write remembering
/// to ask for it. A write that forgot would be a write that could not
/// name its cursor, and so would not compile.
pub(crate) fn appended(
    transaction: &Transaction<'_>,
    retention: &Retention,
    now: i64,
) -> Result<i64> {
    let sequence = transaction.last_insert_rowid();

    truncate(transaction, retention, sequence, now)?;

    Ok(sequence)
}

/// Evict whatever the bound puts outside the window.
///
/// Both bounds are turned into a sequence floor and the deletion is a
/// single range over the log's own key, which is what keeps the
/// retained history a suffix of the order. The floor is the later of
/// the two bounds' answers, and never the event this write just
/// appended: a caller is about to be handed that position as its
/// cursor, and a position that could not be resumed from would be a
/// cursor the store had already broken.
fn truncate(
    transaction: &Transaction<'_>,
    retention: &Retention,
    latest: i64,
    now: i64,
) -> Result<()> {
    let keep = i64::try_from(retention.events()).unwrap_or(i64::MAX);
    let by_count = latest.saturating_sub(keep);

    // The count bound is arithmetic on the position rather than a
    // count of rows, and the two are the same number while the store
    // issues positions without gaps -- which it does, the counter
    // moving inside the transaction that uses it and rolling back with
    // it. Should a gap ever appear, this keeps slightly fewer events
    // than the bound says, never more, so the window can only be
    // shorter than advertised and never a surprise.
    let cutoff = now.saturating_sub(i64::try_from(retention.seconds()).unwrap_or(i64::MAX));
    let by_age: i64 = transaction
        .query_row(
            "SELECT coalesce(max(seq), 0) FROM event_log WHERE recorded_at <= ?1",
            params![cutoff],
            |row| row.get(0),
        )
        .map_err(ops::storage("finding the events that have aged out"))?;

    let floor = by_count.max(by_age).min(latest.saturating_sub(1));
    let evicted = evicted_through(transaction)?;

    // Nothing new to evict, which is the common case once a store has
    // been running for a while without filling its window. Skipping
    // the two statements below keeps a write that evicts nothing from
    // dirtying the page holding the mark.
    if floor <= evicted {
        return Ok(());
    }

    transaction
        .execute("DELETE FROM event_log WHERE seq <= ?1", params![floor])
        .map_err(ops::storage("evicting the oldest events"))?;

    // In the same transaction as the deletion it describes, and so in
    // the same transaction as the write that triggered it. A mark
    // written separately could be lost while the deletion survived,
    // which is the one state in which the store would tell a consumer
    // that it can resume from a position whose events have gone.
    transaction
        .execute(
            "UPDATE event_history SET evicted_through = ?1 WHERE id = 1",
            params![floor],
        )
        .map_err(ops::storage(
            "recording how far the history has been evicted",
        ))?;

    Ok(())
}

/// The highest position whose event has been evicted, or zero.
fn evicted_through(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT evicted_through FROM event_history WHERE id = 1",
        [],
        |row| row.get(0),
    )
    .map_err(ops::storage("reading how far the history has been evicted"))
}

// ---------------------------------------------------------------------
// Reading the history.
//
// Both reads are answered from the connection they are given, so a
// connection inside a read transaction answers them at the position
// that transaction took -- which is also the upper bound on the events
// they return, the transaction being unable to see a commit made after
// it opened. That is why neither statement names an upper bound of its
// own: the view is the bound, and a second one written here could only
// disagree with it.
// ---------------------------------------------------------------------

/// What the position a consumer presented can be answered with, or
/// `None` if it can simply be resumed.
///
/// The order of the three refusals is the order in which they make
/// each other unaskable. A position from another epoch has a sequence
/// number that means nothing here, so no comparison against this
/// store's numbers is worth making; and a position this store has
/// never issued cannot meaningfully be called evicted.
fn refusal(conn: &Connection, at: &Cursor, after: &Cursor) -> Result<Option<History>> {
    if after.epoch() != at.epoch() {
        return Ok(Some(History::EpochChanged {
            presented: after.epoch().clone(),
            store: at.epoch().clone(),
        }));
    }

    if after.sequence() > at.sequence() {
        return Ok(Some(History::NeverIssued {
            presented: after.clone(),
            latest: at.clone(),
        }));
    }

    // At the mark rather than below it is resumable, and the boundary
    // is worth being exact about. What a consumer at position p needs
    // is every event *after* p, so it can be served whenever nothing
    // above p has been evicted -- which is exactly p >= the mark. A
    // store that refused at equality would resync a consumer that was
    // perfectly up to date.
    let evicted = evicted_through(conn)?;
    let evicted = ops::sequence_as_position(evicted)?;
    if after.sequence() < evicted {
        return Ok(Some(History::Evicted {
            presented: after.clone(),
            evicted_through: Cursor::new(at.epoch().clone(), evicted),
        }));
    }

    Ok(None)
}

/// One network's events after a position.
///
/// Scoped in the statement and checked on every row, like every other
/// read that names a network: the event carries the network it belongs
/// to, and an event from another network is refused and counted rather
/// than served.
pub(crate) fn history(
    conn: &Connection,
    reading: &Reading<'_>,
    at: &Cursor,
    after: &Cursor,
) -> Result<History> {
    if let Some(refusal) = refusal(conn, at, after)? {
        return Ok(refusal);
    }

    let mut statement = conn
        .prepare_cached(
            "SELECT seq, network, kind, resource_id, state, recorded_at FROM event_log
             WHERE network = ?1 AND seq > ?2 ORDER BY seq",
        )
        .map_err(ops::storage("preparing a read of a network's history"))?;

    let rows = statement
        .query_map(
            params![reading.network, position_as_sequence(after)?],
            read_event,
        )
        .map_err(ops::storage("reading a network's history"))?;

    let mut events = Vec::new();
    for row in rows {
        let row = row.map_err(ops::storage("reading an event"))?;
        let event = into_event(at.epoch(), row)?;
        reading.guard(&event.network, event.kind.resource(), || {
            event.resource_id.clone()
        })?;
        events.push(event);
    }

    Ok(History::Resumed {
        events,
        cursor: at.clone(),
    })
}

/// Every network's events after a position, in the one order they were
/// applied in.
///
/// Store-global, because the scope a watch may ask for is every
/// network and composing that from the per-network read above would
/// mean merging several orders in the layer that is least able to
/// check it did so correctly. Each event still names its network, and
/// the interleaving still carries no causality between them.
///
/// There is no cross-network check to make here, there being no
/// network the caller asked about for a row to disagree with. The
/// scoping that a check would have guarded is instead the caller's:
/// this method is reached only by a watch whose scope is everything.
pub(crate) fn history_everywhere(
    conn: &Connection,
    at: &Cursor,
    after: &Cursor,
) -> Result<History> {
    if let Some(refusal) = refusal(conn, at, after)? {
        return Ok(refusal);
    }

    let mut statement = conn
        .prepare_cached(
            "SELECT seq, network, kind, resource_id, state, recorded_at FROM event_log
             WHERE seq > ?1 ORDER BY seq",
        )
        .map_err(ops::storage("preparing a read of the store's history"))?;

    let rows = statement
        .query_map(params![position_as_sequence(after)?], read_event)
        .map_err(ops::storage("reading the store's history"))?;

    let mut events = Vec::new();
    for row in rows {
        let row = row.map_err(ops::storage("reading an event"))?;
        events.push(into_event(at.epoch(), row)?);
    }

    Ok(History::Resumed {
        events,
        cursor: at.clone(),
    })
}

/// One row of the event log, as the engine hands it over.
struct Row {
    /// The event's position.
    seq: i64,
    /// The network the resource belongs to.
    network: String,
    /// The kind, as stored.
    kind: String,
    /// The resource's identity within its network.
    resource_id: String,
    /// The post-state, or nothing for a deletion.
    state: Option<String>,
    /// When it was recorded.
    recorded_at: i64,
}

/// Read one row of the event log in the order the statements select.
fn read_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        seq: row.get(0)?,
        network: row.get(1)?,
        kind: row.get(2)?,
        resource_id: row.get(3)?,
        state: row.get(4)?,
        recorded_at: row.get(5)?,
    })
}

/// Turn a row of the log into the event a consumer sees.
fn into_event(epoch: &Epoch, row: Row) -> Result<Event> {
    Ok(Event {
        cursor: Cursor::from_row(epoch, row.seq)?,
        network: row.network,
        kind: EventKind::from_stored(&row.kind)?,
        resource_id: row.resource_id,
        state: row.state,
        recorded_at: Some(row.recorded_at),
    })
}

/// A presented position as the number a statement can compare.
///
/// The engine's integers are signed and a position is not, so a
/// position too large to be one of the engine's numbers is reported
/// rather than wrapped into something that would quietly match every
/// event in the log.
fn position_as_sequence(cursor: &Cursor) -> Result<i64> {
    i64::try_from(cursor.sequence()).map_err(|_| Error::Malformed {
        detail: format!(
            "a position of {} was presented, which no teneral ever issued",
            cursor.sequence()
        ),
    })
}

// ---------------------------------------------------------------------
// The snapshot.
//
// One statement per kind rather than one statement with five arms,
// because each resource's identity is built by the same function the
// write path uses to stamp it into an event -- so the two cannot drift
// and produce a snapshot a consumer would apply as a different
// resource. What is duplicated is the shape of each state, which is
// written in SQL twice: once where the event is appended, once here.
// That duplication is covered by a test comparing a snapshot's state
// with the state of the event the same write emitted, because a drift
// there would have a consumer applying two descriptions of one
// resource and believing both.
//
// Every statement selects the same four leading columns -- the state,
// the position, the network, and then whatever the identity is built
// from -- so the reader below can be one function.
// ---------------------------------------------------------------------

/// The network itself. Its name is both the network column and the
/// identity, which is why it appears twice.
const SNAPSHOT_NETWORK: &str = "SELECT json_object('name', name), cursor_seq, name, name
     FROM network WHERE name = ?1";

/// The interfaces the network has claimed.
const SNAPSHOT_ATTACHMENTS: &str =
    "SELECT json_object('network', network, 'namespace', namespace, 'ifname', ifname),
            cursor_seq, network, namespace, ifname
     FROM attachment WHERE network = ?1";

/// The ranges it owns.
const SNAPSHOT_RANGES: &str =
    "SELECT json_object('network', network, 'name', name, 'first', first_address,
                        'last', last_address),
            cursor_seq, network, name
     FROM address_range WHERE network = ?1";

/// The bindings configured in it.
const SNAPSHOT_BINDINGS: &str =
    "SELECT json_object('network', network, 'selector_kind', selector_kind,
                        'selector_value', selector_value, 'address', address,
                        'hostname', hostname),
            cursor_seq, network, selector_kind, selector_value
     FROM binding WHERE network = ?1";

/// The leases held in it, expired or not.
///
/// An expired lease is in the snapshot, saying what the row says. The
/// store evaluates expiry when a lease is *read as a lease*, and a
/// lease's expiry is not a change to it: nothing wrote anything when
/// the instant passed, so there is no event, and a snapshot that
/// dropped the row would tell a consumer the address had been given up
/// when the store still remembers who had it. The sweep is what turns
/// an expiry into a deletion a consumer hears about.
const SNAPSHOT_LEASES: &str =
    "SELECT json_object('network', network, 'address', address, 'holder_kind', holder_kind,
                        'holder_value', holder_value, 'lease_id', lease_id,
                        'acquired_at', acquired_at, 'expires_at', expires_at),
            cursor_seq, network, address
     FROM lease WHERE network = ?1";

/// One network's whole state, at the position the connection reads at.
pub(crate) fn snapshot(conn: &Connection, reading: &Reading<'_>, at: &Cursor) -> Result<Snapshot> {
    let mut events = Vec::new();

    collect(
        conn,
        reading,
        EventKind::Network,
        SNAPSHOT_NETWORK,
        identify_by_name,
        &mut events,
    )?;

    if events.is_empty() {
        // No network row, so no network. An empty snapshot would be a
        // consumer being told that this network is configured with
        // nothing, where the truth is that there is no such network --
        // and the consumer's next act on a complete snapshot is to
        // delete whatever the store did not mention.
        return Err(Error::UnknownNetwork {
            network: reading.network.to_owned(),
        });
    }

    collect(
        conn,
        reading,
        EventKind::Attachment,
        SNAPSHOT_ATTACHMENTS,
        identify_attachment,
        &mut events,
    )?;
    collect(
        conn,
        reading,
        EventKind::AddressRange,
        SNAPSHOT_RANGES,
        identify_by_name,
        &mut events,
    )?;
    collect(
        conn,
        reading,
        EventKind::Binding,
        SNAPSHOT_BINDINGS,
        identify_binding,
        &mut events,
    )?;
    collect(
        conn,
        reading,
        EventKind::Lease,
        SNAPSHOT_LEASES,
        identify_lease,
        &mut events,
    )?;

    // By position, which reads as the order the network was built in
    // and puts a parent ahead of its children: nothing here could have
    // been written before the network row it belongs to.
    events.sort_by_key(|event| event.cursor.sequence());

    Ok(Snapshot {
        events,
        cursor: at.clone(),
    })
}

/// Run one of the statements above and add what it returns to
/// `events`.
fn collect(
    conn: &Connection,
    reading: &Reading<'_>,
    kind: EventKind,
    sql: &str,
    identify: fn(&rusqlite::Row<'_>) -> Result<String>,
    events: &mut Vec<Event>,
) -> Result<()> {
    let mut statement = conn
        .prepare_cached(sql)
        .map_err(ops::storage("preparing a read of a network's state"))?;

    let mut rows = statement
        .query(params![reading.network])
        .map_err(ops::storage("reading a network's state"))?;

    while let Some(row) = rows
        .next()
        .map_err(ops::storage("reading part of a network's state"))?
    {
        let state: String = column(row, 0)?;
        let cursor_seq: i64 = column(row, 1)?;
        let network: String = column(row, 2)?;
        let resource_id = identify(row)?;

        reading.guard(&network, kind.resource(), || resource_id.clone())?;

        events.push(Event {
            cursor: Cursor::from_row(reading.epoch, cursor_seq)?,
            network,
            kind,
            resource_id,
            state: Some(state),
            // A resource row records where it was written, not when.
            // See `Event::recorded_at`.
            recorded_at: None,
        });
    }

    Ok(())
}

/// One column of a snapshot row, with the engine's error wrapped.
fn column<T: rusqlite::types::FromSql>(row: &rusqlite::Row<'_>, index: usize) -> Result<T> {
    row.get(index)
        .map_err(ops::storage("reading a column of a network's state"))
}

/// A resource identified by a name, which is the column after the
/// network.
fn identify_by_name(row: &rusqlite::Row<'_>) -> Result<String> {
    column(row, 3)
}

/// An attachment, identified by its namespace and interface name
/// through the same function the write path stamps into its event.
fn identify_attachment(row: &rusqlite::Row<'_>) -> Result<String> {
    let namespace: String = column(row, 3)?;
    let ifname: String = column(row, 4)?;

    Ok(ops::attachment_id(&namespace, &ifname))
}

/// A binding, identified by its selector.
fn identify_binding(row: &rusqlite::Row<'_>) -> Result<String> {
    let kind: String = column(row, 3)?;
    let value: String = column(row, 4)?;

    Ok(ops::binding_id(&Selector::from_row(&kind, value)?))
}

/// A lease, identified by its address.
fn identify_lease(row: &rusqlite::Row<'_>) -> Result<String> {
    let address: i64 = column(row, 3)?;

    Ok(lease::lease_id(ops::address_from_row(address)?))
}

// ---------------------------------------------------------------------
// The digest.
// ---------------------------------------------------------------------

/// The configured rows of one network, as the highest position any of
/// them was written at and how many there are.
///
/// Both halves are needed and neither alone would do. The position
/// alone would not move when a row was removed, and the count alone
/// would not move when a row was changed. Together they move on every
/// change to the set: a write takes a position higher than any in the
/// store, so writing or changing a row raises the highest, and
/// removing one lowers the count.
///
/// Leases are not in it, deliberately. See [`Digest`].
const DIGEST: &str = "SELECT coalesce(max(cursor_seq), 0), count(*) FROM (
         SELECT cursor_seq FROM network WHERE name = ?1
         UNION ALL SELECT cursor_seq FROM attachment WHERE network = ?1
         UNION ALL SELECT cursor_seq FROM address_range WHERE network = ?1
         UNION ALL SELECT cursor_seq FROM binding WHERE network = ?1
     )";

/// One network's digest, at the position the connection reads at.
///
/// No cross-network check, for the same reason reading a network by
/// name has none: this is an aggregate over rows the statement scoped
/// to one network, so there is no second value a check could compare
/// against the first. What makes it safe is the shape of the answer
/// rather than a comparison -- a digest says nothing about any
/// network, so one network's cannot be another's answer, and the
/// property that matters is tested directly: a change in one network
/// must not move the other's.
pub(crate) fn digest(conn: &Connection, network: &str) -> Result<Digest> {
    let (highest, rows): (i64, i64) = conn
        .query_row(DIGEST, params![network], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(ops::storage("reading a network's digest"))?;

    // The network's own row is one of the rows counted, so a count of
    // nothing is a network that is not there rather than one with
    // nothing configured in it. The two must not share an answer: a
    // consumer polling a digest it can no longer get is a consumer
    // whose network has been deleted, which is not the same news as
    // "your mirror is still correct".
    if rows == 0 {
        return Err(Error::UnknownNetwork {
            network: network.to_owned(),
        });
    }

    // The leading number is the construction's, so that a later change
    // to what goes into a digest is visible to somebody reading two of
    // them side by side rather than looking like a configuration that
    // changed. A consumer comparing them for equality needs no such
    // help -- any change of construction simply mismatches once, which
    // means a resync, which is a thing that happens anyway.
    Ok(Digest(format!("1:{highest}:{rows}")))
}

#[cfg(test)]
mod tests {
    use super::{EventKind, Retention};
    use crate::error::Error;

    #[test]
    fn a_history_bounded_at_zero_is_refused() {
        assert!(matches!(Retention::new(0, 60), Err(Error::Invalid { .. })));
        assert!(matches!(Retention::new(10, 0), Err(Error::Invalid { .. })));
        assert!(Retention::new(1, 1).is_ok());
    }

    #[test]
    fn every_kind_survives_being_stored_and_read_back() {
        // The log's `kind` column is written from one list and parsed
        // against the same one, so this is a tautology today. It is
        // here for the day somebody adds a variant and forgets the
        // string, which would otherwise be an event that no consumer
        // could be given and that nothing else would notice.
        for kind in EventKind::ALL {
            assert_eq!(EventKind::from_stored(kind.as_str()).unwrap(), kind);
        }

        assert!(matches!(
            EventKind::from_stored("zone"),
            Err(Error::Malformed { .. })
        ));
    }
}
