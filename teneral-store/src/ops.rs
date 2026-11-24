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
use crate::guard::CrossNetworkGuard;
use crate::identity::Epoch;
use crate::model::{AddressRange, Attachment, Binding, Explanation, Network};
use crate::selector::{ClientIdentity, Selector};

/// What the store calls a network when it has to say so to a person.
const NETWORK: &str = "network";
/// What it calls an attachment.
const ATTACHMENT: &str = "attachment";
/// What it calls an address range.
const RANGE: &str = "address range";
/// What it calls a binding.
const BINDING: &str = "binding";

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
    /// A binding was configured, changed or removed.
    pub(super) const BINDING: &str = "binding";
}

/// The identity an event reports a binding by: its selector, which is
/// the whole of what makes it unique inside its network.
///
/// The kind and the value are joined with a colon, and that is
/// unambiguous even though a value may itself contain colons -- a
/// hardware address does. The kind side cannot: the column it comes
/// from is constrained to five hyphenated words, so the first colon in
/// this string is always the separator and everything after it is the
/// value. The attachment identity above needs a refusal instead of
/// this reasoning because both of *its* halves are operator-supplied
/// names.
fn binding_id(selector: &Selector) -> String {
    format!("{}:{}", selector.kind().as_str(), selector.value())
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
pub(crate) fn validate_name(what: &'static str, value: &str) -> Result<()> {
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

/// What every read inside one network needs besides the connection.
///
/// A struct rather than three more parameters, for the same reason
/// [`Writing`] is one: these are the same three values for every read
/// a caller makes, and a signature that repeats them invites a call
/// site to pass one network to the statement and another to the
/// check.
///
/// The check is the point of the third field. Every row these reads
/// return carries the network it came from, and that value is
/// compared with `network` before the row becomes an answer -- so a
/// statement that lost its scoping refuses to answer rather than
/// answering out of the wrong network.
pub(crate) struct Reading<'a> {
    /// The epoch every cursor on a returned row belongs to.
    pub(crate) epoch: &'a Epoch,
    /// The network the caller asked about.
    pub(crate) network: &'a str,
    /// The comparison every returned row passes.
    pub(crate) guard: &'a CrossNetworkGuard,
}

impl<'a> Reading<'a> {
    /// Refuse a row that came from a network other than the one asked
    /// about.
    fn guard<F>(&self, found: &str, kind: &'static str, id: F) -> Result<()>
    where
        F: FnOnce() -> String,
    {
        self.guard.check(self.network, found, kind, id)
    }
}

// ---------------------------------------------------------------------
// Reads.
//
// Every one of these is answered from the connection it is given, so
// a connection inside a read transaction answers all of them at the
// same position. None of them starts a transaction of its own, which
// is what makes that true.
//
// Every read that names a network takes a `Reading` and passes each
// row it materialises through its guard. Today most of those
// comparisons cannot fail, the statements naming the network in their
// own `WHERE` clauses, and they are made anyway: the comparison costs
// one string equality and it is what stands between a dropped clause
// or a rotated parameter list and an answer out of the wrong network.
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
///
/// The only network-named read that takes no [`Reading`], and the
/// reason is that there is nothing here for a guard to compare. A
/// network's identity *is* its name, so the row either is the one
/// asked for or is not returned; there is no second value that could
/// disagree with the first.
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
pub(crate) fn attachments(conn: &Connection, reading: &Reading<'_>) -> Result<Vec<Attachment>> {
    let network = reading.network;
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
        reading.guard(&network, ATTACHMENT, || attachment_id(&namespace, &ifname))?;
        attachments.push(Attachment {
            network,
            namespace,
            ifname,
            cursor: Cursor::from_row(reading.epoch, cursor_seq)?,
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
    reading: &Reading<'_>,
    namespace: &str,
    ifname: &str,
) -> Result<Option<Attachment>> {
    // The network column is selected rather than taken from the
    // question, which is what gives the guard below something to
    // compare. Reading it back costs a string per row and is the
    // difference between a check and an assumption.
    let found: Option<(String, i64)> = conn
        .query_row(
            "SELECT network, cursor_seq FROM attachment
             WHERE namespace = ?1 AND ifname = ?2 AND network = ?3",
            params![namespace, ifname, reading.network],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage("reading an attachment"))?;

    match found {
        Some((network, cursor_seq)) => {
            reading.guard(&network, ATTACHMENT, || attachment_id(namespace, ifname))?;

            Ok(Some(Attachment {
                network,
                namespace: namespace.to_owned(),
                ifname: ifname.to_owned(),
                cursor: Cursor::from_row(reading.epoch, cursor_seq)?,
            }))
        }
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
    reading: &Reading<'_>,
) -> Result<Vec<AddressRange>> {
    let network = reading.network;
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
        reading.guard(&network, RANGE, || name.clone())?;
        ranges.push(AddressRange {
            network,
            name,
            first: address_from_row(first)?,
            last: address_from_row(last)?,
            cursor: Cursor::from_row(reading.epoch, cursor_seq)?,
        });
    }

    Ok(ranges)
}

/// One of a network's ranges, or nothing.
pub(crate) fn address_range(
    conn: &Connection,
    reading: &Reading<'_>,
    name: &str,
) -> Result<Option<AddressRange>> {
    let found: Option<(String, i64, i64, i64)> = conn
        .query_row(
            "SELECT network, first_address, last_address, cursor_seq FROM address_range
             WHERE network = ?1 AND name = ?2",
            params![reading.network, name],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(storage("reading an address range"))?;

    match found {
        Some((network, first, last, cursor_seq)) => {
            reading.guard(&network, RANGE, || name.to_owned())?;

            Ok(Some(AddressRange {
                network,
                name: name.to_owned(),
                first: address_from_row(first)?,
                last: address_from_row(last)?,
                cursor: Cursor::from_row(reading.epoch, cursor_seq)?,
            }))
        }
        None => Ok(None),
    }
}

/// Every binding one network has configured.
///
/// Ordered by the key, which is the selector's kind and then its
/// value, rather than by the precedence between kinds. Precedence is
/// a property of a lookup and not of a network's configuration: a
/// listing is what an operator reads to see what is configured, and
/// the order a client's attributes would be consulted in says nothing
/// about a set of bindings that may not overlap at all.
pub(crate) fn bindings(conn: &Connection, reading: &Reading<'_>) -> Result<Vec<Binding>> {
    let mut statement = conn
        .prepare_cached(
            "SELECT selector_kind, selector_value, address, hostname, network, cursor_seq
             FROM binding WHERE network = ?1 ORDER BY selector_kind, selector_value",
        )
        .map_err(storage("preparing a read of a network's bindings"))?;

    let rows = statement
        .query_map(params![reading.network], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(storage("reading a network's bindings"))?;

    let mut bindings = Vec::new();
    for row in rows {
        let (kind, value, address, hostname, network, cursor_seq) =
            row.map_err(storage("reading a binding"))?;
        let selector = Selector::from_row(&kind, value)?;
        reading.guard(&network, BINDING, || binding_id(&selector))?;
        bindings.push(Binding {
            network,
            selector,
            address: address_from_row(address)?,
            hostname,
            cursor: Cursor::from_row(reading.epoch, cursor_seq)?,
        });
    }

    Ok(bindings)
}

/// The binding configured against one selector in one network, or
/// nothing.
///
/// This is the single probe every lookup is built from, and the only
/// statement in the crate that reads the binding table by its key.
/// Both halves of the key's scoping matter: the network is in the
/// `WHERE` clause, so another network's binding cannot be returned,
/// and the network is read back out of the row, so one that somehow
/// was would be refused rather than served.
pub(crate) fn binding(
    conn: &Connection,
    reading: &Reading<'_>,
    selector: &Selector,
) -> Result<Option<Binding>> {
    let found: Option<(String, i64, Option<String>, i64)> = conn
        .query_row(
            "SELECT network, address, hostname, cursor_seq FROM binding
             WHERE network = ?1 AND selector_kind = ?2 AND selector_value = ?3",
            params![reading.network, selector.kind().as_str(), selector.value()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(storage("reading a binding"))?;

    match found {
        Some((network, address, hostname, cursor_seq)) => {
            reading.guard(&network, BINDING, || binding_id(selector))?;

            Ok(Some(Binding {
                network,
                selector: selector.clone(),
                address: address_from_row(address)?,
                hostname,
                cursor: Cursor::from_row(reading.epoch, cursor_seq)?,
            }))
        }
        None => Ok(None),
    }
}

/// Which of a network's bindings applies to a client presenting this
/// identity.
///
/// The one matcher in the crate. Every way of asking the question --
/// a read within a network, a read that also names the interface the
/// client arrived on, and the explain operation that starts from an
/// interface and resolves the network itself -- comes through here,
/// so the answer cannot depend on which of them was asked and the
/// front-ends cannot grow a second implementation of the precedence.
///
/// It walks the client's selectors in precedence order and returns
/// the first that has a binding, which makes the match the
/// highest-precedence one by construction rather than by a sort that
/// could be written wrongly. The cost is up to one indexed probe per
/// attribute the client presented -- five at the very most, each on a
/// primary key -- which is a trade worth making for a loop whose
/// order is visible in one place and cannot be hidden inside a SQL
/// `ORDER BY` that disagrees with the published documentation.
///
/// The returned binding carries the selector it was found under, so
/// "which selector matched" is part of the answer and not a second
/// question.
pub(crate) fn lookup_binding(
    conn: &Connection,
    reading: &Reading<'_>,
    identity: &ClientIdentity,
) -> Result<Option<Binding>> {
    for selector in identity.selectors() {
        if let Some(found) = binding(conn, reading, &selector)? {
            return Ok(Some(found));
        }
    }

    Ok(None)
}

/// Which of this network's bindings applies to a client presenting
/// this identity on this interface, where the caller has asserted
/// that the interface belongs to the network.
///
/// The assertion is the whole difference from [`lookup_binding`], and
/// it is the read where the cross-network guard can fire from the
/// data rather than from a defect. The network comes from the caller
/// -- a path that names it, or a request context -- and the
/// interface's network comes from the store, so the two can disagree.
/// A request whose arrival interface resolves to one network while
/// the answer would come from another is precisely the quantity the
/// specification asks to be counted, and serving an answer anyway
/// would be the leak this store exists to prevent.
///
/// An interface no network has claimed is not found rather than
/// refused: nothing crossed between networks, and the fault is a
/// different one.
///
/// Note how this differs from [`attachment`], which answers `None`
/// for an interface another network holds. That read asks whether a
/// network has an interface, and no is a legitimate answer. This one
/// asserts that it does in order to be answered for a client on it,
/// and a false premise there has to be refused rather than quietly
/// turned into "no binding".
pub(crate) fn lookup_binding_on_interface(
    conn: &Connection,
    reading: &Reading<'_>,
    namespace: &str,
    ifname: &str,
    identity: &ClientIdentity,
) -> Result<Option<Binding>> {
    let holder = interface_owner(conn, reading.epoch, namespace, ifname)?.ok_or_else(|| {
        Error::NotFound {
            kind: ATTACHMENT,
            id: format!(
                "{} in the network {}",
                attachment_id(namespace, ifname),
                reading.network
            ),
        }
    })?;

    reading.guard(&holder.network, ATTACHMENT, || {
        attachment_id(namespace, ifname)
    })?;

    lookup_binding(conn, reading, identity)
}

/// What the store would answer a client presenting this identity on
/// this interface, and how it got there.
///
/// Store-global, because it takes no network and its answer includes
/// one: it starts where the datapath starts, with an interface, and
/// the first thing it does is the resolution step the datapath
/// performs before anything is scoped. An operator whose binding is
/// not being matched needs that step reported as much as the match
/// itself -- an interface attached to the wrong network, or to none,
/// looks exactly like a binding that does not match.
///
/// This is not a second implementation of the lookup. It resolves the
/// interface, then calls the same matcher every other form of the
/// question calls, and reports what that matcher was given along with
/// what it returned.
pub(crate) fn explain(
    conn: &Connection,
    epoch: &Epoch,
    guard: &CrossNetworkGuard,
    at: &Cursor,
    namespace: &str,
    ifname: &str,
    identity: &ClientIdentity,
) -> Result<Explanation> {
    let considered = identity.selectors();
    let holder = interface_owner(conn, epoch, namespace, ifname)?;

    let matched = match &holder {
        Some(attachment) => {
            let reading = Reading {
                epoch,
                network: &attachment.network,
                guard,
            };

            lookup_binding(conn, &reading, identity)?
        }
        // No network has claimed the interface, so there is no
        // network whose bindings could be consulted. Reported as a
        // network of `None` rather than as an error, because that is
        // the diagnosis: a client arriving here would be answered by
        // nobody, and an operator who expected otherwise has found
        // the fault without having to send a packet.
        None => None,
    };

    Ok(Explanation {
        namespace: namespace.to_owned(),
        ifname: ifname.to_owned(),
        network: holder.map(|attachment| attachment.network),
        matched,
        considered,
        cursor: at.clone(),
    })
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

/// Append the event for a binding that now exists in this form.
///
/// The selector appears twice in the payload: once as the resource's
/// identity, in `resource_id`, and once split into its two parts in
/// the state. A consumer applying events needs the identity to know
/// which resource moved, and needs the parts to avoid parsing that
/// identity back apart.
fn event_binding_written(
    transaction: &Transaction<'_>,
    network: &str,
    selector: &Selector,
    address: Ipv4Addr,
    hostname: Option<&str>,
    now: i64,
) -> Result<i64> {
    transaction
        .execute(
            "INSERT INTO event_log (network, kind, resource_id, state, recorded_at)
             VALUES (?1, ?2, ?3,
                     json_object('network', ?1, 'selector_kind', ?4,
                                 'selector_value', ?5, 'address', ?6,
                                 'hostname', ?7), ?8)",
            params![
                network,
                kind::BINDING,
                binding_id(selector),
                selector.kind().as_str(),
                selector.value(),
                u32::from(address),
                hostname,
                now
            ],
        )
        .map_err(storage("recording a binding"))?;

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
    /// The guard the reads a write does on its way pass through. A
    /// compare-and-set reads the resource it is about to change, and
    /// that read is scoped like any other.
    pub(crate) guard: &'a CrossNetworkGuard,
}

impl<'a> Writing<'a> {
    /// The scope for a read this write makes inside `network`.
    fn reading<'b>(&'b self, network: &'b str) -> Reading<'b> {
        Reading {
            epoch: self.epoch,
            network,
            guard: self.guard,
        }
    }
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
                "it still has {children} attachment(s), range(s) and binding(s); remove those \
                 first"
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
                  + (SELECT count(*) FROM address_range WHERE network = ?1)
                  + (SELECT count(*) FROM binding WHERE network = ?1)",
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
    let current = attachment(&transaction, &writing.reading(network), namespace, ifname)?
        .ok_or_else(|| Error::NotFound {
            kind: ATTACHMENT,
            id: format!("{id} in the network {network}"),
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

    let current =
        address_range(&transaction, &writing.reading(network), name)?.ok_or_else(|| {
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

    let current =
        address_range(&transaction, &writing.reading(network), name)?.ok_or_else(|| {
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

/// Refuse a hostname the store would not be able to report back.
///
/// The same three rules every other name gets, and no more. A
/// hostname has a stricter grammar than this -- labels, lengths,
/// which characters a zone will accept -- and that grammar belongs
/// with the zones that publish it rather than here, where refusing a
/// name on a rule the DNS side has not been written yet would be
/// guessing at it.
fn validate_hostname(hostname: Option<&str>) -> Result<()> {
    match hostname {
        Some(hostname) => validate_name("hostname", hostname),
        None => Ok(()),
    }
}

/// Configure a binding, refusing a selector the network already has
/// one for.
pub(crate) fn create_binding(
    conn: &mut Connection,
    writing: &Writing<'_>,
    network: &str,
    selector: &Selector,
    address: Ipv4Addr,
    hostname: Option<&str>,
) -> Result<Cursor> {
    validate_hostname(hostname)?;

    let now = writing.clock.unix_seconds();
    let transaction = conn
        .transaction()
        .map_err(storage("starting a transaction to configure a binding"))?;

    require_network(&transaction, network)?;

    let sequence = event_binding_written(&transaction, network, selector, address, hostname, now)?;

    transaction
        .execute(
            "INSERT INTO binding
                 (network, selector_kind, selector_value, address, hostname, cursor_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                network,
                selector.kind().as_str(),
                selector.value(),
                u32::from(address),
                hostname,
                sequence
            ],
        )
        .map_err(|error| {
            if is_constraint(&error) {
                Error::Conflict {
                    kind: BINDING,
                    id: binding_id(selector),
                    held_by: format!(
                        "the network {network} already has a binding for that selector; change \
                         it rather than adding a second answer for one client"
                    ),
                }
            } else {
                Error::storage("configuring a binding", error)
            }
        })?;

    transaction
        .commit()
        .map_err(storage("committing a binding"))?;

    Ok(Cursor::new(
        writing.epoch.clone(),
        sequence_as_position(sequence)?,
    ))
}

/// Change a binding that is still where the caller read it.
///
/// The selector is not changeable here, being the binding's identity:
/// a binding whose selector moved is a different binding, and a
/// consumer watching the history would have to be told that one
/// resource went away and another arrived. Removing it and
/// configuring the new one says exactly that.
pub(crate) fn update_binding(
    conn: &mut Connection,
    writing: &Writing<'_>,
    network: &str,
    selector: &Selector,
    expected: &Cursor,
    address: Ipv4Addr,
    hostname: Option<&str>,
) -> Result<Cursor> {
    validate_hostname(hostname)?;

    let now = writing.clock.unix_seconds();
    let transaction = conn
        .transaction()
        .map_err(storage("starting a transaction to change a binding"))?;

    let current = binding(&transaction, &writing.reading(network), selector)?.ok_or_else(|| {
        Error::NotFound {
            kind: BINDING,
            id: format!("{} in the network {network}", binding_id(selector)),
        }
    })?;
    if &current.cursor != expected {
        return Err(Error::changed(
            BINDING,
            binding_id(selector),
            expected,
            &current.cursor,
        ));
    }

    let sequence = event_binding_written(&transaction, network, selector, address, hostname, now)?;

    transaction
        .execute(
            "UPDATE binding SET address = ?4, hostname = ?5, cursor_seq = ?6
             WHERE network = ?1 AND selector_kind = ?2 AND selector_value = ?3",
            params![
                network,
                selector.kind().as_str(),
                selector.value(),
                u32::from(address),
                hostname,
                sequence
            ],
        )
        .map_err(storage("changing a binding"))?;

    transaction
        .commit()
        .map_err(storage("committing a changed binding"))?;

    Ok(Cursor::new(
        writing.epoch.clone(),
        sequence_as_position(sequence)?,
    ))
}

/// Remove a binding that is still where the caller read it.
pub(crate) fn delete_binding(
    conn: &mut Connection,
    writing: &Writing<'_>,
    network: &str,
    selector: &Selector,
    expected: &Cursor,
) -> Result<Cursor> {
    let now = writing.clock.unix_seconds();
    let transaction = conn
        .transaction()
        .map_err(storage("starting a transaction to remove a binding"))?;

    let current = binding(&transaction, &writing.reading(network), selector)?.ok_or_else(|| {
        Error::NotFound {
            kind: BINDING,
            id: format!("{} in the network {network}", binding_id(selector)),
        }
    })?;
    if &current.cursor != expected {
        return Err(Error::changed(
            BINDING,
            binding_id(selector),
            expected,
            &current.cursor,
        ));
    }

    let sequence = event_deleted(
        &transaction,
        network,
        kind::BINDING,
        &binding_id(selector),
        now,
    )?;

    transaction
        .execute(
            "DELETE FROM binding
             WHERE network = ?1 AND selector_kind = ?2 AND selector_value = ?3",
            params![network, selector.kind().as_str(), selector.value()],
        )
        .map_err(storage("removing a binding"))?;

    transaction
        .commit()
        .map_err(storage("committing the removal of a binding"))?;

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
