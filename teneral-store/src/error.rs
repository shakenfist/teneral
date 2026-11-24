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
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Error::Storage { source, .. } => Some(source.as_ref()),
            Error::SchemaAhead { .. } | Error::Pragma { .. } | Error::Malformed { .. } => None,
        }
    }
}
