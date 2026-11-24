//! Every statement the store runs, and the events each write emits.
//!
//! All of the SQL in this crate is here or in a migration, which is
//! deliberate: it is the part a reviewer reads most closely, and the
//! rule that every operation is scoped to a network is only checkable
//! if the statements are in one place. Nothing in this module is
//! public, and no type from `rusqlite` appears in any signature that
//! leaves the crate.
//!
//! The functions come in two shapes. A read takes a connection and
//! answers from whatever view that connection has -- which, for a
//! connection inside a read transaction, is a fixed position in the
//! store's history. A write takes the connection mutably, runs one
//! transaction, and returns the cursor it was assigned.
//!
//! **A write appends its event inside its own transaction, before the
//! row it describes.** Both halves of that matter. In the same
//! transaction, because a store whose history can be missing a change
//! that happened is worse than one that refused the change: a
//! consumer watching the history would carry on believing a state the
//! store has left. Before the row, because the event's sequence
//! number *is* the cursor the write is assigned, and the row records
//! that cursor -- so the number has to exist before the row can hold
//! it. The event is the write's result rather than its intention: a
//! constraint that refuses the row rolls the event back with it, so
//! there is no event for a write that did not happen.

use std::net::Ipv4Addr;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::clock::Clock;
use crate::cursor::Cursor;
use crate::error::{Error, Result};
use crate::identity::Epoch;
use crate::model::{AddressRange, Attachment, Network};

/// What the store calls a network when it has to say so to a person.
const NETWORK: &str = "network";
/// What it calls an attachment.
const ATTACHMENT: &str = "attachment";
/// What it calls an address range.
const RANGE: &str = "address range";

/// The kinds an event carries, which are machine-readable and so are
/// not the words above.
///
/// A kind is a string rather than a column per resource, which is
/// what makes a resource this schema does not have yet a migration
/// and a variant rather than a reshaping of the one table every
/// consumer reads.
mod kind {
    /// A network was created or removed.
    pub(super) const NETWORK: &str = "network";
    /// An interface was claimed or released.
    pub(super) const ATTACHMENT: &str = "attachment";
    /// A range was created, resized or removed.
    pub(super) const RANGE: &str = "address-range";
}

/// The longest a name may be.
///
/// Not a limit the database needs -- SQLite would take a megabyte of
/// it quite happily -- but one every log, error message and event
/// that repeats the name does. The value is generous enough that no
/// legitimate name meets it: a network namespace name is a filename,
/// an interface name is fifteen characters, and a network's name is
/// at most a UUID unless somebody is being inventive.
const NAME_LIMIT: usize = 255;

/// Refuse a name the store would not be able to report back.
///
/// Three rules, each with a reason. Empty, because every message that
/// names the resource would then name nothing. Over the length limit,
/// for the reason above. Containing a control character, because the
/// name travels through logs and event payloads and a newline in the
/// middle of one is how a single event becomes two plausible-looking
/// ones.
fn validate_name(what: &'static str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(Error::Invalid {
            what,
            detail: "it is empty".to_owned(),
        });
    }

    if value.len() > NAME_LIMIT {
        return Err(Error::Invalid {
            what,
            detail: format!(
                "it is {} bytes long, and the limit is {NAME_LIMIT}",
                value.len()
            ),
        });
    }

    if let Some(offending) = value.chars().find(|c| c.is_control()) {
        return Err(Error::Invalid {
            what,
            detail: format!("it contains the control character {:?}", offending),
        });
    }

    Ok(())
}

/// The separator that joins a namespace and an interface name into
/// the identity an event reports.
const ATTACHMENT_SEPARATOR: char = '/';

/// Refuse a namespace or interface name that cannot be rendered into
/// an attachment's identity unambiguously.
///
/// An attachment's identity is two names, and an event carries it as
/// one string. Joining them with a separator either of them could
/// contain would make `a/b` + `c` and `a` + `b/c` the same event, so
/// the separator is refused in the parts instead of being escaped --
/// neither a network namespace, which is a filename, nor an interface
/// name can legitimately contain one.
fn validate_name_part(what: &'static str, value: &str) -> Result<()> {
    validate_name(what, value)?;

    if value.contains(ATTACHMENT_SEPARATOR) {
        return Err(Error::Invalid {
            what,
            detail: format!(
                "it contains {ATTACHMENT_SEPARATOR:?}, which separates the two halves of an interface's identity"
            ),
        });
    }

    Ok(())
}

/// An attachment's identity, as an event reports it.
fn attachment_id(namespace: &str, ifname: &str) -> String {
    format!("{namespace}{ATTACHMENT_SEPARATOR}{ifname}")
}

/// Turn an address out of an INTEGER column back into an address.
///
/// The column's CHECK constraints make an out-of-range value
/// impossible, so this says so rather than clamping: a number outside
/// the unsigned 32-bit space in an address column means something
/// other than this code has been writing the table, and guessing
/// which address was meant is exactly the silently wrong answer the
/// store is built to avoid.
fn address_from_row(stored: i64) -> Result<Ipv4Addr> {
    let value = u32::try_from(stored).map_err(|_| Error::Malformed {
        detail: format!("it holds {stored} in an address column, which is not an IPv4 address"),
    })?;

    Ok(Ipv4Addr::from(value))
}

/// Wrap an engine error with what was being done.
fn storage(context: &'static str) -> impl FnOnce(rusqlite::Error) -> Error {
    move |error| Error::storage(context, error)
}

/// Whether an engine error is a constraint the schema declared doing
/// its job.
fn is_constraint(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

// ---------------------------------------------------------------------
// Reads.
//
// Every one of these is answered from the connection it is given, so
// a connection inside a read transaction answers all of them at the
// same position. None of them starts a transaction of its own, which
// is what makes that true.
// ---------------------------------------------------------------------

/// Every network, by name.
pub(crate) fn networks(conn: &Connection, epoch: &Epoch) -> Result<Vec<Network>> {
    let mut statement = conn
        .prepare_cached("SELECT name, cursor_seq FROM network ORDER BY name")
        .map_err(storage("preparing a read of the networks"))?;

    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(storage("reading the networks"))?;

    let mut networks = Vec::new();
    for row in rows {
        let (name, cursor_seq) = row.map_err(storage("reading a network"))?;
        networks.push(Network {
            name,
            cursor: Cursor::from_row(epoch, cursor_seq)?,
        });
    }

    Ok(networks)
}

/// One network, or nothing.
pub(crate) fn network(conn: &Connection, epoch: &Epoch, name: &str) -> Result<Option<Network>> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT cursor_seq FROM network WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage("reading a network"))?;

    match found {
        Some(cursor_seq) => Ok(Some(Network {
            name: name.to_owned(),
            cursor: Cursor::from_row(epoch, cursor_seq)?,
        })),
        None => Ok(None),
    }
}

/// Every interface one network has claimed.
pub(crate) fn attachments(
    conn: &Connection,
    epoch: &Epoch,
    network: &str,
) -> Result<Vec<Attachment>> {
    let mut statement = conn
        .prepare_cached(
            "SELECT namespace, ifname, network, cursor_seq FROM attachment
             WHERE network = ?1 ORDER BY namespace, ifname",
        )
        .map_err(storage("preparing a read of a network's attachments"))?;

    let rows = statement
        .query_map(params![network], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(storage("reading a network's attachments"))?;

    let mut attachments = Vec::new();
    for row in rows {
        let (namespace, ifname, network, cursor_seq) =
            row.map_err(storage("reading an attachment"))?;
        attachments.push(Attachment {
            network,
            namespace,
            ifname,
            cursor: Cursor::from_row(epoch, cursor_seq)?,
        });
    }

    Ok(attachments)
}

/// One network's claim on one interface, or nothing.
///
/// Scoped in the statement rather than filtered afterwards, so an
/// interface another network holds is absent here rather than
/// returned and then rejected.
pub(crate) fn attachment(
    conn: &Connection,
    epoch: &Epoch,
    network: &str,
    namespace: &str,
    ifname: &str,
) -> Result<Option<Attachment>> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT cursor_seq FROM attachment
             WHERE namespace = ?1 AND ifname = ?2 AND network = ?3",
            params![namespace, ifname, network],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage("reading an attachment"))?;

    match found {
        Some(cursor_seq) => Ok(Some(Attachment {
            network: network.to_owned(),
            namespace: namespace.to_owned(),
            ifname: ifname.to_owned(),
            cursor: Cursor::from_row(epoch, cursor_seq)?,
        })),
        None => Ok(None),
    }
}

/// Which network holds an interface, if any does.
///
/// The one read in the crate that names no network, because its
/// answer is a network. It is also the read the write path uses to
/// name the conflicting party when a claim is refused.
pub(crate) fn interface_owner(
    conn: &Connection,
    epoch: &Epoch,
    namespace: &str,
    ifname: &str,
) -> Result<Option<Attachment>> {
    let found: Option<(String, i64)> = conn
        .query_row(
            "SELECT network, cursor_seq FROM attachment WHERE namespace = ?1 AND ifname = ?2",
            params![namespace, ifname],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage("reading which network holds an interface"))?;

    match found {
        Some((network, cursor_seq)) => Ok(Some(Attachment {
            network,
            namespace: namespace.to_owned(),
            ifname: ifname.to_owned(),
            cursor: Cursor::from_row(epoch, cursor_seq)?,
        })),
        None => Ok(None),
    }
}

/// Every range a network owns, lowest address first.
pub(crate) fn address_ranges(
    conn: &Connection,
    epoch: &Epoch,
    network: &str,
) -> Result<Vec<AddressRange>> {
    let mut statement = conn
        .prepare_cached(
            "SELECT name, first_address, last_address, network, cursor_seq FROM address_range
             WHERE network = ?1 ORDER BY first_address",
        )
        .map_err(storage("preparing a read of a network's ranges"))?;

    let rows = statement
        .query_map(params![network], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(storage("reading a network's ranges"))?;

    let mut ranges = Vec::new();
    for row in rows {
        let (name, first, last, network, cursor_seq) =
            row.map_err(storage("reading an address range"))?;
        ranges.push(AddressRange {
            network,
            name,
            first: address_from_row(first)?,
            last: address_from_row(last)?,
            cursor: Cursor::from_row(epoch, cursor_seq)?,
        });
    }

    Ok(ranges)
}

/// One of a network's ranges, or nothing.
pub(crate) fn address_range(
    conn: &Connection,
    epoch: &Epoch,
    network: &str,
    name: &str,
) -> Result<Option<AddressRange>> {
    let found: Option<(i64, i64, i64)> = conn
        .query_row(
            "SELECT first_address, last_address, cursor_seq FROM address_range
             WHERE network = ?1 AND name = ?2",
            params![network, name],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(storage("reading an address range"))?;

    match found {
        Some((first, last, cursor_seq)) => Ok(Some(AddressRange {
            network: network.to_owned(),
            name: name.to_owned(),
            first: address_from_row(first)?,
            last: address_from_row(last)?,
            cursor: Cursor::from_row(epoch, cursor_seq)?,
        })),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------
// The event log.
//
// One function per kind, because the post-state an event carries is
// built by the engine's own JSON function from the same parameters the
// row is written from. Building the JSON in SQL rather than in Rust
// means the escaping is the engine's: a network called `say "hi"` is
// then a correctly quoted event rather than a payload that parses as
// something else.
// ---------------------------------------------------------------------

/// Append the event for a network that now exists, and return the
/// sequence it was given.
fn event_network_created(transaction: &Transaction<'_>, name: &str, now: i64) -> Result<i64> {
    transaction
        .execute(
            "INSERT INTO event_log (network, kind, resource_id, state, recorded_at)
             VALUES (?1, ?2, ?1, json_object('name', ?1), ?3)",
            params![name, kind::NETWORK, now],
        )
        .map_err(storage("recording that a network was created"))?;

    Ok(transaction.last_insert_rowid())
}

/// Append the event for an attachment that now exists.
fn event_attachment_written(
    transaction: &Transaction<'_>,
    network: &str,
    namespace: &str,
    ifname: &str,
    now: i64,
) -> Result<i64> {
    transaction
        .execute(
            "INSERT INTO event_log (network, kind, resource_id, state, recorded_at)
             VALUES (?1, ?2, ?3,
                     json_object('network', ?1, 'namespace', ?4, 'ifname', ?5), ?6)",
            params![
                network,
                kind::ATTACHMENT,
                attachment_id(namespace, ifname),
                namespace,
                ifname,
                now
            ],
        )
        .map_err(storage("recording that an interface was claimed"))?;

    Ok(transaction.last_insert_rowid())
}

/// Append the event for a range that now exists with these ends.
///
/// The addresses go into the state as the integers the row holds
/// rather than as dotted quads. The state is the post-state of the
/// row, and rendering an address for something to read is the job of
/// whatever is doing the reading.
fn event_range_written(
    transaction: &Transaction<'_>,
    network: &str,
    name: &str,
    first: Ipv4Addr,
    last: Ipv4Addr,
    now: i64,
) -> Result<i64> {
    transaction
        .execute(
            "INSERT INTO event_log (network, kind, resource_id, state, recorded_at)
             VALUES (?1, ?2, ?3,
                     json_object('network', ?1, 'name', ?3, 'first', ?4, 'last', ?5), ?6)",
            params![
                network,
                kind::RANGE,
                name,
                u32::from(first),
                u32::from(last),
                now
            ],
        )
        .map_err(storage("recording an address range"))?;

    Ok(transaction.last_insert_rowid())
}

/// Append the event for a resource that no longer exists.
///
/// A deletion is a NULL state rather than a kind of its own: the
/// state an event carries is the resource's whole post-state, and the
/// post-state of something deleted is its absence. A consumer
/// applying events therefore needs no second rule, and applying one
/// twice does no harm.
fn event_deleted(
    transaction: &Transaction<'_>,
    network: &str,
    kind: &str,
    resource_id: &str,
    now: i64,
) -> Result<i64> {
    transaction
        .execute(
            "INSERT INTO event_log (network, kind, resource_id, state, recorded_at)
             VALUES (?1, ?2, ?3, NULL, ?4)",
            params![network, kind, resource_id, now],
        )
        .map_err(storage("recording that a resource was removed"))?;

    Ok(transaction.last_insert_rowid())
}

// ---------------------------------------------------------------------
// Writes.
// ---------------------------------------------------------------------

/// What every write needs besides the connection it runs on.
///
/// A struct rather than two more parameters on each function, which
/// is not only a matter of length: the clock and the epoch are the
/// same two values for every write the store will ever do, and a
/// signature that repeats them invites a call site to pass a
/// different clock to one write than to the next.
pub(crate) struct Writing<'a> {
    /// The clock every row and event written here is stamped with.
    pub(crate) clock: &'a dyn Clock,
    /// The epoch the cursor this write is assigned belongs to.
    pub(crate) epoch: &'a Epoch,
}

/// Refuse early if the network named does not exist.
///
/// The foreign keys on the child tables would refuse the write
/// anyway, which is the structural guarantee and stays. This runs
/// first so that the caller is told which network was wrong rather
/// than being handed a constraint violation that names a column.
fn require_network(transaction: &Transaction<'_>, network: &str) -> Result<()> {
    let exists: Option<i64> = transaction
        .query_row(
            "SELECT 1 FROM network WHERE name = ?1",
            params![network],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage("checking that a network exists"))?;

    if exists.is_none() {
        return Err(Error::UnknownNetwork {
            network: network.to_owned(),
        });
    }

    Ok(())
}

/// Create a network, refusing a name already in use.
pub(crate) fn create_network(
    conn: &mut Connection,
    writing: &Writing<'_>,
    name: &str,
) -> Result<Cursor> {
    validate_name("network name", name)?;

    let now = writing.clock.unix_seconds();
    let transaction = conn
        .transaction()
        .map_err(storage("starting a transaction to create a network"))?;

    let sequence = event_network_created(&transaction, name, now)?;

    transaction
        .execute(
            "INSERT INTO network (name, cursor_seq) VALUES (?1, ?2)",
            params![name, sequence],
        )
        .map_err(|error| {
            if is_constraint(&error) {
                Error::Conflict {
                    kind: NETWORK,
                    id: name.to_owned(),
                    held_by: "a network of that name already exists".to_owned(),
                }
            } else {
                Error::storage("creating a network", error)
            }
        })?;

    transaction
        .commit()
        .map_err(storage("committing a new network"))?;

    Ok(Cursor::new(
        writing.epoch.clone(),
        sequence_as_position(sequence)?,
    ))
}

/// Remove a network that is still where the caller read it.
pub(crate) fn delete_network(
    conn: &mut Connection,
    writing: &Writing<'_>,
    name: &str,
    expected: &Cursor,
) -> Result<Cursor> {
    let now = writing.clock.unix_seconds();
    let transaction = conn
        .transaction()
        .map_err(storage("starting a transaction to remove a network"))?;

    let current =
        network(&transaction, writing.epoch, name)?.ok_or_else(|| Error::UnknownNetwork {
            network: name.to_owned(),
        })?;
    if &current.cursor != expected {
        return Err(Error::changed(NETWORK, name, expected, &current.cursor));
    }

    // The foreign keys are what refuse a network with children, and
    // they refuse it as a constraint violation naming neither the
    // network nor what is still in it. Counting first turns that into
    // a sentence an operator can act on, and the constraint stays
    // underneath as the thing that actually holds.
    let children = count_children(&transaction, name)?;
    if children > 0 {
        return Err(Error::Conflict {
            kind: NETWORK,
            id: name.to_owned(),
            held_by: format!(
                "it still has {children} attachment(s) and range(s); detach and remove those first"
            ),
        });
    }

    let sequence = event_deleted(&transaction, name, kind::NETWORK, name, now)?;

    transaction
        .execute("DELETE FROM network WHERE name = ?1", params![name])
        .map_err(|error| {
            if is_constraint(&error) {
                Error::Conflict {
                    kind: NETWORK,
                    id: name.to_owned(),
                    held_by: "something in it still refers to it".to_owned(),
                }
            } else {
                Error::storage("removing a network", error)
            }
        })?;

    transaction
        .commit()
        .map_err(storage("committing the removal of a network"))?;

    Ok(Cursor::new(
        writing.epoch.clone(),
        sequence_as_position(sequence)?,
    ))
}

/// How many child rows a network still has.
fn count_children(transaction: &Transaction<'_>, network: &str) -> Result<i64> {
    transaction
        .query_row(
            "SELECT (SELECT count(*) FROM attachment WHERE network = ?1)
                  + (SELECT count(*) FROM address_range WHERE network = ?1)",
            params![network],
            |row| row.get(0),
        )
        .map_err(storage("counting what a network still holds"))
}

/// Claim an interface for a network, refusing a claim another network
/// already holds.
pub(crate) fn attach_interface(
    conn: &mut Connection,
    writing: &Writing<'_>,
    network: &str,
    namespace: &str,
    ifname: &str,
) -> Result<Cursor> {
    validate_name_part("namespace", namespace)?;
    validate_name_part("interface name", ifname)?;

    let now = writing.clock.unix_seconds();
    let transaction = conn
        .transaction()
        .map_err(storage("starting a transaction to claim an interface"))?;

    require_network(&transaction, network)?;

    let sequence = event_attachment_written(&transaction, network, namespace, ifname, now)?;

    let claimed = transaction.execute(
        "INSERT INTO attachment (namespace, ifname, network, cursor_seq)
         VALUES (?1, ?2, ?3, ?4)",
        params![namespace, ifname, network, sequence],
    );

    if let Err(error) = claimed {
        if !is_constraint(&error) {
            return Err(Error::storage("claiming an interface", error));
        }

        // The database refused it. Ask the database which network
        // holds it, so that the refusal names the conflicting party
        // rather than only the fact of the conflict. A constraint
        // violation rolls back the statement and not the transaction,
        // so this read still works; the transaction is then dropped
        // without a commit, which takes the event with it.
        let holder = interface_owner(&transaction, writing.epoch, namespace, ifname)?;

        return Err(match holder {
            Some(attachment) => Error::Conflict {
                kind: ATTACHMENT,
                id: attachment_id(namespace, ifname),
                held_by: format!(
                    "the network {} already holds that interface, and an interface belongs to \
                     exactly one network",
                    attachment.network
                ),
            },
            // Nothing holds it and the insert was still refused, so
            // the constraint that fired was not the one this path is
            // about. Reported as what it is rather than guessed at.
            None => Error::storage("claiming an interface", error),
        });
    }

    transaction
        .commit()
        .map_err(storage("committing a claim on an interface"))?;

    Ok(Cursor::new(
        writing.epoch.clone(),
        sequence_as_position(sequence)?,
    ))
}

/// Release a network's claim on an interface.
pub(crate) fn detach_interface(
    conn: &mut Connection,
    writing: &Writing<'_>,
    network: &str,
    namespace: &str,
    ifname: &str,
    expected: &Cursor,
) -> Result<Cursor> {
    let now = writing.clock.unix_seconds();
    let transaction = conn
        .transaction()
        .map_err(storage("starting a transaction to release an interface"))?;

    let id = attachment_id(namespace, ifname);

    // Scoped to the network that asked. An interface another network
    // holds is not found here, rather than being released on that
    // network's behalf.
    let current =
        attachment(&transaction, writing.epoch, network, namespace, ifname)?.ok_or_else(|| {
            Error::NotFound {
                kind: ATTACHMENT,
                id: format!("{id} in the network {network}"),
            }
        })?;
    if &current.cursor != expected {
        return Err(Error::changed(ATTACHMENT, id, expected, &current.cursor));
    }

    let sequence = event_deleted(&transaction, network, kind::ATTACHMENT, &id, now)?;

    transaction
        .execute(
            "DELETE FROM attachment WHERE namespace = ?1 AND ifname = ?2 AND network = ?3",
            params![namespace, ifname, network],
        )
        .map_err(storage("releasing an interface"))?;

    transaction
        .commit()
        .map_err(storage("committing the release of an interface"))?;

    Ok(Cursor::new(
        writing.epoch.clone(),
        sequence_as_position(sequence)?,
    ))
}

/// Refuse a span that is not a span, and one that overlaps another of
/// the same network's ranges.
///
/// The overlap check names the network in its statement, which is the
/// whole of the per-network behaviour: two networks may own identical
/// ranges, and routinely do, because they are different address
/// spaces that happen to be written the same way. Two ranges in *one*
/// network that both contain an address make "the lowest free address
/// in this network" a question with two answers, so that is refused.
fn check_span(
    transaction: &Transaction<'_>,
    network: &str,
    name: &str,
    first: Ipv4Addr,
    last: Ipv4Addr,
) -> Result<()> {
    if u32::from(first) > u32::from(last) {
        return Err(Error::Invalid {
            what: "address range",
            detail: format!("{first} is above {last}, so the span runs backwards"),
        });
    }

    let overlapping: Option<String> = transaction
        .query_row(
            "SELECT name FROM address_range
             WHERE network = ?1 AND name <> ?2
               AND first_address <= ?3 AND last_address >= ?4
             ORDER BY first_address LIMIT 1",
            params![network, name, u32::from(last), u32::from(first)],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage("looking for an overlapping range"))?;

    if let Some(other) = overlapping {
        return Err(Error::Conflict {
            kind: RANGE,
            id: name.to_owned(),
            held_by: format!(
                "the range {other} in the same network already covers part of {first}-{last}"
            ),
        });
    }

    Ok(())
}

/// Give a network a range.
pub(crate) fn create_address_range(
    conn: &mut Connection,
    writing: &Writing<'_>,
    network: &str,
    name: &str,
    first: Ipv4Addr,
    last: Ipv4Addr,
) -> Result<Cursor> {
    validate_name("address range name", name)?;

    let now = writing.clock.unix_seconds();
    let transaction = conn
        .transaction()
        .map_err(storage("starting a transaction to create a range"))?;

    require_network(&transaction, network)?;
    check_span(&transaction, network, name, first, last)?;

    let sequence = event_range_written(&transaction, network, name, first, last, now)?;

    transaction
        .execute(
            "INSERT INTO address_range (network, name, first_address, last_address, cursor_seq)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![network, name, u32::from(first), u32::from(last), sequence],
        )
        .map_err(|error| {
            if is_constraint(&error) {
                Error::Conflict {
                    kind: RANGE,
                    id: name.to_owned(),
                    held_by: format!("the network {network} already has a range called {name}"),
                }
            } else {
                Error::storage("creating an address range", error)
            }
        })?;

    transaction
        .commit()
        .map_err(storage("committing a new address range"))?;

    Ok(Cursor::new(
        writing.epoch.clone(),
        sequence_as_position(sequence)?,
    ))
}

/// Move the ends of a range that is still where the caller read it.
pub(crate) fn update_address_range(
    conn: &mut Connection,
    writing: &Writing<'_>,
    network: &str,
    name: &str,
    expected: &Cursor,
    first: Ipv4Addr,
    last: Ipv4Addr,
) -> Result<Cursor> {
    let now = writing.clock.unix_seconds();
    let transaction = conn
        .transaction()
        .map_err(storage("starting a transaction to resize a range"))?;

    let current = address_range(&transaction, writing.epoch, network, name)?.ok_or_else(|| {
        Error::NotFound {
            kind: RANGE,
            id: format!("{name} in the network {network}"),
        }
    })?;
    if &current.cursor != expected {
        return Err(Error::changed(RANGE, name, expected, &current.cursor));
    }

    check_span(&transaction, network, name, first, last)?;

    let sequence = event_range_written(&transaction, network, name, first, last, now)?;

    transaction
        .execute(
            "UPDATE address_range SET first_address = ?3, last_address = ?4, cursor_seq = ?5
             WHERE network = ?1 AND name = ?2",
            params![network, name, u32::from(first), u32::from(last), sequence],
        )
        .map_err(storage("resizing an address range"))?;

    transaction
        .commit()
        .map_err(storage("committing a resized address range"))?;

    Ok(Cursor::new(
        writing.epoch.clone(),
        sequence_as_position(sequence)?,
    ))
}

/// Remove a range that is still where the caller read it.
pub(crate) fn delete_address_range(
    conn: &mut Connection,
    writing: &Writing<'_>,
    network: &str,
    name: &str,
    expected: &Cursor,
) -> Result<Cursor> {
    let now = writing.clock.unix_seconds();
    let transaction = conn
        .transaction()
        .map_err(storage("starting a transaction to remove a range"))?;

    let current = address_range(&transaction, writing.epoch, network, name)?.ok_or_else(|| {
        Error::NotFound {
            kind: RANGE,
            id: format!("{name} in the network {network}"),
        }
    })?;
    if &current.cursor != expected {
        return Err(Error::changed(RANGE, name, expected, &current.cursor));
    }

    let sequence = event_deleted(&transaction, network, kind::RANGE, name, now)?;

    transaction
        .execute(
            "DELETE FROM address_range WHERE network = ?1 AND name = ?2",
            params![network, name],
        )
        .map_err(storage("removing an address range"))?;

    transaction
        .commit()
        .map_err(storage("committing the removal of an address range"))?;

    Ok(Cursor::new(
        writing.epoch.clone(),
        sequence_as_position(sequence)?,
    ))
}

/// The position a snapshot reads at.
///
/// The highest sequence number the store has ever issued, as of
/// whatever view the connection has -- read out of the counter the
/// engine keeps for the log's sequence rather than as the largest
/// sequence still in the table. Those differ once old events have
/// been evicted, and it is the counter that is the position: the
/// largest number still present would send a resuming consumer back
/// to before the events that were evicted, which is the one thing a
/// cursor must never do.
///
/// Zero means nothing has been written yet. There is no event at
/// zero, which is what makes it safe to mean "before everything".
pub(crate) fn snapshot_position(conn: &Connection) -> Result<u64> {
    let sequence: i64 = conn
        .query_row(
            "SELECT coalesce((SELECT seq FROM sqlite_sequence WHERE name = 'event_log'), 0)",
            [],
            |row| row.get(0),
        )
        .map_err(storage("reading the store's current position"))?;

    sequence_as_position(sequence)
}

/// Turn a sequence number the engine issued into a position.
fn sequence_as_position(sequence: i64) -> Result<u64> {
    u64::try_from(sequence).map_err(|_| Error::Malformed {
        detail: format!("it issued the sequence number {sequence}, which is not a position"),
    })
}

#[cfg(test)]
mod tests {
    use super::{attachment_id, validate_name, validate_name_part};
    use crate::error::Error;

    #[test]
    fn a_name_with_a_newline_in_it_is_refused() {
        // The name is repeated into an event's payload and into every
        // log line that reports the resource. A newline in the middle
        // of one is how a single event becomes two that both look
        // well-formed.
        let refusal = validate_name("network name", "alpha\nbeta").unwrap_err();

        match refusal {
            Error::Invalid { what, detail } => {
                assert_eq!(what, "network name");
                assert!(detail.contains("control character"), "{detail}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_name_is_refused() {
        assert!(matches!(
            validate_name("network name", ""),
            Err(Error::Invalid { .. })
        ));
    }

    #[test]
    fn the_halves_of_an_interface_identity_may_not_contain_its_separator() {
        // Without this, `a/b` in namespace `x` and `b` in namespace
        // `x/a` would report the same identity in the event log, and
        // a consumer applying both would hold one resource where
        // there are two.
        assert_eq!(attachment_id("x/a", "b"), attachment_id("x", "a/b"));

        assert!(matches!(
            validate_name_part("namespace", "x/a"),
            Err(Error::Invalid { .. })
        ));
        assert!(matches!(
            validate_name_part("interface name", "a/b"),
            Err(Error::Invalid { .. })
        ));
    }
}
