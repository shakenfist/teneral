//! What the decision layer's tests are built from.
//!
//! A real store, in a directory of its own, removed when the test
//! finishes -- rolled by hand, as the store's own suite and the control
//! plane's both do, because a dependency whose only consumer is a
//! temporary directory would not survive a reading of the dependency
//! list.
//!
//! Nothing here fakes the store, and that is not thrift. What these
//! tests check is that one network's answers stay out of another's,
//! and a fake store would be a second implementation of exactly the
//! rules that keep them apart -- so a test against one could only ever
//! prove that this layer agrees with the fake.
//!
//! # Two networks holding the same address, every time
//!
//! [`Lab`] builds the arrangement the whole project exists for: two
//! networks, each with an interface of its own, each allocating from
//! the *same* span of addresses, and each interface holding the same
//! address and prefix. The only thing that tells them apart is which
//! interface a request arrived on. A test that passes against one
//! network proves nothing here, so the fixture does not offer one.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use teneral_store::{Actor, BindingStore, Clock, Selector, SelectorKind, SqliteStore};

use crate::decide::egress::{Egress, Reply};
use crate::decide::{Arrival, Decider};
use crate::wire::ServerFacts;

/// Distinguishes the directories of tests running at the same time in
/// the same process.
static NEXT: AtomicU32 = AtomicU32::new(0);

/// The namespace an interface in no namespace of its own is attached
/// under, which is the datapath's to spell and this fixture's to
/// agree with: a test that wrote the word itself would pass while the
/// daemon looked an attachment up under another one.
pub(crate) use crate::datapath::UNNAMESPACED;

/// The first network, and the interface it claimed.
pub(crate) const ALPHA: &str = "alpha";
/// Alpha's interface.
pub(crate) const ALPHA_IF: &str = "br-alpha";
/// The second network.
pub(crate) const BETA: &str = "beta";
/// Beta's interface.
pub(crate) const BETA_IF: &str = "br-beta";

/// The address both interfaces hold, and therefore the server
/// identifier, the router and the nameserver in both networks.
///
/// Identical on purpose. If the two networks differed in anything an
/// answer is derived from, a test could pass by reading the wrong
/// network's interface.
pub(crate) const SERVER_ADDRESS: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);

/// The prefix configured beside it, in both networks.
pub(crate) const PREFIX_LENGTH: u8 = 24;

/// The span both networks allocate from. The same addresses in both.
pub(crate) const POOL_FIRST: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 10);
/// The top of that span.
pub(crate) const POOL_LAST: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 20);

/// The schema's default lease time, which both networks start with.
pub(crate) const DEFAULT_LEASE: i64 = 43200;

/// A directory holding one test's store, removed when the test ends.
struct Directory {
    /// Where it is.
    path: PathBuf,
}

impl Directory {
    /// A directory named after `label`, so that a leftover one says
    /// which test left it.
    fn new(label: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "teneral-dhcp-decide-{label}-{}-{}",
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
        // leaves a `-wal` and a `-shm` beside the database. A failure
        // to clean up is not the test's verdict and is not reported.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A clock a test moves by hand.
///
/// Leases expire at an absolute instant, so the only honest way to
/// test what happens after one has passed is to move the clock. A test
/// that slept instead would be one somebody later makes faster, and
/// what they would shorten is the margin it was measuring.
#[derive(Debug)]
pub(crate) struct TestClock {
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
    pub(crate) fn advance(&self, seconds: i64) {
        self.now.fetch_add(seconds, Ordering::Relaxed);
    }
}

impl Clock for TestClock {
    fn unix_seconds(&self) -> i64 {
        self.now.load(Ordering::Relaxed)
    }
}

/// One answer, as the egress was given it.
#[derive(Clone, Debug)]
pub(crate) struct Captured {
    /// The interface it was to leave by.
    pub(crate) ifname: String,
    /// The namespace that interface is named within.
    pub(crate) namespace: String,
    /// The client's hardware address.
    pub(crate) hardware_address: Vec<u8>,
    /// The address being offered or acknowledged.
    pub(crate) client_address: Ipv4Addr,
    /// Whether the client asked for a broadcast reply.
    pub(crate) broadcast: bool,
    /// The bytes themselves.
    pub(crate) datagram: Vec<u8>,
}

impl Captured {
    /// `yiaddr`, read out of the bytes rather than taken from the
    /// builder's inputs.
    pub(crate) fn offered_address(&self) -> Ipv4Addr {
        let octets: [u8; 4] = self.datagram[16..20]
            .try_into()
            .expect("a reply carries a yiaddr");

        Ipv4Addr::from(octets)
    }

    /// The value one option carries, if the reply carries it.
    pub(crate) fn option(&self, code: u8) -> Option<Vec<u8>> {
        crate::wire::test_support::option_value(&self.datagram, code)
    }

    /// The option codes the reply carries, in the order its bytes
    /// carry them.
    pub(crate) fn option_codes(&self) -> Vec<u8> {
        crate::wire::test_support::option_codes(&self.datagram)
    }

    /// Option 53, the message type the client will read.
    pub(crate) fn message_type(&self) -> u8 {
        let value = self.option(53).expect("a reply carries a message type");

        value[0]
    }

    /// An option's value read as the four-octet count of seconds the
    /// three time options carry.
    pub(crate) fn seconds(&self, code: u8) -> Option<u32> {
        let value = self.option(code)?;
        let octets: [u8; 4] = value.try_into().ok()?;

        Some(u32::from_be_bytes(octets))
    }
}

/// An egress that keeps what it was given instead of sending it.
///
/// The seam this stands in for is one method, which is the whole
/// reason the decision layer can be tested at all: no socket, no
/// privilege, and no frame to hand-build.
#[derive(Debug, Default)]
pub(crate) struct Recorder {
    /// What was sent, in order.
    sent: Mutex<Vec<Captured>>,
}

impl Recorder {
    /// Everything sent so far, oldest first.
    pub(crate) fn sent(&self) -> Vec<Captured> {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The one answer that was sent, failing the test if there was not
    /// exactly one.
    pub(crate) fn only(&self) -> Captured {
        let sent = self.sent();
        assert_eq!(sent.len(), 1, "exactly one answer should have been sent");

        sent[0].clone()
    }

    /// How many answers have been sent.
    pub(crate) fn count(&self) -> usize {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Forget what was sent, so that a test can make a second request
    /// and read its answer alone.
    pub(crate) fn clear(&self) {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

impl Egress for Recorder {
    fn send(&self, reply: &Reply<'_>) -> std::io::Result<()> {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Captured {
                ifname: reply.ifname().to_owned(),
                namespace: reply.namespace().to_owned(),
                hardware_address: reply.hardware_address().to_vec(),
                client_address: reply.client_address(),
                broadcast: reply.broadcast(),
                datagram: reply.datagram().to_vec(),
            });

        Ok(())
    }
}

/// Two networks holding the same addresses, one process, one store.
pub(crate) struct Lab {
    /// Kept so that the directory outlives the store inside it.
    _directory: Directory,
    /// The store itself, which the decider holds a second handle to.
    pub(crate) store: Arc<SqliteStore>,
    /// The clock the store reads.
    pub(crate) clock: Arc<TestClock>,
    /// The layer under test.
    pub(crate) decider: Decider<Recorder>,
}

impl Lab {
    /// Build the two networks, attach an interface to each, and give
    /// each the same span of addresses to allocate from.
    pub(crate) async fn new(label: &str) -> Self {
        let directory = Directory::new(label);
        let clock = Arc::new(TestClock::new());
        let store = Arc::new(
            SqliteStore::with_clock(directory.path.join("store.sqlite"), Arc::clone(&clock) as _)
                .await
                .expect("a store in the test's own directory"),
        );

        for (network, ifname) in [(ALPHA, ALPHA_IF), (BETA, BETA_IF)] {
            store
                .create_network(&operator(), network)
                .await
                .expect("a network");
            store
                .attach_interface(&operator(), network, UNNAMESPACED, ifname)
                .await
                .expect("an interface attached to it");
            store
                .create_address_range(&operator(), network, "pool", POOL_FIRST, POOL_LAST)
                .await
                .expect("the same span in both networks");
        }

        let decider = Decider::new(
            Arc::clone(&store) as Arc<dyn BindingStore>,
            Recorder::default(),
        );

        Lab {
            _directory: directory,
            store,
            clock,
            decider,
        }
    }

    /// Where a request arrives, for one of the two networks.
    pub(crate) fn arrival(ifname: &str) -> Arrival<'_> {
        Arrival {
            namespace: UNNAMESPACED,
            ifname,
            server: ServerFacts::new(SERVER_ADDRESS, PREFIX_LENGTH).expect("a /24 is a prefix"),
        }
    }

    /// What the egress recorded.
    pub(crate) fn sent(&self) -> &Recorder {
        self.decider.egress()
    }

    /// Configure `network`'s answer for a client identified this way.
    pub(crate) async fn reserve(
        &self,
        network: &str,
        kind: SelectorKind,
        value: &str,
        address: Ipv4Addr,
    ) {
        let selector = Selector::new(kind, value).expect("a selector an operator could type");

        self.store
            .create_binding(&operator(), network, &selector, address, None)
            .await
            .expect("a binding");
    }

    /// Set how long `network` holds the addresses it gives out.
    ///
    /// Reads the network to get the position to compare against,
    /// because the store takes that rather than overwriting whatever
    /// is there.
    pub(crate) async fn set_lease_seconds(&self, network: &str, seconds: i64) {
        let view = self.store.read().await.expect("a view");
        let found = view
            .network(network)
            .await
            .expect("the network")
            .expect("a network the fixture created");
        drop(view);

        self.store
            .update_network(&operator(), network, &found.cursor, seconds)
            .await
            .expect("a lease time");
    }

    /// Move the ends of `network`'s pool, so that the two networks
    /// allocate from different spans.
    pub(crate) async fn set_range(&self, network: &str, first: Ipv4Addr, last: Ipv4Addr) {
        let view = self.store.read().await.expect("a view");
        let found = view
            .address_range(network, "pool")
            .await
            .expect("the range")
            .expect("a range the fixture created");
        drop(view);

        self.store
            .update_address_range(&operator(), network, "pool", &found.cursor, first, last)
            .await
            .expect("a resized pool");
    }

    /// Record some other client as holding `address` in `network`.
    ///
    /// Used to make the two networks answer differently about the same
    /// address, which is the only way a test can tell that the right
    /// network was asked.
    pub(crate) async fn held_by_somebody_else(&self, network: &str, address: Ipv4Addr) {
        let other = teneral_store::ClientIdentity::new()
            .with(SelectorKind::Mac, "aa:aa:aa:aa:aa:aa")
            .expect("a hardware address");

        self.store
            .acquire_lease(&operator(), network, &other, address, DEFAULT_LEASE)
            .await
            .expect("another client's lease");
    }

    /// The addresses `network` has leases for, lowest first.
    pub(crate) async fn leased(&self, network: &str) -> Vec<Ipv4Addr> {
        let view = self.store.read().await.expect("a view");
        let leases = view.leases(network).await.expect("the network's leases");

        leases.into_iter().map(|lease| lease.address).collect()
    }
}

/// The actor a test's own configuration is written by.
///
/// A uid rather than the daemon, because this is the operator's half
/// of the fixture: a test that recorded its own setup as the front-end
/// would make the front-end's writes indistinguishable from them.
pub(crate) fn operator() -> Actor {
    Actor::Peer(1000)
}
