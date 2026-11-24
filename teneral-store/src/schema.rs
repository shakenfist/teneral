//! The schema, and the forward-only migrations that reach it.
//!
//! The version a database is at lives in `PRAGMA user_version`, which
//! is a field in the file's own header. That is deliberate: it costs
//! no table, it is written in the same transaction as the statements
//! it describes, and a database that half-applied a migration is
//! therefore impossible rather than merely unlikely.
//!
//! Migrations only go forward. A database at a version this build
//! does not know is refused rather than interpreted, and there is no
//! downgrade path at all; the reasoning for both is on
//! [`Error::SchemaAhead`](crate::Error::SchemaAhead).

use std::path::Path;

use rusqlite::Connection;

use crate::error::{Error, Result};

/// The schema version this build understands.
///
/// A database at a lower version is migrated up to it on open; one
/// above it is refused. Adding a migration raises this, in the same
/// commit, and the test at the foot of this file is what notices if
/// only one of the two happened.
pub const SCHEMA_VERSION: u32 = 3;

/// One step from the version below it to its own.
struct Migration {
    /// The version the database is at once this has been applied.
    version: u32,
    /// The statements that take it there.
    sql: &'static str,
}

/// Every migration, in the order they apply.
///
/// The SQL lives in its own files rather than in string literals
/// here. It is the part of this crate a reviewer reads most closely,
/// and a `.sql` file is the form in which an identity or a constraint
/// can be read without a Rust compiler in the way.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: include_str!("migrations/0001_foundations.sql"),
    },
    Migration {
        version: 2,
        sql: include_str!("migrations/0002_resources.sql"),
    },
    Migration {
        version: 3,
        sql: include_str!("migrations/0003_bindings.sql"),
    },
];

/// Bring a database up to [`SCHEMA_VERSION`], or refuse it.
///
/// `path` is carried only so that a refusal can name the file: an
/// operator reading the message needs to know which database was
/// ahead, and by then the connection is no use for telling them.
pub(crate) fn migrate(conn: &mut Connection, path: &Path) -> Result<()> {
    let found = user_version(conn)?;

    if found > SCHEMA_VERSION {
        return Err(Error::schema_ahead(path, found, SCHEMA_VERSION));
    }

    for migration in MIGRATIONS.iter().filter(|m| m.version > found) {
        let transaction = conn
            .transaction()
            .map_err(|e| Error::storage(format!("starting migration {}", migration.version), e))?;

        transaction
            .execute_batch(migration.sql)
            .map_err(|e| Error::storage(format!("applying migration {}", migration.version), e))?;

        // The version has to move in the same transaction as the
        // statements it describes, or a crash between the two leaves
        // a database whose header disagrees with its own tables. It
        // cannot be a bound parameter -- no pragma can -- but the
        // value is a constant from the table above and never reaches
        // here from outside.
        transaction
            .execute_batch(&format!("PRAGMA user_version = {};", migration.version))
            .map_err(|e| {
                Error::storage(
                    format!("recording migration {} as applied", migration.version),
                    e,
                )
            })?;

        transaction.commit().map_err(|e| {
            Error::storage(format!("committing migration {}", migration.version), e)
        })?;
    }

    // Read the version back rather than assuming the loop above left
    // it right. The cost is one pragma per open, and what it catches
    // is a migration that applied its statements and failed to record
    // itself -- which would otherwise be invisible until the next
    // open applied it a second time.
    let reached = user_version(conn)?;
    if reached != SCHEMA_VERSION {
        return Err(Error::Malformed {
            detail: format!(
                "its schema version is {reached} after migrating, not the {SCHEMA_VERSION} \
                 the migrations should have left it at"
            ),
        });
    }

    Ok(())
}

/// The version written in the database's header.
pub(crate) fn user_version(conn: &Connection) -> Result<u32> {
    let found: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| Error::storage("reading the database's schema version", e))?;

    // The pragma is a signed 32-bit field, so the type system allows
    // values no migration here could have written. A negative one
    // means something else has been using this file, which is worth
    // saying rather than clamping.
    u32::try_from(found).map_err(|_| Error::Malformed {
        detail: format!("its schema version is {found}, which no teneral ever wrote"),
    })
}

#[cfg(test)]
mod tests {
    use super::{MIGRATIONS, SCHEMA_VERSION};
    use crate::db::Database;
    use crate::error::Error;
    use crate::test_support::TempStore;
    use rusqlite::Connection;

    #[test]
    fn migrations_are_contiguous_and_reach_the_declared_version() {
        // A migration added without raising SCHEMA_VERSION leaves a
        // store that migrates itself on every open and never records
        // it; raising the version without the migration leaves one
        // that claims a schema it does not have. Both are one-line
        // mistakes, and this is the only thing that would notice.
        for (index, migration) in MIGRATIONS.iter().enumerate() {
            let expected = u32::try_from(index).unwrap() + 1;
            assert_eq!(
                migration.version, expected,
                "migrations must be numbered from 1 with no gaps"
            );
        }

        let last = MIGRATIONS.last().unwrap();
        assert_eq!(last.version, SCHEMA_VERSION);
    }

    #[test]
    fn a_fresh_database_is_migrated_to_the_declared_version() {
        let store = TempStore::new("fresh");
        let database = Database::open(store.path()).unwrap();

        assert_eq!(database.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn reopening_a_migrated_database_applies_nothing() {
        let store = TempStore::new("reopen");

        let epoch = {
            let database = Database::open(store.path()).unwrap();
            database.epoch().clone()
        };

        // The migration generates the epoch, so an epoch that
        // survived the reopen is proof that the migration was not run
        // a second time -- a stronger statement than the version
        // being unchanged, which an idempotent migration would also
        // satisfy.
        let database = Database::open(store.path()).unwrap();
        assert_eq!(database.epoch(), &epoch);
        assert_eq!(database.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn a_database_ahead_of_the_code_refuses_to_open_and_names_both_versions() {
        let store = TempStore::new("ahead");
        let ahead = SCHEMA_VERSION + 1;

        drop(Database::open(store.path()).unwrap());

        // Written with the engine directly, because the point is a
        // database some future teneral migrated and this one cannot:
        // no code in this build will put that version in the header.
        let connection = Connection::open(store.path()).unwrap();
        connection
            .execute_batch(&format!("PRAGMA user_version = {ahead};"))
            .unwrap();
        drop(connection);

        let refusal = Database::open(store.path()).unwrap_err();

        match &refusal {
            Error::SchemaAhead {
                path,
                database,
                code,
            } => {
                assert_eq!(path, store.path());
                assert_eq!(*database, ahead);
                assert_eq!(*code, SCHEMA_VERSION);
            }
            other => panic!("expected a refusal naming both versions, got {other:?}"),
        }

        // An operator reads the message, not the variant, so the
        // message is what has to carry both numbers.
        let message = refusal.to_string();
        assert!(message.contains(&ahead.to_string()), "{message}");
        assert!(message.contains(&SCHEMA_VERSION.to_string()), "{message}");
    }
}
