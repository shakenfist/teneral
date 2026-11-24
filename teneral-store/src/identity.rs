//! The store's own identity.

use std::fmt;

use rusqlite::Connection;

use crate::error::{Error, Result};

/// A store's epoch: the half of a cursor that says whose sequence
/// numbers a consumer is holding.
///
/// It is generated once, when the database is created, and never
/// changes while that database is the store. A different epoch is how
/// a consumer learns that the sequence numbers it remembers cannot be
/// continued -- because the store was restored from a backup, or
/// because its history was truncated past the point the consumer had
/// reached -- and that it must start again from a snapshot rather
/// than resume. A consumer comparing epochs should treat the value as
/// opaque: it is hex today and its only contract is that two stores
/// do not share one.
///
/// A type of its own rather than a `String`, because it travels
/// alongside network names and resource identities, and a mix-up
/// between those is exactly the kind of thing that type checks for
/// free.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Epoch(String);

impl Epoch {
    /// The epoch as it is written on the wire and in a log.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Epoch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Read the epoch the database was created with.
///
/// The identity row is written by the first migration, so a database
/// that has been migrated has exactly one -- the table's primary key
/// and its CHECK constraint together make a second row impossible.
/// An absent row therefore means the file is not a store that this
/// code built, which is worth a distinct complaint rather than an
/// empty answer.
pub(crate) fn read_epoch(conn: &Connection) -> Result<Epoch> {
    let epoch: Option<String> = conn
        .query_row("SELECT epoch FROM store_identity WHERE id = 1", [], |row| {
            row.get(0)
        })
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(Error::storage("reading the store's epoch", other)),
        })?;

    match epoch {
        Some(epoch) => Ok(Epoch(epoch)),
        None => Err(Error::Malformed {
            detail: "it has no identity row, so no epoch to continue a cursor from".to_owned(),
        }),
    }
}
