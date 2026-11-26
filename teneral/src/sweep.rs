//! Turning lapsed leases into events, on a timer.
//!
//! The store evaluates expiry against its clock on every read and
//! refuses an address a live lease holds, so nothing here is load
//! bearing for correctness: a sweeper that never ran would leave every
//! answer the store gives correct and only its history quiet. What a
//! sweep does is turn an expiry into an event, so that a consumer
//! watching the history is *told* an address was given up rather than
//! left to compare an instant against its own clock.
//!
//! # Why the interval is a day
//!
//! Removing the row is also what stops a returning client getting its
//! old address back: the store picks the lowest free address in a pool
//! and reads a lapsed lease as its holder's claim, so that row is the
//! only source of address stability this daemon has. The two cannot be
//! set apart -- one interval decides how timely the event is and how
//! long the stickiness lasts -- and of the two, the stickiness is the
//! one worth keeping, because the event is a convenience for a
//! consumer that could have worked it out and the row cannot be
//! reconstructed once it is gone. A day, so that the sentence an
//! operator reads is "a client that was away for a day gets its
//! address back".
//!
//! Nothing is swept at startup, deliberately. A daemon that swept as
//! it came up would make its own restarts the bound on that
//! stickiness, and a restart is the event most likely to coincide with
//! the outage a client is returning from.
//!
//! There is no grace period, which is what decoupling the two would
//! need: the event when the lease lapses and the row removed later is
//! two writes and a lease state the schema does not have.
//!
//! # Stopping
//!
//! The timer is a task holding a handle on the store, so it is told to
//! stop and waited for rather than cancelled with the runtime. A store
//! dropped while the task was still writing would be a daemon blocked
//! on a thread something was still using.

use std::sync::Arc;
use std::time::Duration;

use teneral_store::{Actor, BindingStore, Cursor, Lease};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior, interval_at};

use crate::serve::log;

type Failure = Box<dyn std::error::Error>;

/// Who a sweep's writes are recorded against. The daemon on its own
/// behalf: there is no request behind a sweep to attribute it to.
const SWEEPER: Actor = Actor::Daemon("sweeper");

/// The timer, and how to stop it.
pub struct Sweeper {
    /// Told once, read by the task.
    stop: watch::Sender<bool>,
    /// The task itself.
    task: JoinHandle<()>,
}

impl Sweeper {
    /// Stop sweeping, and wait for a sweep in progress to finish.
    ///
    /// Returns when the task has returned, which is when its handle on
    /// the store has been given back.
    pub async fn stop(self) {
        // A send with no receiver left means the task has already
        // returned, which is the outcome being asked for.
        let _ = self.stop.send(true);

        // A task that panicked has already said so on its own account;
        // there is nothing further to report and nothing to do about
        // it while stopping.
        let _ = self.task.await;
    }
}

/// Start the timer. Nothing is swept until one interval has passed.
///
/// The interval is given rather than read from the flags, because it
/// has to be one the monotonic clock can name an instant an interval
/// ahead of -- which is a refusal the daemon makes before it creates
/// anything, so that it names the flag rather than panicking in here.
pub fn start(store: Arc<dyn BindingStore>, every: Duration) -> Sweeper {
    let (stop, listening) = watch::channel(false);

    Sweeper {
        stop,
        task: tokio::spawn(sweep_every(store, every, listening)),
    }
}

/// Sweep once an interval until the daemon stops.
async fn sweep_every(
    store: Arc<dyn BindingStore>,
    every: Duration,
    mut stop: watch::Receiver<bool>,
) {
    // The first tick an interval away rather than now, which is what
    // "no sweep at startup" means in code.
    let mut ticks = interval_at(Instant::now() + every, every);
    // A sweep that ran late has nothing to catch up on: whatever a
    // missed tick would have removed is still there for the next one,
    // so the ticks that were missed are dropped rather than run back
    // to back.
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = ticks.tick() => {}
            _ = stop.changed() => return,
        }

        match sweep(store.as_ref()).await {
            Ok(swept) => {
                for line in swept.iter().filter_map(Swept::line) {
                    log(&line);
                }
            }
            // The networks could not be listed, so this sweep did
            // nothing at all. The next tick tries again: whatever has
            // lapsed is still lapsed, and a daemon that stopped
            // sweeping because one read failed would need a restart to
            // start again.
            Err(error) => log(&format!("sweeping lapsed leases: {error}")),
        }
    }
}

/// What one sweep did in one network.
#[derive(Debug)]
struct Swept {
    /// The network swept.
    network: String,
    /// Where the expiries it wrote sit in the store's order, or `None`
    /// when it removed nothing: a sweep that wrote nothing has no
    /// position, and reporting one would invent it.
    at: Option<Cursor>,
    /// The leases it removed.
    leases: Vec<Lease>,
}

impl Swept {
    /// The line this sweep is worth logging, or `None` when it is not.
    ///
    /// A sweep that removed nothing says nothing at all. Most sweeps
    /// in most deployments remove nothing, and a daily line saying so
    /// would be a line an operator learns to skip past -- which is the
    /// state in which the one that matters is skipped too.
    fn line(&self) -> Option<String> {
        if self.leases.is_empty() {
            return None;
        }

        let count = self.leases.len();
        let plural = if count == 1 { "" } else { "s" };
        let addresses: Vec<String> = self
            .leases
            .iter()
            .map(|lease| lease.address.to_string())
            .collect();
        let addresses = addresses.join(", ");

        // The position is reported so that an operator reading this
        // line can find the same expiries in the history a consumer
        // was served. It is always there when something was removed;
        // the other arm is the store's contract written out rather
        // than assumed, because asserting it here would be a panic in
        // a daemon for the sake of a log line.
        Some(match &self.at {
            Some(at) => format!(
                "swept {count} lapsed lease{plural} from the network {}, at position {at}: \
                 {addresses}",
                self.network
            ),
            None => format!(
                "swept {count} lapsed lease{plural} from the network {}: {addresses}",
                self.network
            ),
        })
    }
}

/// Sweep every network the store holds, once.
///
/// The timer calls this and so does a test, which is the reason it is
/// separate from the timer at all: what a sweep does can then be
/// checked without waiting for one.
async fn sweep(store: &dyn BindingStore) -> Result<Vec<Swept>, Failure> {
    // The listing through a view, dropped before anything is written:
    // a view costs the store a thread while it is held, and the writes
    // below need none. A network created while this sweep runs is
    // swept by the next one, which is a day at most and nothing waits
    // for it.
    let view = store
        .read()
        .await
        .map_err(|error| format!("reading the store: {error}"))?;
    let networks = view
        .networks()
        .await
        .map_err(|error| format!("listing the networks to sweep: {error}"))?;
    drop(view);

    let mut swept = Vec::with_capacity(networks.len());

    for network in networks {
        match store.sweep_expired_leases(&SWEEPER, &network.name).await {
            Ok((at, leases)) => swept.push(Swept {
                network: network.name,
                at,
                leases,
            }),
            // One network's failure is not the others'. A network
            // deleted between the listing above and this write is
            // exactly this case, and every network after it still has
            // leases to sweep.
            Err(error) => log(&format!("sweeping the network {}: {error}", network.name)),
        }
    }

    Ok(swept)
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};

    use teneral_store::{
        Actor, BindingStore, ClientIdentity, Clock, Cursor, History, SelectorKind, SqliteStore,
    };

    use super::{SWEEPER, Swept, sweep};

    /// Distinguishes the directories of tests running at the same time
    /// in the same process.
    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// The first network.
    const ALPHA: &str = "alpha";
    /// The second, holding the same addresses.
    const BETA: &str = "beta";
    /// The address both networks lease out, which is what makes a
    /// sweep of one network's lease visible as a sweep of the wrong
    /// one.
    const SHARED: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 10);
    /// How long the leases in these tests run for.
    const MINUTE: i64 = 60;

    /// A directory holding one test's store, removed when the test
    /// ends.
    ///
    /// Rolled by hand, as the store's own suite, the control plane's
    /// and the DHCPv4 crate's all do: there is no temporary-directory
    /// dependency in this workspace, and a dependency whose only
    /// consumer is four lines of `create_dir_all` would not survive a
    /// reading of the dependency list.
    struct Directory {
        /// Where it is.
        path: PathBuf,
    }

    impl Directory {
        /// A directory named after `label`, so that a leftover one
        /// says which test left it.
        fn new(label: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "teneral-sweep-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));

            std::fs::create_dir_all(&path).expect("a temporary directory for a test");

            Self { path }
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            // The directory rather than the file: the write-ahead log
            // leaves a `-wal` and a `-shm` beside the database. A
            // failure to clean up is not the test's verdict and is not
            // reported.
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// A clock a test moves by hand.
    ///
    /// A lease expires at an absolute instant, so the only honest way
    /// to test what happens after one has passed is to move the clock.
    /// A test that slept instead would be one somebody later makes
    /// faster, and what they would shorten is the margin it was
    /// measuring.
    #[derive(Debug)]
    struct TestClock {
        /// Now, in seconds since the Unix epoch.
        now: AtomicI64,
    }

    impl TestClock {
        /// A clock stopped at a round instant well inside the range a
        /// lease time can be added to.
        fn new() -> Self {
            TestClock {
                now: AtomicI64::new(1_700_000_000),
            }
        }

        /// Move it forward.
        fn advance(&self, seconds: i64) {
            self.now.fetch_add(seconds, Ordering::Relaxed);
        }
    }

    impl Clock for TestClock {
        fn unix_seconds(&self) -> i64 {
            self.now.load(Ordering::Relaxed)
        }
    }

    /// Two networks leasing the same addresses, one store, a clock the
    /// test moves.
    struct Lab {
        /// Kept so that the directory outlives the store inside it.
        _directory: Directory,
        /// The store the sweep is run against.
        store: Arc<SqliteStore>,
        /// The clock it reads.
        clock: Arc<TestClock>,
    }

    impl Lab {
        async fn new(label: &str) -> Self {
            let directory = Directory::new(label);
            let clock = Arc::new(TestClock::new());
            let store = Arc::new(
                SqliteStore::with_clock(
                    directory.path.join("store.sqlite"),
                    Arc::clone(&clock) as _,
                )
                .await
                .expect("a store in the test's own directory"),
            );

            for network in [ALPHA, BETA] {
                store
                    .create_network(&operator(), network)
                    .await
                    .expect("a network");
                store
                    .create_address_range(
                        &operator(),
                        network,
                        "pool",
                        SHARED,
                        Ipv4Addr::new(10, 0, 0, 20),
                    )
                    .await
                    .expect("the same span in both networks");
            }

            Lab {
                _directory: directory,
                store,
                clock,
            }
        }

        /// Record a client as holding `address` in `network` for
        /// `duration` seconds.
        async fn lease(&self, network: &str, address: Ipv4Addr, mac: &str, duration: i64) {
            let identity = ClientIdentity::new()
                .with(SelectorKind::Mac, mac)
                .expect("a hardware address");

            self.store
                .acquire_lease(&operator(), network, &identity, address, duration)
                .await
                .expect("a lease");
        }

        /// The addresses `network` still has leases for.
        async fn leased(&self, network: &str) -> Vec<Ipv4Addr> {
            let view = self.store.read().await.expect("a view");
            let leases = view.leases(network).await.expect("the network's leases");

            leases.into_iter().map(|lease| lease.address).collect()
        }

        /// The position the store has reached, which is what a sweep
        /// that wrote nothing must leave alone.
        fn position(&self) -> Cursor {
            self.store.subscribe().borrow().clone()
        }

        /// Everything `network`'s history holds after `after`.
        async fn history_after(&self, network: &str, after: &Cursor) -> Vec<teneral_store::Event> {
            let view = self.store.read().await.expect("a view");
            let history = view.history(network, after).await.expect("the history");

            match history {
                History::Resumed { events, .. } => events,
                other => panic!("the position the test took should still resume: {other:?}"),
            }
        }

        /// Sweep every network, as the timer does.
        async fn sweep(&self) -> Vec<Swept> {
            sweep(self.store.as_ref() as &dyn BindingStore)
                .await
                .expect("a sweep of every network")
        }
    }

    /// Whoever configured the store and recorded the leases, which is
    /// not the sweeper: a test that wrote its own setup as the daemon
    /// could not tell the sweep's writes from its own.
    fn operator() -> Actor {
        Actor::Peer(1000)
    }

    /// One network's result out of a whole sweep.
    fn in_network<'a>(swept: &'a [Swept], network: &str) -> &'a Swept {
        swept
            .iter()
            .find(|one| one.network == network)
            .expect("every network the store holds is swept")
    }

    #[tokio::test]
    async fn a_lapsed_lease_is_removed_and_its_expiry_reaches_the_history() {
        let lab = Lab::new("lapsed").await;
        lab.lease(ALPHA, SHARED, "02:00:00:00:00:01", MINUTE).await;

        let before = lab.position();
        lab.clock.advance(MINUTE + 1);
        let swept = lab.sweep().await;

        let alpha = in_network(&swept, ALPHA);
        assert_eq!(alpha.leases.len(), 1);
        assert_eq!(alpha.leases[0].address, SHARED);
        assert!(alpha.at.is_some(), "a sweep that removed a lease wrote one");
        assert!(lab.leased(ALPHA).await.is_empty());

        // The point of the whole operation: an event saying the
        // address was given up, attributed to the sweeper, which a
        // consumer watching the history would otherwise have had to
        // infer by comparing an instant against its own clock.
        let events = lab.history_after(ALPHA, &before).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].resource_id, SHARED.to_string());
        assert!(
            events[0].state.is_none(),
            "the post-state of a removed lease is nothing"
        );
        assert_eq!(events[0].actor, SWEEPER.as_recorded());
        assert_eq!(alpha.at.as_ref(), Some(&events[0].cursor));

        // And the line an operator reads, which names the network and
        // the address rather than a count on its own.
        let line = alpha.line().expect("a sweep that removed something logs");
        assert!(line.contains(ALPHA), "{line}");
        assert!(line.contains(&SHARED.to_string()), "{line}");
    }

    #[tokio::test]
    async fn a_lease_that_has_not_lapsed_is_left_alone() {
        let lab = Lab::new("live").await;
        lab.lease(ALPHA, SHARED, "02:00:00:00:00:01", MINUTE).await;

        let before = lab.position();
        // Up to the instant before the lease's own, which is the
        // boundary: a lease is held while the clock is before its
        // instant.
        lab.clock.advance(MINUTE - 1);
        let swept = lab.sweep().await;

        assert!(in_network(&swept, ALPHA).leases.is_empty());
        assert_eq!(lab.leased(ALPHA).await, vec![SHARED]);
        assert_eq!(lab.position(), before, "a live lease is not an event");
    }

    #[tokio::test]
    async fn one_network_is_swept_while_the_same_address_in_another_is_left_alone() {
        let lab = Lab::new("two-networks").await;
        // The same address, the same client, two networks -- and only
        // alpha's lease has lapsed when the sweep runs. A sweeper
        // keyed by address rather than by network would take both.
        lab.lease(ALPHA, SHARED, "02:00:00:00:00:01", MINUTE).await;
        lab.lease(BETA, SHARED, "02:00:00:00:00:01", MINUTE * 10)
            .await;

        let before = lab.position();
        lab.clock.advance(MINUTE + 1);
        let swept = lab.sweep().await;

        let alpha = in_network(&swept, ALPHA);
        assert_eq!(alpha.leases.len(), 1);
        assert_eq!(alpha.leases[0].address, SHARED);
        assert_eq!(alpha.leases[0].network, ALPHA);

        let beta = in_network(&swept, BETA);
        assert!(beta.leases.is_empty());
        assert!(beta.at.is_none());
        assert!(beta.line().is_none(), "beta gave nothing up");

        assert!(lab.leased(ALPHA).await.is_empty());
        assert_eq!(lab.leased(BETA).await, vec![SHARED]);

        // And the history says so in the network it happened in, with
        // nothing written in the other.
        assert_eq!(lab.history_after(ALPHA, &before).await.len(), 1);
        assert!(lab.history_after(BETA, &before).await.is_empty());
    }

    #[tokio::test]
    async fn a_sweep_that_finds_nothing_writes_no_event_and_reports_no_position() {
        let lab = Lab::new("nothing").await;
        lab.lease(ALPHA, SHARED, "02:00:00:00:00:01", MINUTE).await;

        let before = lab.position();
        let swept = lab.sweep().await;

        assert_eq!(swept.len(), 2, "both networks were swept");
        for one in &swept {
            assert!(one.leases.is_empty());
            assert!(one.at.is_none(), "there was no position to report");
            assert!(one.line().is_none(), "and so nothing to log");
        }

        assert_eq!(
            lab.position(),
            before,
            "a sweep that removed nothing wrote nothing"
        );
        assert!(lab.history_after(ALPHA, &before).await.is_empty());
    }
}
