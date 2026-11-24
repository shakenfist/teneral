//! Shared scaffolding for this crate's own tests.
//!
//! Compiled only under `cfg(test)`, so nothing here is part of the
//! crate a consumer links.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

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
