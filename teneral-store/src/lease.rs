//! Leases: who is using an address in a network right now, how that
//! changes, and which address a client is given when it has none.
//!
//! A lease is keyed by `(network, address)`. Within one network an
//! address has at most one holder at a time, which makes it a true
//! key; a client may legitimately hold two addresses at once while it
//! moves from one to the other, which makes the client not one. The
//! consequence is that "what does this client hold" is a query rather
//! than a key lookup, and it is a first class operation here rather
//! than a filter a caller applies to a list of everything -- that
//! question being the one the consumer this store replaces cannot ask
//! at all today.
//!
//! # The states, and what moves between them
//!
//! Three states, of which only two are rows:
//!
//! - **Unleased.** There is no row for `(network, address)`, so
//!   nothing holds the address.
//! - **Held.** There is a row whose expiry instant is still ahead of
//!   the clock. Exactly one client holds the address, and it is named
//!   by the row.
//! - **Expired.** There is a row whose expiry instant has passed.
//!   Nothing holds the address. This is not a stored state and no
//!   write produces it: it is the *same row*, read later.
//!
//! The transitions:
//!
//! | From | By | To |
//! |------|----|----|
//! | Unleased, Expired | acquiring, with the address named | Held, under a new lease id |
//! | Unleased, Expired | allocating, with the address chosen here | Held, under a new lease id |
//! | Held | allocating, where the holder already holds it | Held, same lease id, later expiry |
//! | Held | renewing | Held, same lease id, later expiry |
//! | Held, Expired | revoking | Unleased |
//! | Expired | sweeping | Unleased |
//!
//! Two transitions are deliberately absent. There is no way to move a
//! held address to a different holder in one step without saying so:
//! an acquisition against an address another client holds is refused
//! and names the holder, and taking it requires revoking the lease,
//! which is an operation a consumer has to ask for. And there is no
//! renewal of an expired lease. The address was free in the interim
//! and may have been given away, so the honest answer to a client
//! renewing something that has lapsed is that it must acquire again
//! -- which issues a new lease id, because the holding ended.
//!
//! # Expiry is absolute, and it is evaluated when a row is read
//!
//! A lease carries the instant it expires, not a duration and a
//! start. A row whose instant has passed is expired at the moment it
//! is read, by every read and by every allocation, whether or not
//! anything has swept it. That is the whole of the rule, and it is
//! what makes the sweep an observability mechanism rather than a
//! correctness one: nothing here waits for a sweeper, so a sweeper
//! that dies cannot cause a double allocation. The sweep's only job
//! is to turn expiries into events, so that a consumer watching the
//! history hears that an address was given up rather than inferring
//! it from a timestamp it was left to compare itself. Anything built
//! on this later that wants an expiry to *mean* something must not
//! make that meaning depend on the sweep having run.
//!
//! The boundary is that a lease is held while the clock is before its
//! instant and expired from the instant itself, which [`is_held`]
//! states once. Every statement in this module spells the same rule
//! as `expires_at > :now`, and the tests that cross the boundary
//! exercise both forms together.
//!
//! # Which attribute a lease is recorded against
//!
//! A client presents several attributes at once and a lease has one
//! holder, so one of them is recorded. Two different questions follow
//! and they have different answers.
//!
//! Recording a new acquisition uses the *highest-precedence*
//! attribute the client presented, by the same published order a
//! binding lookup walks. That is deliberate rather than convenient: a
//! client is then identified the same way whether it is being matched
//! against an operator's configuration or being given an address, and
//! the top of that order is not this project's choice but RFC 2131
//! section 4.2's -- where a client supplies a client identifier, the
//! server must identify it by that identifier.
//!
//! Asking whether an existing lease is *this* client's is the other
//! question, and it is answered by whether the client still presents
//! the attribute the lease was recorded against. A client that has
//! since begun presenting something of higher precedence has not
//! become a different client, and treating it as one would hand it a
//! second address while refusing it the first.

use std::net::Ipv4Addr;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::cursor::Cursor;
use crate::error::{Error, Result};
use crate::history::{self, EventKind, Retention};
use crate::identity::Epoch;
use crate::model::Lease;
use crate::ops::{self, LEASE, Reading, Writing};
use crate::selector::{ClientIdentity, Selector};

/// Whether a lease expiring at `expires_at` is still held at `now`.
///
/// Held while the clock is before the instant; expired from the
/// instant itself. The boundary is stated here once because it is
/// also written into every statement in this module as
/// `expires_at > :now`, and the two disagreeing by one would be an
/// address that is held and allocatable at the same time -- for one
/// second, in production, and never in a test that did not think to
/// land on the boundary.
pub(crate) fn is_held(expires_at: i64, now: i64) -> bool {
    now < expires_at
}

/// The columns every read of a lease selects, in one place so that
/// the row reader below cannot drift from them.
const COLUMNS: &str =
    "network, address, holder_kind, holder_value, lease_id, acquired_at, expires_at, cursor_seq";

/// One row of the lease table, as the engine hands it over.
///
/// A struct rather than an eight-element tuple: every field is a
/// number or a string, so a tuple would let two of them swap places
/// without a type error, and two of the numbers are an instant and a
/// position.
struct Row {
    /// The network the row belongs to, read back rather than taken
    /// from the question, so that the guard has something to compare.
    network: String,
    /// The address, as its 32-bit value.
    address: i64,
    /// The kind of the selector the holder was identified by.
    holder_kind: String,
    /// That selector's value.
    holder_value: String,
    /// The sequence at which the current holder acquired the address.
    lease_id: i64,
    /// When it acquired it.
    acquired_at: i64,
    /// When the lease runs out.
    expires_at: i64,
    /// Where the row's last write sits in the store's order.
    cursor_seq: i64,
}

/// Read one row of the lease table in the order [`COLUMNS`] names.
fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        network: row.get(0)?,
        address: row.get(1)?,
        holder_kind: row.get(2)?,
        holder_value: row.get(3)?,
        lease_id: row.get(4)?,
        acquired_at: row.get(5)?,
        expires_at: row.get(6)?,
        cursor_seq: row.get(7)?,
    })
}

/// Turn a row into the lease a caller sees, refusing one that came
/// from the wrong network.
///
/// The expiry is evaluated here, against `now`, which is what makes
/// "expired when read" a property of every read rather than something
/// each caller remembers to do.
fn into_lease(reading: &Reading<'_>, now: i64, row: Row) -> Result<Lease> {
    let address = ops::address_from_row(row.address)?;
    let holder = Selector::from_row(&row.holder_kind, row.holder_value)?;

    reading.guard(&row.network, LEASE, || lease_id(address))?;

    Ok(Lease {
        network: row.network,
        address,
        holder,
        lease_id: Cursor::from_row(reading.epoch, row.lease_id)?,
        acquired_at: row.acquired_at,
        expires_at: row.expires_at,
        expired: !is_held(row.expires_at, now),
        cursor: Cursor::from_row(reading.epoch, row.cursor_seq)?,
    })
}

/// A lease's identity as an event and an error report it: its
/// address, the network being named separately by both.
pub(crate) fn lease_id(address: Ipv4Addr) -> String {
    address.to_string()
}

/// What a refusal says about the lease standing in the way.
///
/// One sentence, built in one place, because three operations refuse
/// for this reason and an operator comparing their messages should
/// not have to work out whether they mean the same thing.
fn held_by(lease: &Lease) -> String {
    format!(
        "the client {} holds it under the lease {} until {}",
        lease.holder, lease.lease_id, lease.expires_at
    )
}

// ---------------------------------------------------------------------
// Reads.
//
// Each is answered from the connection it is given, so a connection
// inside a read transaction answers all of them at one position in the
// store's order -- and, separately, at the one instant the caller
// passes as `now`. Both halves matter for a lease: a view that
// answered one question before an expiry and the next after it would
// report two leases whose states could not have been true together.
// ---------------------------------------------------------------------

/// The lease on one address in one network, expired or not.
///
/// The row is returned whether or not it is still held, with
/// [`Lease::expired`] saying which. Hiding an expired row would hide
/// the thing an operator most needs to see -- an address that is free
/// and still remembered -- and would make the sweep's effect
/// invisible.
pub(crate) fn lease(
    conn: &Connection,
    reading: &Reading<'_>,
    now: i64,
    address: Ipv4Addr,
) -> Result<Option<Lease>> {
    let found = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM lease WHERE network = ?1 AND address = ?2"),
            params![reading.network, u32::from(address)],
            read_row,
        )
        .optional()
        .map_err(ops::storage("reading a lease"))?;

    match found {
        Some(row) => Ok(Some(into_lease(reading, now, row)?)),
        None => Ok(None),
    }
}

/// Every lease in one network, lowest address first.
pub(crate) fn leases(conn: &Connection, reading: &Reading<'_>, now: i64) -> Result<Vec<Lease>> {
    let mut statement = conn
        .prepare_cached(&format!(
            "SELECT {COLUMNS} FROM lease WHERE network = ?1 ORDER BY address"
        ))
        .map_err(ops::storage("preparing a read of a network's leases"))?;

    let rows = statement
        .query_map(params![reading.network], read_row)
        .map_err(ops::storage("reading a network's leases"))?;

    let mut leases = Vec::new();
    for row in rows {
        let row = row.map_err(ops::storage("reading a lease"))?;
        leases.push(into_lease(reading, now, row)?);
    }

    Ok(leases)
}

/// Every lease in one network held by a client presenting this
/// identity.
///
/// This is the query the resource model asks for by name, and the
/// reason a lease is keyed by address rather than by client: the
/// question has a list for an answer, because a client moving from
/// one address to another holds both for a moment. The answer is
/// ordered by the precedence of the attribute each lease was recorded
/// against and then by address, so a caller that wants one lease can
/// take the first and get the same one every time.
///
/// Expired leases are included, flagged. A client's lapsed lease is
/// how the store knows which address it used to have, which is a step
/// of the address selection order, so a query that hid it would hide
/// the input to that step.
pub(crate) fn leases_held_by(
    conn: &Connection,
    reading: &Reading<'_>,
    now: i64,
    identity: &ClientIdentity,
) -> Result<Vec<Lease>> {
    let mut statement = conn
        .prepare_cached(&format!(
            "SELECT {COLUMNS} FROM lease
             WHERE network = ?1 AND holder_kind = ?2 AND holder_value = ?3
             ORDER BY address"
        ))
        .map_err(ops::storage("preparing a read of what a client holds"))?;

    let mut held = Vec::new();
    for selector in identity.selectors() {
        let rows = statement
            .query_map(
                params![reading.network, selector.kind().as_str(), selector.value()],
                read_row,
            )
            .map_err(ops::storage("reading what a client holds"))?;

        for row in rows {
            let row = row.map_err(ops::storage("reading a lease"))?;
            held.push(into_lease(reading, now, row)?);
        }
    }

    Ok(held)
}

/// The live lease on an address, or nothing if nothing holds it.
///
/// A read of its own rather than [`lease`] with a test applied
/// afterwards, because the statement itself is where the expiry rule
/// has to be for the question "is this address free" to be answered
/// by the engine rather than by the caller's memory of it.
fn live_lease(
    conn: &Connection,
    reading: &Reading<'_>,
    now: i64,
    address: Ipv4Addr,
) -> Result<Option<Lease>> {
    let found = conn
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM lease
                 WHERE network = ?1 AND address = ?2 AND expires_at > ?3"
            ),
            params![reading.network, u32::from(address), now],
            read_row,
        )
        .optional()
        .map_err(ops::storage("reading the lease holding an address"))?;

    match found {
        Some(row) => Ok(Some(into_lease(reading, now, row)?)),
        None => Ok(None),
    }
}

/// Whether a client presenting `identity` is the holder of `lease`.
///
/// By whether it still presents the attribute the lease was recorded
/// against, for the reason in this module's documentation: a client
/// that has begun presenting something of higher precedence has not
/// become a different client.
fn holds(identity: &ClientIdentity, lease: &Lease) -> bool {
    identity.presents(lease.holder.kind()) == Some(lease.holder.value())
}

/// The attribute a new acquisition is recorded against: the
/// highest-precedence one the client presents.
///
/// A client presenting nothing at all is refused rather than given a
/// lease nothing could ever find again. That is a real case -- a
/// lookup for an identity with no attributes is how a caller asks
/// what a completely unknown client would be answered -- and the
/// difference is that a lookup can answer "nothing" while a lease
/// cannot be held by nobody.
fn holder_of(identity: &ClientIdentity) -> Result<Selector> {
    identity
        .selectors()
        .into_iter()
        .next()
        .ok_or_else(|| Error::Invalid {
            what: "client identity",
            detail: "it presents nothing, so there is no attribute a lease could be recorded \
                     against"
                .to_owned(),
        })
}

/// The instant a lease taken now for `duration` seconds runs out.
///
/// A duration of zero or less is refused rather than stored: it would
/// write a lease that is expired from the instant it is committed,
/// which reads as a client holding an address and as an address being
/// free at the same time.
fn expiry(now: i64, duration: i64) -> Result<i64> {
    if duration < 1 {
        return Err(Error::Invalid {
            what: "lease duration",
            detail: format!("{duration} seconds would expire before it was taken"),
        });
    }

    now.checked_add(duration).ok_or_else(|| Error::Invalid {
        what: "lease duration",
        detail: format!("{duration} seconds from {now} is past the end of time"),
    })
}

// ---------------------------------------------------------------------
// The event, and the row.
// ---------------------------------------------------------------------

/// A lease row as it is about to be written.
///
/// Gathered into a struct because the event and the row are written
/// from the same values, and a signature repeating them would let a
/// caller stamp one with the lease it is replacing and the other with
/// the lease it is writing.
struct Writable {
    /// The address being leased.
    address: Ipv4Addr,
    /// The attribute the holder is recorded against.
    holder: Selector,
    /// The sequence at which the current holder acquired the address,
    /// or `None` when *this* write is that acquisition.
    ///
    /// It is `None` rather than a number in that case because the id
    /// is the position of the event this write is about to append, so
    /// it does not exist until the event does. Writing it down as an
    /// absence rather than as a zero is what stops a zero reaching a
    /// row or a payload if a step between here and the event is ever
    /// reordered.
    lease_id: Option<i64>,
    /// When the current holder acquired it.
    acquired_at: i64,
    /// When the lease runs out.
    expires_at: i64,
}

/// A lease event that has been appended, and so has a position.
struct Appended {
    /// The event's own position, which is the cursor the write will
    /// be reported at and the position the row records.
    sequence: i64,
    /// The lease id the row is to carry: the one it already had for a
    /// renewal, and this event's own position for an acquisition.
    lease_id: i64,
}

/// Append the event for a lease that now exists in this form.
///
/// The lease id is in the payload as well as in the row, because it
/// is the field a consumer compares to tell a renewal from a
/// reassignment, and an event carrying the whole post-state except
/// the one field the state is read for would make every consumer
/// fetch the lease again.
///
/// Which is why an acquisition writes its payload twice. The id of a
/// holding is the position of the event that begins it, so the event
/// has to exist before it can say what the id is; the second
/// statement fills it in. Both are inside the caller's transaction,
/// so no reader ever sees the state without it -- and the alternative
/// would be to predict the next sequence number from the engine's
/// own counter, which is a guess about an implementation detail
/// standing where an identity should be.
fn event_lease_written(
    transaction: &Transaction<'_>,
    bound: &Retention,
    network: &str,
    writable: &Writable,
    now: i64,
) -> Result<Appended> {
    transaction
        .execute(
            "INSERT INTO event_log (network, kind, resource_id, state, recorded_at)
             VALUES (?1, ?2, ?3,
                     json_object('network', ?1, 'address', ?4, 'holder_kind', ?5,
                                 'holder_value', ?6, 'lease_id', ?7,
                                 'acquired_at', ?8, 'expires_at', ?9), ?10)",
            params![
                network,
                EventKind::Lease.as_str(),
                lease_id(writable.address),
                u32::from(writable.address),
                writable.holder.kind().as_str(),
                writable.holder.value(),
                writable.lease_id,
                writable.acquired_at,
                writable.expires_at,
                now
            ],
        )
        .map_err(ops::storage("recording a lease"))?;

    let sequence = history::appended(transaction, bound, now)?;

    let lease_id = match writable.lease_id {
        Some(existing) => existing,
        None => {
            transaction
                .execute(
                    // The one parameter appears twice, which is what
                    // this statement is for: the id of a holding is
                    // the position of the event that begins it.
                    "UPDATE event_log SET state = json_set(state, '$.lease_id', ?1)
                      WHERE seq = ?1",
                    params![sequence],
                )
                .map_err(ops::storage("recording the identity of a new lease"))?;

            sequence
        }
    };

    Ok(Appended { sequence, lease_id })
}

/// Write the row, replacing the one already on the address if there
/// is one.
///
/// `replacing` is passed rather than discovered, because every caller
/// has already read the row in order to decide what to do about it,
/// and a second read to find out whether to insert or update would be
/// a read that could disagree with the first.
fn write_row(
    transaction: &Transaction<'_>,
    network: &str,
    writable: &Writable,
    appended: &Appended,
    replacing: bool,
) -> Result<()> {
    let written = if replacing {
        transaction.execute(
            "UPDATE lease
                SET holder_kind = ?3, holder_value = ?4, lease_id = ?5,
                    acquired_at = ?6, expires_at = ?7, cursor_seq = ?8
              WHERE network = ?1 AND address = ?2",
            params![
                network,
                u32::from(writable.address),
                writable.holder.kind().as_str(),
                writable.holder.value(),
                appended.lease_id,
                writable.acquired_at,
                writable.expires_at,
                appended.sequence
            ],
        )
    } else {
        transaction.execute(
            "INSERT INTO lease
                 (network, address, holder_kind, holder_value, lease_id,
                  acquired_at, expires_at, cursor_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                network,
                u32::from(writable.address),
                writable.holder.kind().as_str(),
                writable.holder.value(),
                appended.lease_id,
                writable.acquired_at,
                writable.expires_at,
                appended.sequence
            ],
        )
    };

    written.map_err(ops::storage("writing a lease"))?;

    Ok(())
}

/// The lease a write just made, built from the values it wrote.
///
/// Built from those values rather than read back, and built by the
/// caller only after its transaction has committed. Both halves are
/// the durability point: the caller is handed a lease it may
/// acknowledge to a client, and it is handed it after the commit that
/// made the lease survive a power cut, never before.
fn written(
    network: &str,
    writable: &Writable,
    appended: &Appended,
    epoch: &Epoch,
) -> Result<Lease> {
    Ok(Lease {
        network: network.to_owned(),
        address: writable.address,
        holder: writable.holder.clone(),
        lease_id: Cursor::from_row(epoch, appended.lease_id)?,
        acquired_at: writable.acquired_at,
        expires_at: writable.expires_at,
        // A lease just written for a positive duration is held by
        // definition. Saying so from the values rather than from a
        // second reading of the clock keeps a write's answer
        // consistent with the instant the write itself used.
        expired: false,
        cursor: Cursor::from_row(epoch, appended.sequence)?,
    })
}

/// A lease's id as the column holds it.
///
/// The id is half of a cursor, so it comes out of the model as one
/// and has to go back as a number. A value that will not fit means
/// the column has been written by something other than this code,
/// which is worth a complaint rather than a truncation.
fn stored_id(lease: &Lease) -> Result<i64> {
    i64::try_from(lease.lease_id.sequence()).map_err(|_| Error::Malformed {
        detail: format!(
            "it records a lease id of {}, which no teneral ever issued",
            lease.lease_id
        ),
    })
}

// ---------------------------------------------------------------------
// Writes.
// ---------------------------------------------------------------------

/// Give a client the address it asked to be given, or refuse and name
/// what is in the way.
///
/// This is the path for an address somebody else chose: a reservation
/// an operator configured, or an address a consumer allocates
/// elsewhere and wants the store to agree with. It therefore does
/// *not* require the address to fall inside one of the network's
/// ranges -- a network with no range at all is an ordinary
/// configuration here, and a reservation outside a pool is how an
/// operator holds an address the pool can never hand out.
///
/// A live lease held by this client is renewed in place and keeps its
/// lease id. An expired one is replaced, and the replacement gets a
/// new id, because the address was free in between and the holding
/// that ended is not the holding that begins. One held by anybody
/// else is refused, naming the holder: moving an address between
/// clients is something a caller has to ask for, through a revocation.
pub(crate) fn acquire_lease(
    conn: &mut Connection,
    writing: &Writing<'_>,
    network: &str,
    identity: &ClientIdentity,
    address: Ipv4Addr,
    duration: i64,
) -> Result<(Cursor, Lease)> {
    let now = writing.clock.unix_seconds();
    let expires_at = expiry(now, duration)?;
    let holder = holder_of(identity)?;

    let transaction = conn
        .transaction()
        .map_err(ops::storage("starting a transaction to take a lease"))?;

    ops::require_network(&transaction, network)?;

    let reading = writing.reading(network);
    let current = lease(&transaction, &reading, now, address)?;
    let replacing = current.is_some();
    let writable = plan_acquisition(identity, &holder, current, address, now, expires_at)?;

    let appended = event_lease_written(&transaction, writing.retention, network, &writable, now)?;
    write_row(&transaction, network, &writable, &appended, replacing)?;

    transaction
        .commit()
        .map_err(ops::storage("committing a lease"))?;

    // After the commit, never before. A caller holding the lease any
    // earlier could acknowledge an address to a client that a power
    // cut would then take back -- and the store would hand the same
    // address to somebody else while the first client went on using
    // it.
    let lease = written(network, &writable, &appended, writing.epoch)?;
    let at = Cursor::new(
        writing.epoch.clone(),
        ops::sequence_as_position(appended.sequence)?,
    );

    Ok((at, lease))
}

/// What to write for an acquisition of `address`, given whatever is
/// there now.
///
/// Separated from the statements so that the three cases are visible
/// together: a renewal of this client's own live lease, a fresh
/// holding of an address that is free or whose lease has lapsed, and
/// a refusal.
fn plan_acquisition(
    identity: &ClientIdentity,
    holder: &Selector,
    current: Option<Lease>,
    address: Ipv4Addr,
    now: i64,
    expires_at: i64,
) -> Result<Writable> {
    match current {
        // Held, by this client: a renewal. The lease id and the
        // instant it was acquired both stay, because the holder has
        // not changed -- which is the whole of what a consumer reads
        // a lease id for.
        Some(lease) if !lease.expired && holds(identity, &lease) => Ok(Writable {
            address,
            lease_id: Some(stored_id(&lease)?),
            acquired_at: lease.acquired_at,
            holder: lease.holder,
            expires_at,
        }),

        // Held, by somebody else.
        Some(lease) if !lease.expired => Err(Error::Conflict {
            kind: LEASE,
            id: lease_id(address),
            held_by: held_by(&lease),
        }),

        // Expired, or nothing there at all. Either way nothing holds
        // the address and this is a new holding, so the lease id is
        // the position of the event about to be appended. It is not
        // known yet, which is why it is filled in below.
        _ => Ok(Writable {
            address,
            holder: holder.clone(),
            lease_id: None,
            acquired_at: now,
            expires_at,
        }),
    }
}

/// Extend the lease a client already holds, keeping its lease id.
///
/// The compare-and-set here is on identity rather than on a cursor,
/// which is the form the specification asks for and the only one the
/// caller can satisfy: a renewal arrives as a client's request on the
/// wire, carrying what the client presents and no position in the
/// store's order. So the refusals are the two that an identity can
/// decide. An address held by another client is refused and names it.
/// An address whose lease has lapsed is refused as having no live
/// lease on it, because the address was free in the interim and may
/// have been given away -- a client in that position has to acquire
/// again, and acquiring is what issues a new lease id.
pub(crate) fn renew_lease(
    conn: &mut Connection,
    writing: &Writing<'_>,
    network: &str,
    address: Ipv4Addr,
    identity: &ClientIdentity,
    duration: i64,
) -> Result<Cursor> {
    let now = writing.clock.unix_seconds();
    let expires_at = expiry(now, duration)?;

    let transaction = conn
        .transaction()
        .map_err(ops::storage("starting a transaction to renew a lease"))?;

    let reading = writing.reading(network);
    let current =
        live_lease(&transaction, &reading, now, address)?.ok_or_else(|| Error::NotFound {
            kind: LEASE,
            id: format!("a live lease on {address} in the network {network}"),
        })?;

    if !holds(identity, &current) {
        return Err(Error::Conflict {
            kind: LEASE,
            id: lease_id(address),
            held_by: held_by(&current),
        });
    }

    let writable = Writable {
        address,
        lease_id: Some(stored_id(&current)?),
        acquired_at: current.acquired_at,
        holder: current.holder,
        expires_at,
    };

    let appended = event_lease_written(&transaction, writing.retention, network, &writable, now)?;
    write_row(&transaction, network, &writable, &appended, true)?;

    transaction
        .commit()
        .map_err(ops::storage("committing a renewed lease"))?;

    Ok(Cursor::new(
        writing.epoch.clone(),
        ops::sequence_as_position(appended.sequence)?,
    ))
}

/// Take a lease away, whether it is still held or has lapsed.
///
/// One operation for two callers, because they are the same write: an
/// operator taking an address back, and a client giving one up. The
/// compare-and-set is on the position the caller read the lease at,
/// which both of them have -- an operator revoking read the lease in
/// order to decide to, and a front-end handling a release read it in
/// order to check who was releasing it.
///
/// A lapsed lease may be revoked. Nothing holds the address, but the
/// row is still there, and removing it is how a caller says so
/// deliberately rather than waiting for a sweep.
pub(crate) fn revoke_lease(
    conn: &mut Connection,
    writing: &Writing<'_>,
    network: &str,
    address: Ipv4Addr,
    expected: &Cursor,
) -> Result<Cursor> {
    let now = writing.clock.unix_seconds();

    let transaction = conn
        .transaction()
        .map_err(ops::storage("starting a transaction to revoke a lease"))?;

    let reading = writing.reading(network);
    let current = lease(&transaction, &reading, now, address)?.ok_or_else(|| Error::NotFound {
        kind: LEASE,
        id: format!("{address} in the network {network}"),
    })?;

    if &current.cursor != expected {
        return Err(Error::changed(
            LEASE,
            lease_id(address),
            expected,
            &current.cursor,
        ));
    }

    let sequence = remove(&transaction, writing.retention, network, address, now)?;

    transaction
        .commit()
        .map_err(ops::storage("committing the revocation of a lease"))?;

    Ok(Cursor::new(
        writing.epoch.clone(),
        ops::sequence_as_position(sequence)?,
    ))
}

/// Remove one lease row and record its absence, inside a caller's
/// transaction.
fn remove(
    transaction: &Transaction<'_>,
    bound: &Retention,
    network: &str,
    address: Ipv4Addr,
    now: i64,
) -> Result<i64> {
    let sequence = ops::event_deleted(
        transaction,
        bound,
        network,
        EventKind::Lease,
        &lease_id(address),
        now,
    )?;

    transaction
        .execute(
            "DELETE FROM lease WHERE network = ?1 AND address = ?2",
            params![network, u32::from(address)],
        )
        .map_err(ops::storage("removing a lease"))?;

    Ok(sequence)
}

/// Record the expiry of every lease in a network whose instant has
/// passed, and remove the rows.
///
/// This exists for one reason, and naming it is as important as the
/// code: a consumer watching the history has to be *told* that an
/// address was given up, rather than being left to compare an expiry
/// instant against its own clock and work it out. Nothing in this
/// store waits for it. Every read evaluates expiry itself and every
/// allocation refuses to hand out an address whose lease is still
/// live, so a sweeper that never runs leaves the store's answers
/// correct and its history quiet -- and a sweeper that dies cannot
/// cause a double allocation. Nothing written later should come to
/// depend on this having run.
///
/// It returns the position of the last event it wrote, or `None` when
/// nothing had expired: a sweep that removed nothing wrote nothing,
/// and reporting a position for it would invent one.
///
/// One consequence is worth having in view before a scheduler is
/// written for this. Removing the row is what makes the event a
/// deletion, and the row is also where the store remembers which
/// address a client used to have -- the second step of the address
/// selection order. So the interval a sweeper runs at is also how
/// long a client keeps getting its old address back after a lapse,
/// and the two cannot be set independently without keeping the rows
/// and marking them instead.
pub(crate) fn sweep_expired_leases(
    conn: &mut Connection,
    writing: &Writing<'_>,
    network: &str,
) -> Result<(Option<Cursor>, Vec<Lease>)> {
    let now = writing.clock.unix_seconds();

    let transaction = conn
        .transaction()
        .map_err(ops::storage("starting a transaction to sweep leases"))?;

    let reading = writing.reading(network);
    let expired = {
        let mut statement = transaction
            .prepare_cached(&format!(
                "SELECT {COLUMNS} FROM lease
                 WHERE network = ?1 AND expires_at <= ?2 ORDER BY address"
            ))
            .map_err(ops::storage("preparing a sweep of expired leases"))?;

        let rows = statement
            .query_map(params![network, now], read_row)
            .map_err(ops::storage("reading the expired leases"))?;

        let mut expired = Vec::new();
        for row in rows {
            let row = row.map_err(ops::storage("reading an expired lease"))?;
            expired.push(into_lease(&reading, now, row)?);
        }
        expired
    };

    let mut last = None;
    for lease in &expired {
        last = Some(remove(
            &transaction,
            writing.retention,
            network,
            lease.address,
            now,
        )?);
    }

    transaction
        .commit()
        .map_err(ops::storage("committing a sweep of expired leases"))?;

    let at = last
        .map(|sequence| -> Result<Cursor> {
            Ok(Cursor::new(
                writing.epoch.clone(),
                ops::sequence_as_position(sequence)?,
            ))
        })
        .transpose()?;

    Ok((at, expired))
}

/// Give a client an address this store chose.
///
/// The order the candidates are considered in is RFC 2131 section
/// 4.3.1's, which specifies four steps and specifies that they are
/// tried in turn:
///
/// 1. **The address the client already holds**, as recorded in its
///    current lease. Renewed in place, keeping its lease id.
/// 2. **The address it held until its lease lapsed**, if that address
///    is in one of this network's ranges and nothing holds it now.
///    Acquired afresh, with a new lease id, because the holding that
///    ended is not the one that begins.
/// 3. **The address it asked for**, if that address is in one of this
///    network's ranges and nothing holds it.
/// 4. **An address from the pool.**
///
/// Two readings of that section are this project's and are marked as
/// such rather than attributed to the RFC. The specification says the
/// requested address must be "valid", which is read here as falling
/// inside one of the ranges the network owns, there being nothing
/// else in this store for it to be valid against. And the
/// specification says nothing whatever about *which* address the
/// fourth step picks: the choice of the lowest free one is teneral's,
/// made because it is deterministic, cheap to express as a statement
/// over an integer column, and reproducible in a test, where a
/// counter or a hash of the client's identity would be none of those.
///
/// Two addresses are not available to steps 2, 3 and 4 even though
/// nothing holds them: one inside no range, and one an operator has
/// configured a binding for. The second is not in the specification
/// either, and it is teneral's for a plain reason -- an address
/// reserved for a machine that happens to be switched off is an
/// address the pool must not hand to somebody else, or the reservation
/// becomes a conflict the moment the machine comes back.
///
/// A network with no range at all allocates nothing, which is not a
/// failure of this store but a description of the commonest
/// deployment it serves: a network whose addresses are all chosen
/// elsewhere and asserted through [`acquire_lease`].
pub(crate) fn allocate_lease(
    conn: &mut Connection,
    writing: &Writing<'_>,
    network: &str,
    identity: &ClientIdentity,
    requested: Option<Ipv4Addr>,
    duration: i64,
) -> Result<(Cursor, Lease)> {
    let now = writing.clock.unix_seconds();
    let expires_at = expiry(now, duration)?;
    let holder = holder_of(identity)?;

    let transaction = conn.transaction().map_err(ops::storage(
        "starting a transaction to allocate an address",
    ))?;

    ops::require_network(&transaction, network)?;

    let reading = writing.reading(network);
    let chosen = choose_address(&transaction, &reading, now, identity, requested)?;

    let (writable, replacing) = match chosen {
        // The client's current lease: renewed in place, which is the
        // same plan an acquisition of the same address would make.
        // Taking the same path rather than a second one is what keeps
        // "a renewal keeps the lease id" one rule instead of two.
        Chosen::Held(lease) => {
            let address = lease.address;
            (
                plan_acquisition(identity, &holder, Some(lease), address, now, expires_at)?,
                true,
            )
        }
        Chosen::Previous(address) => (
            Writable {
                address,
                holder: holder.clone(),
                lease_id: None,
                acquired_at: now,
                expires_at,
            },
            true,
        ),
        Chosen::Free(address) => {
            let replacing = lease(&transaction, &reading, now, address)?.is_some();
            (
                Writable {
                    address,
                    holder: holder.clone(),
                    lease_id: None,
                    acquired_at: now,
                    expires_at,
                },
                replacing,
            )
        }
    };

    let appended = event_lease_written(&transaction, writing.retention, network, &writable, now)?;
    write_row(&transaction, network, &writable, &appended, replacing)?;

    transaction
        .commit()
        .map_err(ops::storage("committing an allocated address"))?;

    // After the commit, for the reason in `acquire_lease`.
    let lease = written(network, &writable, &appended, writing.epoch)?;
    let at = Cursor::new(
        writing.epoch.clone(),
        ops::sequence_as_position(appended.sequence)?,
    );

    Ok((at, lease))
}

/// Which step of the selection order answered, and with what.
///
/// An enum rather than an address alone, because the first step is a
/// renewal and the others are acquisitions, and the difference is the
/// lease id.
enum Chosen {
    /// The client's own live lease, to be renewed in place.
    Held(Lease),
    /// An address the client held until its lease lapsed. The row is
    /// still there, so writing it is a replacement.
    Previous(Ipv4Addr),
    /// An address nothing holds, which may or may not have a lapsed
    /// row on it.
    Free(Ipv4Addr),
}

/// Walk the selection order and return the first step that answers.
///
/// The order is the whole of this function and is deliberately a
/// sequence of early returns rather than a sort or a scored
/// comparison: a reader checking it against the specification should
/// be able to read the steps top to bottom, and a step inserted in
/// the wrong place should be visible as a misplaced paragraph rather
/// than as a changed number.
fn choose_address(
    transaction: &Transaction<'_>,
    reading: &Reading<'_>,
    now: i64,
    identity: &ClientIdentity,
    requested: Option<Ipv4Addr>,
) -> Result<Chosen> {
    // 1. The address the client already holds.
    if let Some(held) = held_by_client(transaction, reading, now, identity)? {
        return Ok(Chosen::Held(held));
    }

    // 2. The address it held until its lease lapsed.
    if let Some(previous) = previously_held(transaction, reading, now, identity)? {
        return Ok(Chosen::Previous(previous));
    }

    // 3. The address it asked for.
    if let Some(wanted) = requested
        && available(transaction, reading, now, wanted)?
    {
        return Ok(Chosen::Free(wanted));
    }

    // 4. The lowest free address in the network's ranges.
    match lowest_free(transaction, reading, now)? {
        Some(address) => Ok(Chosen::Free(address)),
        None => Err(Error::NoAddress {
            network: reading.network.to_owned(),
            detail: exhaustion(transaction, reading)?,
        }),
    }
}

/// Why a network had nothing to allocate, as a sentence.
///
/// Two causes, with different fixes, which is why they are told
/// apart: a network with no range allocates nothing by design and the
/// answer is usually that the caller wanted to assert an address
/// rather than ask for one, while a network whose range is full is an
/// operational problem with an operational fix.
fn exhaustion(transaction: &Transaction<'_>, reading: &Reading<'_>) -> Result<String> {
    let ranges: i64 = transaction
        .query_row(
            "SELECT count(*) FROM address_range WHERE network = ?1",
            params![reading.network],
            |row| row.get(0),
        )
        .map_err(ops::storage("counting a network's ranges"))?;

    Ok(if ranges == 0 {
        "it has no address range, so every address in it is one somebody else chose".to_owned()
    } else {
        format!("every address in its {ranges} range(s) is either leased or reserved by a binding")
    })
}

/// The live lease this client holds, if it holds one.
///
/// Walks the client's attributes in precedence order and takes the
/// lowest address under the first that holds anything, which makes
/// the answer the same every time for a client holding two addresses
/// at once.
fn held_by_client(
    conn: &Connection,
    reading: &Reading<'_>,
    now: i64,
    identity: &ClientIdentity,
) -> Result<Option<Lease>> {
    for selector in identity.selectors() {
        let found = conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM lease
                     WHERE network = ?1 AND holder_kind = ?2 AND holder_value = ?3
                       AND expires_at > ?4
                     ORDER BY address LIMIT 1"
                ),
                params![
                    reading.network,
                    selector.kind().as_str(),
                    selector.value(),
                    now
                ],
                read_row,
            )
            .optional()
            .map_err(ops::storage("reading the lease a client holds"))?;

        if let Some(row) = found {
            return Ok(Some(into_lease(reading, now, row)?));
        }
    }

    Ok(None)
}

/// The address this client held until its lease lapsed, if that
/// address is still allocatable.
///
/// Allocatable means inside one of the network's ranges and not
/// reserved by a binding. It cannot be held by a live lease: the row
/// found here *is* the lease on that address, the key being the
/// network and the address, and it has expired.
fn previously_held(
    conn: &Connection,
    reading: &Reading<'_>,
    now: i64,
    identity: &ClientIdentity,
) -> Result<Option<Ipv4Addr>> {
    for selector in identity.selectors() {
        let found: Option<i64> = conn
            .query_row(
                "SELECT address FROM lease
                  WHERE network = ?1 AND holder_kind = ?2 AND holder_value = ?3
                    AND expires_at <= ?4
                    AND EXISTS (SELECT 1 FROM address_range AS r
                                 WHERE r.network = ?1
                                   AND lease.address BETWEEN r.first_address AND r.last_address)
                    AND NOT EXISTS (SELECT 1 FROM binding AS b
                                     WHERE b.network = ?1 AND b.address = lease.address)
                  ORDER BY address LIMIT 1",
                params![
                    reading.network,
                    selector.kind().as_str(),
                    selector.value(),
                    now
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(ops::storage("reading the address a client used to hold"))?;

        if let Some(address) = found {
            return Ok(Some(ops::address_from_row(address)?));
        }
    }

    Ok(None)
}

/// Whether an address can be allocated: inside a range, not held, not
/// reserved.
fn available(
    conn: &Connection,
    reading: &Reading<'_>,
    now: i64,
    address: Ipv4Addr,
) -> Result<bool> {
    let free: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM address_range
              WHERE network = ?1 AND ?2 BETWEEN first_address AND last_address
                AND NOT EXISTS (SELECT 1 FROM lease AS l
                                 WHERE l.network = ?1 AND l.address = ?2
                                   AND l.expires_at > ?3)
                AND NOT EXISTS (SELECT 1 FROM binding AS b
                                 WHERE b.network = ?1 AND b.address = ?2)
              LIMIT 1",
            params![reading.network, u32::from(address), now],
            |row| row.get(0),
        )
        .optional()
        .map_err(ops::storage("checking whether an address can be allocated"))?;

    Ok(free.is_some())
}

/// The lowest address in any of the network's ranges that nothing
/// holds and no binding reserves.
///
/// The lowest free address is teneral's tie-break and not the
/// specification's, which says nothing about which address a pool
/// offers. It is a statement rather than a walk for the reason the
/// addresses are integers at all: the candidates are each either the
/// bottom of a range or one above something taken, so the answer is a
/// minimum over a set whose size is the number of addresses in use
/// rather than the number in the range. A range of a million
/// addresses with three leases in it costs four candidates, not a
/// million.
fn lowest_free(conn: &Connection, reading: &Reading<'_>, now: i64) -> Result<Option<Ipv4Addr>> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT min(candidate) FROM (
                 SELECT first_address AS candidate FROM address_range WHERE network = ?1
                 UNION
                 SELECT address + 1 AS candidate FROM lease
                  WHERE network = ?1 AND expires_at > ?2
                 UNION
                 SELECT address + 1 AS candidate FROM binding WHERE network = ?1
             )
             WHERE EXISTS (SELECT 1 FROM address_range AS r
                            WHERE r.network = ?1
                              AND candidate BETWEEN r.first_address AND r.last_address)
               AND NOT EXISTS (SELECT 1 FROM lease AS l
                                WHERE l.network = ?1 AND l.address = candidate
                                  AND l.expires_at > ?2)
               AND NOT EXISTS (SELECT 1 FROM binding AS b
                                WHERE b.network = ?1 AND b.address = candidate)",
            params![reading.network, now],
            |row| row.get(0),
        )
        .map_err(ops::storage("looking for the lowest free address"))?;

    match found {
        Some(address) => Ok(Some(ops::address_from_row(address)?)),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------
// What the binding writes need from here.
//
// A binding and a lease both say what an address is for, so the one
// conflict between them that the store can decide has to be decided in
// the write that would create it. These two functions are that, and
// they live here rather than beside the binding statements because
// both of them are about a lease.
// ---------------------------------------------------------------------

/// Refuse a binding against an address a lease currently holds.
///
/// This is the first of the two compare-and-set operations the API
/// asks for -- "create this binding only if no lease holds the
/// address" -- expressed as a property of the write rather than as a
/// second method beside it. A caller cannot get the unchecked
/// version, which matters: the two writes would be one safe and one
/// not, and the unsafe one would be the one with the shorter name.
///
/// It is a conflict rather than a warning because the alternative is
/// two answers for one address, which is the shape of every failure
/// this store is built to prevent. The refusal names the lease and the
/// holder, as the API requires, so that an operator can decide whether
/// to take it -- and [`revoke_for_binding`] is how they say they have.
///
/// An expired lease does not stand in the way. Nothing holds the
/// address, and refusing on the strength of a row a sweep has not
/// reached yet would make a binding's acceptance depend on a sweeper,
/// which is the dependency this module exists not to have.
pub(crate) fn require_address_unleased(
    transaction: &Transaction<'_>,
    writing: &Writing<'_>,
    network: &str,
    selector: &Selector,
    address: Ipv4Addr,
) -> Result<()> {
    let now = writing.clock.unix_seconds();
    let reading = writing.reading(network);

    match live_lease(transaction, &reading, now, address)? {
        Some(lease) => Err(Error::Conflict {
            kind: ops::BINDING,
            id: ops::binding_id(selector),
            held_by: format!(
                "the address {address} is leased in the network {network}: {}. Revoke that lease \
                 as part of this write, or configure a different address",
                held_by(&lease)
            ),
        }),
        None => Ok(()),
    }
}

/// Revoke the lease a caller named, inside the transaction that is
/// about to write a binding.
///
/// This is the second compare-and-set operation the API asks for --
/// "revoke that lease and create this binding" -- and the whole point
/// of it being one operation is in this function's being called from
/// inside the binding's own transaction. A refusal of the binding
/// rolls the revocation back with it, so there is no state in which
/// the lease was taken away and the binding that justified taking it
/// was not written.
///
/// `expected` is the lease id, not the position of the lease's last
/// write, and that is the right token here for a reason worth stating.
/// The caller reached this operation by being refused and told which
/// lease was in the way; what it is asserting is that the lease it was
/// told about is the one it means to take. A lease id changes when and
/// only when the address changes holder, so a renewal in between does
/// not invalidate the instruction and a reassignment does -- which is
/// exactly the distinction an operator taking an address from a named
/// client cares about.
///
/// A lapsed lease may be revoked this way. Nothing holds the address,
/// so the plain write would have been accepted; doing what the caller
/// asked rather than refusing it keeps the row from being left behind
/// for a sweep to announce after the binding that replaced it.
pub(crate) fn revoke_for_binding(
    transaction: &Transaction<'_>,
    writing: &Writing<'_>,
    network: &str,
    address: Ipv4Addr,
    expected: &Cursor,
) -> Result<()> {
    let now = writing.clock.unix_seconds();
    let reading = writing.reading(network);

    let current = lease(transaction, &reading, now, address)?.ok_or_else(|| Error::NotFound {
        kind: LEASE,
        id: format!("{address} in the network {network}, which this write was to revoke"),
    })?;

    if &current.lease_id != expected {
        return Err(Error::changed(
            LEASE,
            lease_id(address),
            expected,
            &current.lease_id,
        ));
    }

    remove(transaction, writing.retention, network, address, now)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{expiry, is_held};
    use crate::error::Error;

    #[test]
    fn a_lease_is_held_until_its_instant_and_expired_from_it() {
        // The boundary, stated in the only place it is written in
        // Rust. The statements in this module spell the same rule in
        // SQL, and the behaviour tests cross the boundary through
        // both at once; this is the half that says which side the
        // instant itself falls on.
        assert!(is_held(100, 99));
        assert!(!is_held(100, 100));
        assert!(!is_held(100, 101));
    }

    #[test]
    fn a_lease_that_would_expire_before_it_was_taken_is_refused() {
        assert!(matches!(expiry(100, 0), Err(Error::Invalid { .. })));
        assert!(matches!(expiry(100, -1), Err(Error::Invalid { .. })));
        assert_eq!(expiry(100, 1).unwrap(), 101);
    }

    #[test]
    fn a_duration_past_the_end_of_time_is_refused_rather_than_wrapped() {
        assert!(matches!(expiry(i64::MAX, 1), Err(Error::Invalid { .. })));
    }
}
