//! Opening a store's database, and the pragmas it is opened with.
//!
//! SQLite's defaults are chosen for a library embedded in anything at
//! all, and three of them are wrong for a store whose answers a
//! client acts on. Each is set here, on every connection the store
//! opens, and each is asserted by a test -- because a pragma is a
//! line that can be deleted without anything failing to compile, and
//! the cost of it being missing is paid at a power cut rather than at
//! the next build.

use std::path::Path;

use rusqlite::Connection;

use crate::error::{Error, Result};
use crate::identity::{self, Epoch};
use crate::schema;

/// An open store database: migrated, identified, and ready to be
/// written through.
///
/// The connection inside is private and stays private. A caller that
/// could reach it could run SQL that forgot to scope itself to a
/// network, which is the one mistake this crate exists to make
/// impossible rather than merely discouraged.
#[derive(Debug)]
pub struct Database {
    /// The write connection. There is exactly one, which is what
    /// makes the sequence numbers in the event log a total order
    /// rather than an interleaving that happens to look like one.
    writer: Connection,
    /// The epoch this database was created with.
    epoch: Epoch,
}

impl Database {
    /// Open the database at `path`, creating and migrating it if
    /// needed, or refuse it if a newer build wrote it.
    ///
    /// The order of what follows matters. The pragmas go on before
    /// any migration runs, because foreign key enforcement cannot be
    /// changed inside a transaction and a migration is a transaction;
    /// and the identity is read after migrating, because the first
    /// migration is what generates it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        let mut writer = Connection::open(path).map_err(|error| {
            Error::storage(format!("opening the store at {}", path.display()), error)
        })?;

        apply_connection_pragmas(&writer)?;
        apply_writer_pragmas(&writer)?;

        schema::migrate(&mut writer, path)?;
        let epoch = identity::read_epoch(&writer)?;

        Ok(Self { writer, epoch })
    }

    /// The epoch a consumer's cursor has to match to be resumable.
    ///
    /// Store-global rather than per-network on purpose: the sequence
    /// numbers it qualifies are a single order over every network's
    /// mutations, so an epoch per network would describe nothing.
    pub fn epoch(&self) -> &Epoch {
        &self.epoch
    }

    /// The schema version this database is at, which after a
    /// successful open is the version this build understands.
    ///
    /// Read out of the file rather than remembered from the open, so
    /// that it answers for the database rather than for what the
    /// migration believed it had done. Store-global: a schema belongs
    /// to the file, not to a network in it.
    pub fn schema_version(&self) -> Result<u32> {
        schema::user_version(&self.writer)
    }
}

/// The pragmas every connection the store opens carries, read or
/// write.
fn apply_connection_pragmas(conn: &Connection) -> Result<()> {
    // Foreign key enforcement is a property of the connection rather
    // than of the file, so a constraint the schema declares is
    // enforced only for whoever asked. The constraints it enables are
    // what stop a row in one network from referring to a parent in
    // another, which is a cross-network leak expressed as a dangling
    // reference.
    //
    // SQLite's own default is off. The amalgamation this crate
    // compiles is built with SQLITE_DEFAULT_FOREIGN_KEYS=1, so today
    // this line changes nothing -- which is exactly why it is here:
    // the invariant would otherwise rest on a compile flag inside a
    // dependency's build script, somewhere nobody looks, and it would
    // go quiet the day the engine is built differently. The test that
    // guards this line calls this function on a connection with
    // enforcement off, because no test against the bundled engine can
    // tell the pragma from the default.
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|error| Error::storage("enabling foreign key enforcement", error))?;

    Ok(())
}

/// The pragmas that only the write connection needs.
fn apply_writer_pragmas(conn: &Connection) -> Result<()> {
    // The write-ahead log, which is what lets readers hold a
    // consistent view of the database while the writer commits into
    // it. Without it a reader blocks the writer and the writer
    // blocks every reader, and a store whose readers are long-lived
    // by design would spend its life waiting.
    //
    // This is a query rather than a statement: the pragma reports the
    // mode the connection ended up in, and a request for WAL can be
    // declined -- by a database on a filesystem that cannot do the
    // shared memory it needs, among others. Ignoring the returned
    // value is how a store ends up in rollback mode believing it is
    // not.
    let mode: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .map_err(|error| Error::storage("selecting the write-ahead log", error))?;

    if !mode.eq_ignore_ascii_case("wal") {
        return Err(Error::Pragma {
            pragma: "journal_mode",
            wanted: "WAL",
            found: mode,
        });
    }

    // FULL rather than the NORMAL that WAL defaults to, and the
    // difference is the whole durability argument. Under WAL, NORMAL
    // does not fsync at commit: it survives a process crash and not a
    // loss of power. A lease that was acknowledged to a client and
    // then lost to a power cut is a duplicated address -- the store
    // will hand the same one to somebody else while the first client
    // goes on using it -- so the commit has to be durable before
    // anything can acknowledge it. The cost is one fsync per
    // transaction, and the honest way to reduce it later is to batch
    // the acknowledgements rather than to weaken this.
    conn.execute_batch("PRAGMA synchronous = FULL;")
        .map_err(|error| Error::storage("asking for a durable commit", error))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Database;
    use crate::test_support::TempStore;
    use rusqlite::{Connection, ErrorCode};

    /// What `PRAGMA synchronous` reads back as when it is `FULL`.
    /// SQLite reports the mode as a number: 0 is `OFF`, 1 `NORMAL`,
    /// 2 `FULL` and 3 `EXTRA`.
    const SYNCHRONOUS_FULL: i64 = 2;

    /// `NORMAL`, which is what the guard test below turns a
    /// connection down to so that the pragma has something to change.
    const SYNCHRONOUS_NORMAL: i64 = 1;

    /// What `PRAGMA synchronous` reads back as on `conn`.
    fn synchronous(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA synchronous", [], |row| row.get(0))
            .unwrap()
    }

    /// Append one event with the engine directly.
    ///
    /// There is no write path through the store yet, and this test
    /// module is asserting a property of the table rather than of any
    /// code above it: the sequence a row is given.
    fn append_event(database: &Database, network: &str, resource_id: &str) -> i64 {
        database
            .writer
            .execute(
                "INSERT INTO event_log (network, kind, resource_id, state, recorded_at)
                 VALUES (?1, 'test', ?2, NULL, 0)",
                rusqlite::params![network, resource_id],
            )
            .unwrap();

        database.writer.last_insert_rowid()
    }

    /// What `PRAGMA foreign_keys` reads back as on `conn`.
    fn foreign_keys(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn the_pragma_set_turns_foreign_key_enforcement_on() {
        // This is the test that holds the pragma line in place, and
        // it has to start from a connection with enforcement off to
        // do it. The engine this crate compiles defaults it on, so a
        // test that merely observed enforcement would pass with the
        // line deleted -- and the whole point of the line is that the
        // store does not depend on that default.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        assert_eq!(foreign_keys(&conn), 0, "the connection should start off");

        super::apply_connection_pragmas(&conn).unwrap();

        assert_eq!(foreign_keys(&conn), 1);
    }

    #[test]
    fn the_write_connection_enforces_foreign_keys() {
        let store = TempStore::new("foreign-keys");
        let database = Database::open(store.path()).unwrap();

        assert_eq!(foreign_keys(&database.writer), 1);

        // The pair of tables is made here rather than borrowed from
        // the schema, so that this test goes on meaning the same
        // thing as the schema grows constraints of its own: what is
        // being asserted is that the connection enforces references
        // at all, not that one particular table has one.
        //
        // Note that this asserts the property rather than guarding
        // the line that provides it: the bundled engine would enforce
        // references here with the pragma gone. The test above is the
        // guard.
        database
            .writer
            .execute_batch(
                "CREATE TABLE parent (id INTEGER PRIMARY KEY) STRICT;
                 CREATE TABLE child (
                     id INTEGER PRIMARY KEY,
                     parent_id INTEGER NOT NULL REFERENCES parent(id)
                 ) STRICT;",
            )
            .unwrap();

        let refused = database
            .writer
            .execute("INSERT INTO child (id, parent_id) VALUES (1, 404)", []);

        match refused {
            Err(rusqlite::Error::SqliteFailure(failure, _)) => {
                assert_eq!(failure.code, ErrorCode::ConstraintViolation);
            }
            other => panic!("a child row with no parent was accepted: {other:?}"),
        }
    }

    #[test]
    fn the_write_connection_uses_the_write_ahead_log() {
        let store = TempStore::new("journal-mode");
        let database = Database::open(store.path()).unwrap();

        let mode: String = database
            .writer
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();

        assert_eq!(mode, "wal");
    }

    #[test]
    fn the_write_connection_fsyncs_at_every_commit() {
        let store = TempStore::new("synchronous");
        let database = Database::open(store.path()).unwrap();

        // As with foreign keys, this asserts the property and does
        // not guard the line that provides it: the engine this crate
        // compiles would read FULL here with the pragma deleted,
        // because it moves neither SQLITE_DEFAULT_SYNCHRONOUS nor
        // SQLITE_DEFAULT_WAL_SYNCHRONOUS off 2. The test below is the
        // guard.
        assert_eq!(synchronous(&database.writer), SYNCHRONOUS_FULL);
    }

    #[test]
    fn the_pragma_set_turns_durable_commits_on() {
        // The guard for the `synchronous = FULL` line. Its
        // precondition has to be built in a particular order to be
        // worth anything.
        //
        // Entering WAL sets synchronous to the compile-time WAL
        // default, so turning it down *before* the journal mode
        // changes would be undone by the change, and this test would
        // pass with the line deleted -- the exact failure it exists
        // to catch. So: enter WAL first, turn synchronous down
        // afterwards, and only then run the pragma set. Asking for
        // WAL when WAL is already on is a no-op, which is what leaves
        // the turned-down value there for the function under test to
        // find.
        let store = TempStore::new("synchronous-guard");
        let conn = Connection::open(store.path()).unwrap();

        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal", "the precondition needs a WAL database");

        conn.execute_batch("PRAGMA synchronous = NORMAL;").unwrap();
        assert_eq!(
            synchronous(&conn),
            SYNCHRONOUS_NORMAL,
            "the connection should start less durable than it ends"
        );

        super::apply_writer_pragmas(&conn).unwrap();

        assert_eq!(synchronous(&conn), SYNCHRONOUS_FULL);
    }

    #[test]
    fn two_stores_get_different_epochs() {
        let first = TempStore::new("epoch-first");
        let second = TempStore::new("epoch-second");

        let first = Database::open(first.path()).unwrap();
        let second = Database::open(second.path()).unwrap();

        // Sixteen random bytes as lower-case hex. The length is
        // checked because the value is generated in SQL, where a
        // misplaced argument produces a plausible-looking string
        // rather than an error.
        assert_eq!(first.epoch().as_str().len(), 32);
        assert!(
            first
                .epoch()
                .as_str()
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );

        assert_ne!(first.epoch(), second.epoch());
    }

    #[test]
    fn the_event_sequence_is_never_reused() {
        let store = TempStore::new("event-sequence");
        let database = Database::open(store.path()).unwrap();

        // Two networks, because the sequence is one order over both
        // of them: a store that reset the counter would do it for
        // events neither network can see belong to the other.
        let issued = [
            append_event(&database, "net-a", "a1"),
            append_event(&database, "net-b", "b1"),
            append_event(&database, "net-a", "a2"),
        ];
        let high_water = issued.iter().copied().max().unwrap();

        // The log is bounded, so eviction deletes from it. This is
        // that, with the bound as small as it goes.
        database
            .writer
            .execute_batch("DELETE FROM event_log;")
            .unwrap();

        let reissued = [
            append_event(&database, "net-b", "b2"),
            append_event(&database, "net-a", "a3"),
        ];

        for sequence in reissued {
            assert!(
                sequence > high_water,
                "sequence {sequence} was issued again after {high_water}; a resuming consumer \
                 would be handed somebody else's history"
            );
        }
        assert!(reissued[1] > reissued[0]);
    }
}
