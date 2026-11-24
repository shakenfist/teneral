//! What the store returns when it will not do what was asked.
//!
//! Two things shape this type. The first is that a caller outside
//! this crate must be able to tell a refusal it should act on from a
//! failure of the machinery underneath: a database written by a newer
//! build is an operator's problem and says so in one sentence, where
//! a disk error is not. The second is that the SQLite error type does
//! not appear in any signature here, so a consumer of the store
//! neither links it nor matches on it; the underlying error is still
//! reachable through `source()` for a log, which is the only place it
//! is any use.

use std::error::Error as StdError;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::cursor::Cursor;

/// The result of a store operation.
pub type Result<T> = std::result::Result<T, Error>;

/// Why a store operation did not happen.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The storage engine refused an operation, or the medium under
    /// it did. `context` says what the store was doing at the time,
    /// because the engine's own message rarely does.
    Storage {
        /// What was being attempted, as a phrase that completes
        /// "while ...".
        context: String,
        /// The engine's error, for a log rather than for a match.
        source: Box<dyn StdError + Send + Sync>,
    },

    /// The database on disk carries a schema version this build does
    /// not know, so a newer teneral wrote it.
    ///
    /// Opening it anyway is the failure this whole project is built
    /// to avoid: most of the columns this build knows would still be
    /// there, and the ones it did not know about would be the ones
    /// that mattered. Migrations are forward-only and there is no
    /// downgrade, because a downgrade that drops a column drops
    /// bindings an operator configured.
    SchemaAhead {
        /// The database that was asked for.
        path: PathBuf,
        /// The version written in the file.
        database: u32,
        /// The version this build understands.
        code: u32,
    },

    /// A pragma the store's correctness rests on did not take effect.
    ///
    /// Worth its own variant because the engine reports some of these
    /// by returning a value rather than by failing: a request for the
    /// write-ahead log can be declined, and the decline arrives as
    /// the mode the connection is actually in.
    Pragma {
        /// The pragma that was set.
        pragma: &'static str,
        /// What the store asked for.
        wanted: &'static str,
        /// What the connection reports instead.
        found: String,
    },

    /// The file opened, but it is not shaped like a teneral store.
    Malformed {
        /// What was wrong with it.
        detail: String,
    },

    /// An operation named a network the store does not have.
    ///
    /// Distinct from a resource inside a network being absent,
    /// because the two have different causes and different fixes: a
    /// caller that has the network wrong is misconfigured, where one
    /// that has the resource wrong is usually racing something.
    UnknownNetwork {
        /// The network that was named.
        network: String,
    },

    /// The resource the operation was to act on is not there.
    NotFound {
        /// What kind of resource was being looked for.
        kind: &'static str,
        /// Which one, within the network that was asked about.
        id: String,
    },

    /// The operation would have collided with something already
    /// configured, and the thing it collided with is named.
    ///
    /// Naming it is the point of the variant. "An interface belongs
    /// to exactly one network" is the single most important refusal
    /// the store makes, and an operator who is told only that the
    /// claim failed has to go looking for which network holds it --
    /// which is the moment they are most likely to guess.
    Conflict {
        /// What kind of resource was being written.
        kind: &'static str,
        /// The one that was asked for.
        id: String,
        /// What is in the way, in a form an operator can act on.
        held_by: String,
    },

    /// The resource moved on between the caller reading it and asking
    /// for it to be changed.
    ///
    /// The store offers compare-and-set rather than a read followed
    /// by a write because the latter is correct only until something
    /// gets in between. This is that refusal: both positions are
    /// reported, so a caller can tell a change it raced from a cursor
    /// it never held, and so that an epoch from a restored backup is
    /// visible rather than merely unequal.
    Changed {
        /// What kind of resource it is.
        kind: &'static str,
        /// Which one.
        id: String,
        /// The position the caller expected it to be at.
        expected: String,
        /// The position it is actually at.
        found: String,
    },

    /// A read scoped to one network produced a row belonging to
    /// another, and the store refused to serve it.
    ///
    /// By construction this cannot happen, which is exactly why it is
    /// an error and is counted rather than left to a comment: a store
    /// that noticed a cross-network answer and served it anyway would
    /// have turned the one failure this project exists to prevent
    /// into a metric. Both networks are named, because the pair is
    /// what an operator or a test needs in order to tell which of the
    /// two scopes was the wrong one.
    CrossNetwork {
        /// What kind of resource was being read.
        kind: &'static str,
        /// Which one, as its identity reads within a network.
        id: String,
        /// The network the caller asked about.
        asked: String,
        /// The network the row came from.
        found: String,
    },

    /// A name or a value the store will not store.
    ///
    /// The store validates the few things it would otherwise have to
    /// encode or compare later: a name it renders into an event's
    /// identity cannot contain the separator used to render it, and
    /// an empty or control-laden name is one that will be unreadable
    /// in every log that reports it.
    Invalid {
        /// What was being named or set.
        what: &'static str,
        /// Why it was refused.
        detail: String,
    },

    /// The store could not start a thread it needs.
    ///
    /// The database is confined to threads the store owns, so one it
    /// cannot start is a store it cannot open or a view it cannot
    /// hand out. Distinct from a storage failure because the medium
    /// is fine and the host is not: the fix is a limit somewhere in
    /// the process, not a disk.
    Threads {
        /// What the thread was to have done.
        context: &'static str,
        /// The operating system's complaint.
        source: Box<dyn StdError + Send + Sync>,
    },

    /// The store cannot do any more work: the thread that owns the
    /// database has stopped.
    ///
    /// A variant of its own because the alternative is worse than an
    /// error. The store confines the database to threads it owns and
    /// speaks to them over channels, so a thread that has gone leaves
    /// a channel that no longer answers -- and a caller told nothing
    /// would be a caller that believes a write it never got happened.
    Unavailable {
        /// What the caller was trying to do.
        context: String,
    },
}

impl Error {
    /// Wrap an engine error with what the store was doing.
    ///
    /// Taken by value and boxed here so that no caller has to name
    /// the engine's error type, and so that adding the context is the
    /// cheaper path than discarding it.
    pub(crate) fn storage(context: impl Into<String>, source: rusqlite::Error) -> Self {
        Error::Storage {
            context: context.into(),
            source: Box::new(source),
        }
    }

    /// The refusal of a compare-and-set whose expected position is
    /// not where the resource is.
    ///
    /// Both cursors are rendered here rather than carried as values,
    /// so that the error type does not grow a field whose only
    /// purpose is to be printed, and so that the epoch travels with
    /// the sequence number in the message an operator reads.
    pub(crate) fn changed(
        kind: &'static str,
        id: impl Into<String>,
        expected: &Cursor,
        found: &Cursor,
    ) -> Self {
        Error::Changed {
            kind,
            id: id.into(),
            expected: expected.to_string(),
            found: found.to_string(),
        }
    }

    /// The refusal that follows a thread owning the database having
    /// stopped.
    pub(crate) fn unavailable(context: impl Into<String>) -> Self {
        Error::Unavailable {
            context: context.into(),
        }
    }

    /// The refusal that follows the host declining to start a thread.
    pub(crate) fn threads(context: &'static str, source: std::io::Error) -> Self {
        Error::Threads {
            context,
            source: Box::new(source),
        }
    }

    /// The refusal to open a database a newer build wrote.
    pub(crate) fn schema_ahead(path: &Path, database: u32, code: u32) -> Self {
        Error::SchemaAhead {
            path: path.to_path_buf(),
            database,
            code,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Storage { context, source } => {
                write!(f, "while {context}: {source}")
            }
            Error::SchemaAhead {
                path,
                database,
                code,
            } => write!(
                f,
                "{} has schema version {database}, which is newer than the version this build \
                 understands ({code}); a newer teneral wrote it and there is no downgrade",
                path.display()
            ),
            Error::Pragma {
                pragma,
                wanted,
                found,
            } => write!(
                f,
                "the database would not accept {pragma} = {wanted}; it reports {found}"
            ),
            Error::Malformed { detail } => {
                write!(f, "the database is not a teneral store: {detail}")
            }
            Error::UnknownNetwork { network } => {
                write!(f, "there is no network called {network}")
            }
            Error::NotFound { kind, id } => write!(f, "there is no {kind} {id}"),
            Error::Conflict { kind, id, held_by } => {
                write!(f, "the {kind} {id} cannot be written: {held_by}")
            }
            Error::Changed {
                kind,
                id,
                expected,
                found,
            } => write!(
                f,
                "the {kind} {id} was at {expected} when it was read and is at {found} now, so the \
                 change was computed from a state it has left"
            ),
            Error::CrossNetwork {
                kind,
                id,
                asked,
                found,
            } => write!(
                f,
                "the {kind} {id} was read in the network {asked} and belongs to {found}; the \
                 store refused the answer rather than serving one network's configuration to \
                 another"
            ),
            Error::Invalid { what, detail } => write!(f, "that {what} will not do: {detail}"),
            Error::Threads { context, source } => {
                write!(
                    f,
                    "the store could not start a thread to {context}: {source}"
                )
            }
            Error::Unavailable { context } => write!(
                f,
                "the store has stopped and cannot {context}; the thread owning its database is \
                 gone"
            ),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Error::Storage { source, .. } | Error::Threads { source, .. } => Some(source.as_ref()),
            // Every other variant is the store's own refusal rather
            // than a failure underneath it, so there is nothing below
            // it to report.
            _ => None,
        }
    }
}
