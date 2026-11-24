//! Shared scaffolding for this crate's own tests.
//!
//! Compiled only under `cfg(test)`, so nothing here is part of the
//! crate a consumer links.

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};

use rusqlite::Connection;

use crate::clock::Clock;
use crate::sqlite::SqliteStore;
use crate::store::BindingStore;

/// Distinguishes the directories of tests that run at the same time
/// in the same process.
static NEXT: AtomicU32 = AtomicU32::new(0);

/// A directory holding one test's database, removed when the test
/// finishes.
///
/// Rolled by hand rather than taken from a crate, because the store's
/// dependency list is short enough to be read in review and a
/// dependency whose only consumer is a temporary directory would not
/// survive that reading.
pub(crate) struct TempStore {
    /// The directory, which is what gets removed.
    directory: PathBuf,
    /// The database file inside it.
    database: PathBuf,
}

impl TempStore {
    /// A fresh, empty directory for a test's database. `label`
    /// appears in the path so that a leftover directory says which
    /// test left it.
    pub(crate) fn new(label: &str) -> Self {
        let mut directory = std::env::temp_dir();
        directory.push(format!(
            "teneral-store-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));

        std::fs::create_dir_all(&directory).expect("a temporary directory for a test's database");
        let database = directory.join("store.sqlite");

        Self {
            directory,
            database,
        }
    }

    /// Where the test's database should be opened.
    pub(crate) fn path(&self) -> &Path {
        &self.database
    }
}

impl Drop for TempStore {
    fn drop(&mut self) {
        // The directory goes rather than the file: the write-ahead
        // log leaves a `-wal` and a `-shm` beside the database, and
        // removing only the one named file would leave those behind
        // in whatever the system's temporary directory is.
        //
        // A failure to clean up is not the test's verdict, so it is
        // not reported here. A test that failed has usually left
        // something worth looking at anyway.
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// A clock a test moves by hand.
///
/// The store's notion of now is injected so that this can exist. A
/// test that needed to wait for a timeout would otherwise have to
/// sleep, and a suite with real waits in it is one somebody later
/// makes faster -- shortening exactly the margin the test was
/// measuring.
#[derive(Debug)]
pub(crate) struct TestClock {
    /// Now, in seconds since the Unix epoch.
    now: AtomicI64,
}

impl TestClock {
    /// A clock stopped at `start`.
    pub(crate) fn new(start: i64) -> Self {
        Self {
            now: AtomicI64::new(start),
        }
    }

    /// Move the clock forward.
    pub(crate) fn advance(&self, seconds: i64) {
        self.now.fetch_add(seconds, Ordering::AcqRel);
    }
}

impl Clock for TestClock {
    fn unix_seconds(&self) -> i64 {
        self.now.load(Ordering::Acquire)
    }
}

/// The instant the fixture's clock starts at: a round number in 2023,
/// chosen only because a timestamp in a failure message is easier to
/// read when it is not today's.
pub(crate) const FIXTURE_NOW: i64 = 1_700_000_000;

/// The first of the fixture's two networks.
pub(crate) const ALPHA: &str = "alpha";
/// The second, which uses the same addresses as the first.
pub(crate) const BETA: &str = "beta";
/// The name both networks give their pool. The same name in two
/// networks is two different ranges, and the fixture uses one name to
/// make sure nothing in the store is keyed on it alone.
pub(crate) const POOL: &str = "pool";
/// The lowest address in both networks' pools.
pub(crate) const POOL_FIRST: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 10);
/// The highest address in both networks' pools.
pub(crate) const POOL_LAST: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 100);
/// The namespace the first network's interface lives in.
pub(crate) const ALPHA_NAMESPACE: &str = "ns-alpha";
/// The namespace the second network's interface lives in.
pub(crate) const BETA_NAMESPACE: &str = "ns-beta";
/// The interface name both networks use. The same name in two
/// namespaces is two different interfaces, which is why the namespace
/// is half of an attachment's identity.
pub(crate) const IFNAME: &str = "eth0";

/// A store holding two networks that use the same addresses.
///
/// This is the fixture every test that touches the store starts from,
/// and the reason is the project's central correctness property: two
/// networks may legitimately use the same address space, so a test
/// with one network in it proves nothing about whether an answer came
/// from the right one. Both networks here have a pool over the same
/// addresses, both pools have the same name, and both networks have
/// an interface with the same name in a namespace of its own. Every
/// one of those collisions is deliberate: each is a thing the store
/// must not be keyed on.
pub(crate) struct TwoNetworks {
    /// The store, which drops first so that its threads are joined
    /// before the directory below is removed.
    pub(crate) store: SqliteStore,
    /// The clock the store stamps writes with.
    pub(crate) clock: Arc<TestClock>,
    /// The directory holding the database, removed last.
    directory: TempStore,
}

impl TwoNetworks {
    /// Build the fixture, through the store's own interface.
    ///
    /// Through the interface rather than by writing rows, so that the
    /// fixture is also the first test of every write it uses: a
    /// create that stopped working would fail every test in the suite
    /// rather than leaving them passing against a database no code
    /// path can produce.
    pub(crate) async fn new(label: &str) -> Self {
        let directory = TempStore::new(label);
        let clock = Arc::new(TestClock::new(FIXTURE_NOW));

        let store = SqliteStore::with_clock(directory.path(), Arc::clone(&clock) as Arc<dyn Clock>)
            .await
            .expect("a store in a fresh directory");

        for network in [ALPHA, BETA] {
            store.create_network(network).await.expect("a network");
            store
                .create_address_range(network, POOL, POOL_FIRST, POOL_LAST)
                .await
                .expect("a pool");
        }

        store
            .attach_interface(ALPHA, ALPHA_NAMESPACE, IFNAME)
            .await
            .expect("an interface for the first network");
        store
            .attach_interface(BETA, BETA_NAMESPACE, IFNAME)
            .await
            .expect("an interface for the second network");

        Self {
            store,
            clock,
            directory,
        }
    }

    /// Where the database is, for a test that has to go behind the
    /// store.
    pub(crate) fn path(&self) -> &Path {
        self.directory.path()
    }

    /// A connection to the database that does not go through the
    /// store at all.
    ///
    /// Only a test may have this, and only for the two things it is
    /// the only way to do: producing a state the interface cannot
    /// produce, so that the database can be seen refusing it; and
    /// reading the event log, which no trait method exposes yet.
    pub(crate) fn behind_the_store(&self) -> Connection {
        let connection = Connection::open(self.path()).expect("a connection to the test database");
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .expect("foreign key enforcement");

        connection
    }
}

impl TwoNetworks {
    /// A second store over the same database, for a test that has to
    /// ask whether a write survived the connection that made it.
    ///
    /// A separate store rather than a reopen of this one, because the
    /// fixture owns the directory and dropping the store would mean
    /// giving that up. What it buys is the assertion that matters:
    /// another connection sees only what has been committed, so a
    /// value readable through this one is a value that was durable
    /// before the caller was told about it.
    pub(crate) async fn second_store(&self) -> SqliteStore {
        SqliteStore::with_clock(
            self.directory.path(),
            Arc::clone(&self.clock) as Arc<dyn Clock>,
        )
        .await
        .expect("a second store over the same database")
    }
}
