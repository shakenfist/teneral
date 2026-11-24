//! The store, as an asynchronous interface over threads it owns.
//!
//! SQLite is a synchronous library and that is not a problem to be
//! hidden; it is a problem to be *located*. This module is where it
//! is located.
//!
//! The constraint that fixes the design is small and sharp. A read
//! transaction borrows its connection, and a connection may be moved
//! between threads but not shared, so a transaction cannot be held
//! across an await in any future a multi-threaded executor will
//! accept. A consistent view of the store that a caller can hold
//! across its own awaits is the whole of the second demand the API
//! makes of this layer. The two cannot both be true inside the
//! caller's task, so the transaction lives somewhere that is not a
//! future at all:
//!
//! - **One writer thread**, owning the write connection, fed by a
//!   channel. A write is a message; its reply carries the cursor the
//!   write was assigned. One writer is what makes the sequence
//!   numbers a total order rather than an interleaving that looks
//!   like one, and it is what puts every commit's `fsync` on a thread
//!   this store owns.
//! - **One thread per live view**, each holding its read transaction
//!   on its own stack for as long as the view is held. The
//!   write-ahead log makes concurrent readers free, so these do not
//!   contend with the writer or with each other. A view is an
//!   asynchronous object whose methods are channel round trips.
//!
//! The threads are the store's own `std::thread`s rather than a
//! runtime's blocking pool, and that is deliberate rather than
//! incidental. A shared pool is shared with every other blocking call
//! in the process, so a writer in it can be crowded out by work that
//! has nothing to do with this store -- and the failure that produces
//! is one network's request waiting on another network's `fsync`,
//! which no single-network test can see. A thread of its own cannot
//! be crowded out of anything. It also keeps the choice of runtime
//! with the binary: the only runtime machinery this crate uses is a
//! channel, which needs no runtime to exist.
//!
//! Each channel here is half of a pair. The request direction is a
//! channel that the owning thread blocks on, and the reply is a
//! one-shot that the caller awaits; nothing else in the crate needs
//! to know a runtime exists.
//!
//! One place blocking does still happen on the caller's thread, and
//! it is named here rather than left for somebody to find. Both
//! `Drop` implementations below end a thread and then join it, and a
//! join blocks whoever is dropping. For the store that happens once,
//! at shutdown. For a view it happens every time one is dropped,
//! which in a request handler is once a request -- and what it waits
//! for is a parked thread waking, a read transaction ending and a
//! stack unwinding, none of which touches the disk. The exception is
//! a view dropped while one of its own queries is still running,
//! which only a cancelled future can arrange: that join waits for the
//! query. Joining is not optional, because the transaction lives on
//! that thread's stack and the thread returning is the only thing
//! that can end it; a view that skipped the join would leave a read
//! transaction open for the life of the process. The honest summary
//! is that the `fsync` is on a thread this store owns and the
//! thread's own teardown is not.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};

use async_trait::async_trait;
use rusqlite::Connection;
use tokio::sync::{mpsc, oneshot};

use crate::clock::{Clock, SystemClock};
use crate::cursor::Cursor;
use crate::db::{self, Database};
use crate::error::{Error, Result};
use crate::guard::CrossNetworkGuard;
use crate::identity::Epoch;
use crate::lease;
use crate::model::{AddressRange, Attachment, Binding, Explanation, Lease, Network};
use crate::ops::{self, Reading, Writing};
use crate::selector::{ClientIdentity, Selector};
use crate::store::{BindingStore, ReadHandle};

/// A unit of work for the writer thread.
///
/// A boxed closure rather than an enum of commands. Both work; this
/// one keeps each operation's SQL and its reply in one place, so an
/// operation that forgot to emit its event is an asymmetry inside one
/// function rather than a missing arm somewhere else.
type WriteJob = Box<dyn FnOnce(&mut Database) + Send + 'static>;

/// A unit of work for a view's thread.
///
/// The job is handed the connection rather than the transaction, and
/// the transaction is what the thread is holding. That is not a
/// loophole: a transaction derefs to the connection it is running on,
/// so a statement run through this reference is inside the view's
/// transaction and sees the view's position. Passing the transaction
/// itself would mean naming two lifetimes inside a boxed closure, for
/// no gain.
type ReadJob = Box<dyn FnOnce(&Connection) + Send + 'static>;

/// How many threads the store currently has, by kind.
///
/// Counted because the threads are the store's own and a leaked one
/// is invisible otherwise -- a view whose thread outlived it would go
/// unnoticed until a much later test deadlocked on something
/// unrelated. It is also the quantity a bound on live views would be
/// enforced against, which is the next thing to want here.
#[derive(Debug, Default)]
pub(crate) struct ThreadCensus {
    /// Threads holding a view's read transaction.
    views: AtomicUsize,
    /// Threads owning a write connection. One, or none once the
    /// store has been dropped.
    writers: AtomicUsize,
}

impl ThreadCensus {
    /// Views currently alive.
    pub(crate) fn views(&self) -> usize {
        self.views.load(Ordering::Acquire)
    }

    /// Writers currently alive.
    ///
    /// Only a test asks. In a running daemon the answer is always one
    /// until the store is dropped, at which point there is nothing
    /// left to ask -- which is exactly why a test has to be able to.
    #[cfg(test)]
    pub(crate) fn writers(&self) -> usize {
        self.writers.load(Ordering::Acquire)
    }
}

/// A thread's membership of the census, given up when the thread ends.
///
/// A guard rather than a decrement at the end of the thread body, so
/// that a thread that ends by panicking is still counted out. A
/// census that only counted orderly exits would report exactly the
/// wrong number in the case worth knowing about.
struct Enlisted {
    /// The census this thread is counted in.
    census: Arc<ThreadCensus>,
    /// Whether it is a view's thread rather than the writer.
    view: bool,
}

impl Enlisted {
    /// Count this thread in.
    fn new(census: Arc<ThreadCensus>, view: bool) -> Self {
        let counter = if view { &census.views } else { &census.writers };
        counter.fetch_add(1, Ordering::AcqRel);

        Self { census, view }
    }
}

impl Drop for Enlisted {
    fn drop(&mut self) {
        let counter = if self.view {
            &self.census.views
        } else {
            &self.census.writers
        };
        counter.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The store, backed by a SQLite database on this host.
///
/// Opening one starts the writer thread; dropping one stops it and
/// waits for it, so a dropped store leaves nothing behind. The type
/// is `Send + Sync` and every operation takes `&self`, so it is
/// shared by reference or in an `Arc` rather than cloned.
#[derive(Debug)]
pub struct SqliteStore {
    /// Where writes are sent. `None` only while dropping, which is
    /// how the writer thread is told there will be no more work: the
    /// channel's last sender going away is the signal.
    writes: Option<mpsc::UnboundedSender<WriteJob>>,
    /// The writer thread, joined on drop.
    writer: Option<JoinHandle<()>>,
    /// The database file, so that a view can open its own connection
    /// to it.
    path: PathBuf,
    /// This store's epoch, read once at open and fixed thereafter.
    epoch: Epoch,
    /// The schema version the database is at, likewise.
    schema_version: u32,
    /// Where the time stamped on every write comes from. Held here
    /// rather than on the writer thread's database so that each write
    /// job can carry its own reference to it alongside the mutable
    /// borrow of the connection.
    clock: Arc<dyn Clock>,
    /// Live thread counts, shared with every thread the store starts.
    census: Arc<ThreadCensus>,
    /// The cross-network check, shared with every view this store
    /// hands out so that the count is the store's rather than one
    /// view's.
    guard: Arc<CrossNetworkGuard>,
}

impl SqliteStore {
    /// Open the store at `path`, creating and migrating the database
    /// if it is not there, using the host's clock.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::with_clock(path, Arc::new(SystemClock)).await
    }

    /// Open the store at `path`, taking the time from `clock`.
    ///
    /// Every open goes through here, including the one above. The
    /// whole of the open -- the file, the pragmas, the migrations and
    /// the identity -- happens on the writer thread rather than on
    /// the caller's, so that the one blocking thing this crate does
    /// never happens anywhere but on a thread it owns. The caller
    /// awaits the answer.
    pub async fn with_clock(path: impl AsRef<Path>, clock: Arc<dyn Clock>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let census = Arc::new(ThreadCensus::default());

        let (writes, mut jobs) = mpsc::unbounded_channel::<WriteJob>();
        let (ready, opened) = oneshot::channel::<Result<(Epoch, u32)>>();

        let thread_path = path.clone();
        let thread_census = Arc::clone(&census);
        let writer = thread::Builder::new()
            .name("teneral-store-writer".to_owned())
            .spawn(move || {
                let _enlisted = Enlisted::new(thread_census, false);

                let mut database = match open_database(&thread_path) {
                    Ok(opened) => opened,
                    Err(refusal) => {
                        // Nothing to own, so nothing to wait for. The
                        // caller gets the refusal and joins this
                        // thread.
                        let _ = ready.send(Err(refusal));
                        return;
                    }
                };

                let identity = database
                    .schema_version()
                    .map(|version| (database.epoch().clone(), version));
                let reported = identity.is_ok();
                if ready.send(identity).is_err() || !reported {
                    return;
                }

                // One job at a time, in the order they were sent,
                // until the last sender goes away. That is both the
                // single-writer property the total order rests on and
                // the shutdown path: dropping the store drops the
                // sender, this returns `None`, and the thread ends
                // with the connection closed.
                while let Some(job) = jobs.blocking_recv() {
                    job(&mut database);
                }
            })
            .map_err(|error| Error::threads("own the store's write connection", error))?;

        match opened.await {
            Ok(Ok((epoch, schema_version))) => Ok(Self {
                writes: Some(writes),
                writer: Some(writer),
                path,
                epoch,
                schema_version,
                clock,
                census,
                guard: Arc::new(CrossNetworkGuard::default()),
            }),
            Ok(Err(refusal)) => {
                let _ = writer.join();
                Err(refusal)
            }
            // The thread ended without answering, which means it
            // panicked: there is no other path out of the code above.
            Err(_) => {
                let _ = writer.join();
                Err(Error::unavailable("open its database"))
            }
        }
    }

    /// How many views of this store are currently held.
    ///
    /// Each costs a thread for as long as it is held, which is what
    /// makes this worth being able to ask: it is the quantity any
    /// bound on concurrent views would be enforced against, and the
    /// number that should return to zero when a consumer has finished
    /// with the store.
    pub fn live_views(&self) -> usize {
        self.census.views()
    }

    /// The thread counts, for a test that outlives the store.
    ///
    /// A test is the only thing that wants this: it has to be able to
    /// ask whether the threads have gone *after* the store that owned
    /// them has been dropped, which the method above cannot answer
    /// because there is no longer a store to call it on.
    #[cfg(test)]
    pub(crate) fn census_for_test(&self) -> Arc<ThreadCensus> {
        Arc::clone(&self.census)
    }

    /// Run `job` on the writer thread and await its answer.
    ///
    /// The queue is unbounded, deliberately. A bounded queue would
    /// have to make a full one wait, and the only way to wait on it
    /// from here without a runtime is to block the caller's thread --
    /// which is precisely the thing this design exists to avoid. What
    /// bounds the work in flight is the number of callers.
    async fn write<T, F>(&self, context: &'static str, job: F) -> Result<T>
    where
        F: FnOnce(&mut Database) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let (reply, answer) = oneshot::channel();

        let writes = self
            .writes
            .as_ref()
            .ok_or_else(|| Error::unavailable(context))?;

        writes
            .send(Box::new(move |database| {
                // The receiver is gone if the caller's future was
                // dropped before the reply arrived, which is not an
                // error: the write has happened either way, and the
                // store's history is where it is recorded.
                let _ = reply.send(job(database));
            }))
            .map_err(|_| Error::unavailable(context))?;

        answer.await.map_err(|_| Error::unavailable(context))?
    }

    /// Read something on the writer thread, outside any transaction.
    ///
    /// The two reads this store answers without a view go through
    /// here. The write connection is used because it is the one that
    /// always sees the latest commit, and because a thread per
    /// point read would cost more than the read does. The price is
    /// that such a read waits behind whatever write is in progress,
    /// which for the indexed single-row reads this serves is a
    /// transaction's worth of waiting; if that ever shows up in a
    /// measurement, the answer is a small pool of reader threads
    /// rather than a change to any of these signatures.
    async fn read_now<T, F>(&self, context: &'static str, job: F) -> Result<T>
    where
        F: FnOnce(&Database) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.write(context, move |database| job(database)).await
    }

    /// What every write needs besides the connection, for a job
    /// running on the writer thread.
    fn writing<'a>(
        clock: &'a Arc<dyn Clock>,
        epoch: &'a Epoch,
        guard: &'a Arc<CrossNetworkGuard>,
    ) -> Writing<'a> {
        Writing {
            clock: clock.as_ref(),
            epoch,
            guard: guard.as_ref(),
        }
    }
}

impl Drop for SqliteStore {
    /// Stop the writer thread and wait for it.
    ///
    /// Waiting is the point. The thread owns the write connection, so
    /// a store dropped without the join returning would leave a
    /// connection open to a file the caller believes it has finished
    /// with -- and, in a test, a database still being written to
    /// inside a directory about to be removed. Dropping the sender is
    /// what ends the thread's loop; the join is what makes the end of
    /// this function the end of the store.
    fn drop(&mut self) {
        drop(self.writes.take());

        if let Some(writer) = self.writer.take() {
            // A writer thread that panicked has already reported the
            // failure to whichever caller was waiting on it. There is
            // nothing a drop can do about it and nothing it should
            // say.
            let _ = writer.join();
        }
    }
}

/// Open the database, for the writer thread.
///
/// A function of its own only so that the thread body stays readable.
fn open_database(path: &Path) -> Result<Database> {
    Database::open(path)
}

#[async_trait]
impl BindingStore for SqliteStore {
    fn epoch(&self) -> &Epoch {
        &self.epoch
    }

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn cross_network_answers(&self) -> u64 {
        // The store's own check, which every view it hands out
        // shares, so this is the count for the store rather than for
        // whichever view happened to notice.
        self.guard.refused()
    }

    async fn read(&self) -> Result<Box<dyn ReadHandle>> {
        let view = SnapshotView::open(
            self.path.clone(),
            self.epoch.clone(),
            // The instant the view reads at, taken here beside the
            // position it reads at. A lease's expiry is a fact about
            // the clock rather than about the store's order, so a
            // view needs both to answer consistently -- see
            // `Lease::expired` for why it is fixed rather than read
            // again per question.
            self.clock.unix_seconds(),
            Arc::clone(&self.census),
            Arc::clone(&self.guard),
        )
        .await?;

        Ok(Box::new(view))
    }

    async fn interface_owner(&self, namespace: &str, ifname: &str) -> Result<Option<Attachment>> {
        let namespace = namespace.to_owned();
        let ifname = ifname.to_owned();
        let epoch = self.epoch.clone();

        self.read_now("ask which network holds an interface", move |database| {
            ops::interface_owner(database.connection(), &epoch, &namespace, &ifname)
        })
        .await
    }

    async fn create_network(&self, network: &str) -> Result<Cursor> {
        let network = network.to_owned();

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("create a network", move |database| {
            ops::create_network(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
            )
        })
        .await
    }

    async fn delete_network(&self, network: &str, expected: &Cursor) -> Result<Cursor> {
        let network = network.to_owned();
        let expected = expected.clone();

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("remove a network", move |database| {
            ops::delete_network(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
                &expected,
            )
        })
        .await
    }

    async fn attach_interface(
        &self,
        network: &str,
        namespace: &str,
        ifname: &str,
    ) -> Result<Cursor> {
        let network = network.to_owned();
        let namespace = namespace.to_owned();
        let ifname = ifname.to_owned();

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("claim an interface", move |database| {
            ops::attach_interface(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
                &namespace,
                &ifname,
            )
        })
        .await
    }

    async fn detach_interface(
        &self,
        network: &str,
        namespace: &str,
        ifname: &str,
        expected: &Cursor,
    ) -> Result<Cursor> {
        let network = network.to_owned();
        let namespace = namespace.to_owned();
        let ifname = ifname.to_owned();
        let expected = expected.clone();

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("release an interface", move |database| {
            ops::detach_interface(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
                &namespace,
                &ifname,
                &expected,
            )
        })
        .await
    }

    async fn create_address_range(
        &self,
        network: &str,
        name: &str,
        first: std::net::Ipv4Addr,
        last: std::net::Ipv4Addr,
    ) -> Result<Cursor> {
        let network = network.to_owned();
        let name = name.to_owned();

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("create an address range", move |database| {
            ops::create_address_range(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
                &name,
                first,
                last,
            )
        })
        .await
    }

    async fn update_address_range(
        &self,
        network: &str,
        name: &str,
        expected: &Cursor,
        first: std::net::Ipv4Addr,
        last: std::net::Ipv4Addr,
    ) -> Result<Cursor> {
        let network = network.to_owned();
        let name = name.to_owned();
        let expected = expected.clone();

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("resize an address range", move |database| {
            ops::update_address_range(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
                &name,
                &expected,
                first,
                last,
            )
        })
        .await
    }

    async fn delete_address_range(
        &self,
        network: &str,
        name: &str,
        expected: &Cursor,
    ) -> Result<Cursor> {
        let network = network.to_owned();
        let name = name.to_owned();
        let expected = expected.clone();

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("remove an address range", move |database| {
            ops::delete_address_range(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
                &name,
                &expected,
            )
        })
        .await
    }

    async fn create_binding(
        &self,
        network: &str,
        selector: &Selector,
        address: std::net::Ipv4Addr,
        hostname: Option<&str>,
    ) -> Result<Cursor> {
        let network = network.to_owned();
        let selector = selector.clone();
        let hostname = hostname.map(str::to_owned);

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("configure a binding", move |database| {
            ops::create_binding(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
                &selector,
                address,
                hostname.as_deref(),
            )
        })
        .await
    }

    async fn update_binding(
        &self,
        network: &str,
        selector: &Selector,
        expected: &Cursor,
        address: std::net::Ipv4Addr,
        hostname: Option<&str>,
    ) -> Result<Cursor> {
        let network = network.to_owned();
        let selector = selector.clone();
        let expected = expected.clone();
        let hostname = hostname.map(str::to_owned);

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("change a binding", move |database| {
            ops::update_binding(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
                &selector,
                &expected,
                address,
                hostname.as_deref(),
            )
        })
        .await
    }

    async fn delete_binding(
        &self,
        network: &str,
        selector: &Selector,
        expected: &Cursor,
    ) -> Result<Cursor> {
        let network = network.to_owned();
        let selector = selector.clone();
        let expected = expected.clone();

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("remove a binding", move |database| {
            ops::delete_binding(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
                &selector,
                &expected,
            )
        })
        .await
    }

    async fn create_binding_revoking_lease(
        &self,
        network: &str,
        selector: &Selector,
        address: std::net::Ipv4Addr,
        hostname: Option<&str>,
        holder: &Cursor,
    ) -> Result<Cursor> {
        let network = network.to_owned();
        let selector = selector.clone();
        let hostname = hostname.map(str::to_owned);
        let holder = holder.clone();

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("configure a binding over a lease", move |database| {
            ops::create_binding_revoking_lease(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
                &selector,
                address,
                hostname.as_deref(),
                &holder,
            )
        })
        .await
    }

    async fn acquire_lease(
        &self,
        network: &str,
        identity: &ClientIdentity,
        address: std::net::Ipv4Addr,
        duration: i64,
    ) -> Result<(Cursor, Lease)> {
        let network = network.to_owned();
        let identity = identity.clone();

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("take a lease on an address", move |database| {
            lease::acquire_lease(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
                &identity,
                address,
                duration,
            )
        })
        .await
    }

    async fn allocate_lease(
        &self,
        network: &str,
        identity: &ClientIdentity,
        requested: Option<std::net::Ipv4Addr>,
        duration: i64,
    ) -> Result<(Cursor, Lease)> {
        let network = network.to_owned();
        let identity = identity.clone();

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("allocate an address", move |database| {
            lease::allocate_lease(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
                &identity,
                requested,
                duration,
            )
        })
        .await
    }

    async fn renew_lease(
        &self,
        network: &str,
        address: std::net::Ipv4Addr,
        identity: &ClientIdentity,
        duration: i64,
    ) -> Result<Cursor> {
        let network = network.to_owned();
        let identity = identity.clone();

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("renew a lease", move |database| {
            lease::renew_lease(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
                address,
                &identity,
                duration,
            )
        })
        .await
    }

    async fn revoke_lease(
        &self,
        network: &str,
        address: std::net::Ipv4Addr,
        expected: &Cursor,
    ) -> Result<Cursor> {
        let network = network.to_owned();
        let expected = expected.clone();

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("revoke a lease", move |database| {
            lease::revoke_lease(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
                address,
                &expected,
            )
        })
        .await
    }

    async fn sweep_expired_leases(&self, network: &str) -> Result<(Option<Cursor>, Vec<Lease>)> {
        let network = network.to_owned();

        let clock = Arc::clone(&self.clock);
        let epoch = self.epoch.clone();
        let guard = Arc::clone(&self.guard);

        self.write("sweep expired leases", move |database| {
            lease::sweep_expired_leases(
                database.connection_mut(),
                &SqliteStore::writing(&clock, &epoch, &guard),
                &network,
            )
        })
        .await
    }
}

/// The scope for one network-named read, assembled on the thread
/// that is about to run it.
///
/// A function rather than a method, because the three values it
/// gathers arrive as owned clones moved into a job closure: the
/// closure has to own what it reads from, and this is what turns
/// those owned values back into the borrowed scope the statements
/// take.
fn reading<'a>(
    epoch: &'a Epoch,
    network: &'a str,
    guard: &'a Arc<CrossNetworkGuard>,
) -> Reading<'a> {
    Reading {
        epoch,
        network,
        guard: guard.as_ref(),
    }
}

/// A view of the store at one position in its order.
///
/// The transaction this view reads through is held on a thread of its
/// own, which is the only place it can be held: it borrows a
/// connection, which cannot be shared between threads, and it has to
/// survive the caller's awaits. Every method here is a round trip to
/// that thread.
#[derive(Debug)]
pub struct SnapshotView {
    /// Where queries are sent. `None` only while dropping.
    queries: Option<mpsc::UnboundedSender<ReadJob>>,
    /// The thread holding the read transaction, joined on drop.
    thread: Option<JoinHandle<()>>,
    /// The position every answer from this view is consistent with.
    cursor: Cursor,
    /// The instant every answer from this view evaluates an expiry
    /// at, taken when the view opened. See `Lease::expired`.
    as_of: i64,
    /// The store's cross-network check, shared rather than this
    /// view's own: the count is a property of the store, and a view
    /// is a short-lived thing nobody would read a counter off.
    guard: Arc<CrossNetworkGuard>,
}

impl SnapshotView {
    /// Start a view of the database at `path`.
    ///
    /// The thread opens its own connection, begins a read
    /// transaction, and reads the store's position inside it -- in
    /// that order, and the order is what makes the view a view. A
    /// deferred transaction takes its snapshot at its first read, so
    /// reading the position is both the answer this returns and the
    /// act that fixes everything the view will go on to say.
    async fn open(
        path: PathBuf,
        epoch: Epoch,
        as_of: i64,
        census: Arc<ThreadCensus>,
        guard: Arc<CrossNetworkGuard>,
    ) -> Result<Self> {
        let (queries, mut jobs) = mpsc::unbounded_channel::<ReadJob>();
        let (ready, opened) = oneshot::channel::<Result<u64>>();

        let thread = thread::Builder::new()
            .name("teneral-store-view".to_owned())
            .spawn(move || {
                let _enlisted = Enlisted::new(census, true);

                let mut connection = match db::open_read_connection(&path) {
                    Ok(connection) => connection,
                    Err(refusal) => {
                        let _ = ready.send(Err(refusal));
                        return;
                    }
                };

                let transaction = match connection.transaction() {
                    Ok(transaction) => transaction,
                    Err(error) => {
                        let _ =
                            ready.send(Err(Error::storage("opening a view of the store", error)));
                        return;
                    }
                };

                let position = ops::snapshot_position(&transaction);
                let reported = position.is_ok();
                if ready.send(position).is_err() || !reported {
                    return;
                }

                while let Some(job) = jobs.blocking_recv() {
                    job(&transaction);
                }

                // The transaction ends here, when it goes out of
                // scope, which is what releases the view's snapshot.
                // Nothing else does: a view whose thread outlived it
                // would hold a read transaction open for the life of
                // the process.
            })
            .map_err(|error| Error::threads("hold a view of the store", error))?;

        match opened.await {
            Ok(Ok(position)) => Ok(Self {
                queries: Some(queries),
                thread: Some(thread),
                cursor: Cursor::new(epoch, position),
                as_of,
                guard,
            }),
            Ok(Err(refusal)) => {
                let _ = thread.join();
                Err(refusal)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::unavailable("open a view of the store"))
            }
        }
    }

    /// Run `job` inside this view's transaction and await its answer.
    async fn query<T, F>(&self, context: &'static str, job: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let (reply, answer) = oneshot::channel();

        let queries = self
            .queries
            .as_ref()
            .ok_or_else(|| Error::unavailable(context))?;

        queries
            .send(Box::new(move |connection| {
                let _ = reply.send(job(connection));
            }))
            .map_err(|_| Error::unavailable(context))?;

        answer.await.map_err(|_| Error::unavailable(context))?
    }

    /// The epoch this view's answers are positioned in.
    fn epoch(&self) -> Epoch {
        self.cursor.epoch().clone()
    }

    /// The store's cross-network check, for a job about to run on
    /// this view's thread.
    fn guard(&self) -> Arc<CrossNetworkGuard> {
        Arc::clone(&self.guard)
    }
}

impl Drop for SnapshotView {
    /// End the view's transaction and wait for its thread.
    ///
    /// Both halves, for the same reason as the store's own drop: the
    /// transaction is on that thread's stack, so the only thing that
    /// can end it is the thread returning, and the only way to know
    /// it has is to join. A view that was dropped without this would
    /// leave a read transaction open and a thread running, neither of
    /// which anything would notice until something much later failed
    /// for an unrelated-looking reason.
    fn drop(&mut self) {
        drop(self.queries.take());

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[async_trait]
impl ReadHandle for SnapshotView {
    fn cursor(&self) -> &Cursor {
        &self.cursor
    }

    async fn networks(&self) -> Result<Vec<Network>> {
        let epoch = self.epoch();

        self.query("list the networks", move |connection| {
            ops::networks(connection, &epoch)
        })
        .await
    }

    async fn network(&self, network: &str) -> Result<Option<Network>> {
        let epoch = self.epoch();
        let network = network.to_owned();

        self.query("read a network", move |connection| {
            ops::network(connection, &epoch, &network)
        })
        .await
    }

    async fn attachments(&self, network: &str) -> Result<Vec<Attachment>> {
        let epoch = self.epoch();
        let guard = self.guard();
        let network = network.to_owned();

        self.query("list a network's attachments", move |connection| {
            ops::attachments(connection, &reading(&epoch, &network, &guard))
        })
        .await
    }

    async fn attachment(
        &self,
        network: &str,
        namespace: &str,
        ifname: &str,
    ) -> Result<Option<Attachment>> {
        let epoch = self.epoch();
        let guard = self.guard();
        let network = network.to_owned();
        let namespace = namespace.to_owned();
        let ifname = ifname.to_owned();

        self.query("read an attachment", move |connection| {
            ops::attachment(
                connection,
                &reading(&epoch, &network, &guard),
                &namespace,
                &ifname,
            )
        })
        .await
    }

    async fn address_ranges(&self, network: &str) -> Result<Vec<AddressRange>> {
        let epoch = self.epoch();
        let guard = self.guard();
        let network = network.to_owned();

        self.query("list a network's address ranges", move |connection| {
            ops::address_ranges(connection, &reading(&epoch, &network, &guard))
        })
        .await
    }

    async fn address_range(&self, network: &str, name: &str) -> Result<Option<AddressRange>> {
        let epoch = self.epoch();
        let guard = self.guard();
        let network = network.to_owned();
        let name = name.to_owned();

        self.query("read an address range", move |connection| {
            ops::address_range(connection, &reading(&epoch, &network, &guard), &name)
        })
        .await
    }

    async fn bindings(&self, network: &str) -> Result<Vec<Binding>> {
        let epoch = self.epoch();
        let guard = self.guard();
        let network = network.to_owned();

        self.query("list a network's bindings", move |connection| {
            ops::bindings(connection, &reading(&epoch, &network, &guard))
        })
        .await
    }

    async fn binding(&self, network: &str, selector: &Selector) -> Result<Option<Binding>> {
        let epoch = self.epoch();
        let guard = self.guard();
        let network = network.to_owned();
        let selector = selector.clone();

        self.query("read a binding", move |connection| {
            ops::binding(connection, &reading(&epoch, &network, &guard), &selector)
        })
        .await
    }

    async fn lookup_binding(
        &self,
        network: &str,
        identity: &ClientIdentity,
    ) -> Result<Option<Binding>> {
        let epoch = self.epoch();
        let guard = self.guard();
        let network = network.to_owned();
        let identity = identity.clone();

        self.query("look a client up in a network", move |connection| {
            ops::lookup_binding(connection, &reading(&epoch, &network, &guard), &identity)
        })
        .await
    }

    async fn lookup_binding_on_interface(
        &self,
        network: &str,
        namespace: &str,
        ifname: &str,
        identity: &ClientIdentity,
    ) -> Result<Option<Binding>> {
        let epoch = self.epoch();
        let guard = self.guard();
        let network = network.to_owned();
        let namespace = namespace.to_owned();
        let ifname = ifname.to_owned();
        let identity = identity.clone();

        self.query("look a client up on an interface", move |connection| {
            ops::lookup_binding_on_interface(
                connection,
                &reading(&epoch, &network, &guard),
                &namespace,
                &ifname,
                &identity,
            )
        })
        .await
    }

    async fn leases(&self, network: &str) -> Result<Vec<Lease>> {
        let epoch = self.epoch();
        let guard = self.guard();
        let network = network.to_owned();
        let as_of = self.as_of;

        self.query("list a network's leases", move |connection| {
            lease::leases(connection, &reading(&epoch, &network, &guard), as_of)
        })
        .await
    }

    async fn lease(&self, network: &str, address: std::net::Ipv4Addr) -> Result<Option<Lease>> {
        let epoch = self.epoch();
        let guard = self.guard();
        let network = network.to_owned();
        let as_of = self.as_of;

        self.query("read a lease", move |connection| {
            lease::lease(
                connection,
                &reading(&epoch, &network, &guard),
                as_of,
                address,
            )
        })
        .await
    }

    async fn leases_held_by(&self, network: &str, identity: &ClientIdentity) -> Result<Vec<Lease>> {
        let epoch = self.epoch();
        let guard = self.guard();
        let network = network.to_owned();
        let identity = identity.clone();
        let as_of = self.as_of;

        self.query("read what a client holds", move |connection| {
            lease::leases_held_by(
                connection,
                &reading(&epoch, &network, &guard),
                as_of,
                &identity,
            )
        })
        .await
    }

    async fn explain(
        &self,
        namespace: &str,
        ifname: &str,
        identity: &ClientIdentity,
    ) -> Result<Explanation> {
        let epoch = self.epoch();
        let guard = self.guard();
        let namespace = namespace.to_owned();
        let ifname = ifname.to_owned();
        let identity = identity.clone();
        // The view's own position, so that an explanation carries the
        // cursor it was computed at and can be ordered against a
        // watch stream like any other answer.
        let at = self.cursor.clone();

        self.query(
            "explain what a client would be answered",
            move |connection| {
                ops::explain(
                    connection,
                    &epoch,
                    guard.as_ref(),
                    &at,
                    &namespace,
                    &ifname,
                    &identity,
                )
            },
        )
        .await
    }
}
