//! What the tests inside this crate share.
//!
//! A real store, in a directory of its own, removed when the test that
//! asked for it finishes. Nothing here fakes the store: what these
//! tests check is behaviour that only means anything against the real
//! one, and a fake would be a second implementation of the rules that
//! keep one network's answers out of another's.
//!
//! The suites in `tests/` have a harness of their own, which serves a
//! socket and mints credentials. This is the smaller thing a unit test
//! needs, which is a store and somewhere to put it.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use teneral_store::SqliteStore;

/// Distinguishes the directories of tests running at the same time in
/// the same process.
static NEXT: AtomicU32 = AtomicU32::new(0);

/// A directory holding one test's store, removed when the test ends.
///
/// Rolled by hand rather than taken from a crate, as the store's own
/// suite does: a dependency whose only consumer is a temporary
/// directory would not survive a reading of the dependency list.
pub(crate) struct Directory {
    /// Where it is.
    path: PathBuf,
}

impl Directory {
    /// A directory named after `label`, for a test to put a store in.
    pub(crate) fn new(label: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "teneral-api-unit-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));

        std::fs::create_dir_all(&path).expect("a temporary directory for a test");

        Self { path }
    }

    /// A store in it.
    pub(crate) async fn store(&self) -> Arc<SqliteStore> {
        Arc::new(
            SqliteStore::open(self.path.join("store.sqlite"))
                .await
                .expect("a store in the test's own directory"),
        )
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        // The directory rather than the file: the write-ahead log
        // leaves a `-wal` and a `-shm` beside the database. A failure
        // to clean up is not the test's verdict and is not reported.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
