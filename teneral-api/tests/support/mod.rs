//! The harness every test in this crate's suites runs against.
//!
//! A real store in a directory of its own, a real socket inside that
//! directory, and credentials from a real mint -- there is
//! deliberately no way to construct one of those outside the store, so
//! a credential in a test is one the store issued. Nothing here fakes
//! the store: what these suites check is the behaviour of a daemon
//! answering from it, and a fake would be a second implementation of
//! the rules that keep one network's answers out of another's.
//!
//! Two networks hold the same address in every fixture below, because
//! that is this project's central correctness property and one network
//! proves nothing about it.

// A harness shared by every test binary in this crate, each of which
// uses the part of it that its own subject needs. Without this,
// whichever helper a suite does not call is dead code in that suite's
// compilation, and the lint gate -- which compiles every target with
// warnings as errors -- fails for a helper another suite depends on.
#![allow(dead_code)]

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use teneral_api::client::{Connection, Stream};
use teneral_api::proto::{self, request, response, watch_event};
use teneral_api::{Listener, ServerConfig};
use teneral_store::{
    Actor, BindingStore, Caller, Capability, ClientIdentity, Cursor, EventKind, Lease, MintedToken,
    Retention, SelectorKind, SqliteStore, SystemClock, TokenScope, TokenStore,
};
use tokio::task::JoinHandle;

/// One of the two networks.
pub const ALPHA: &str = "alpha";
/// The other, which holds the same addresses.
pub const BETA: &str = "beta";

/// The address both networks configure, for the same client.
pub const BOUND: &str = "10.0.0.5";
/// The address both networks lease, to different clients.
pub const LEASED: &str = "10.0.0.7";
/// An address neither network has anything on.
pub const FREE: &str = "10.0.0.42";

/// The client both networks have a binding for, legitimately meaning
/// two unrelated clients.
pub const CLIENT: &str = "00:11:22:33:44:55";

/// How long a lease the fixtures take, in seconds. Long enough that no
/// test's answers depend on when it ran.
pub const LEASE_SECONDS: i64 = 3600;

/// Distinguishes the directories of tests running at the same time in
/// the same process.
static NEXT: AtomicU32 = AtomicU32::new(0);

/// A directory holding one test's store and socket, removed when the
/// test finishes.
///
/// Rolled by hand rather than taken from a crate, as the store's own
/// suite does: a dependency whose only consumer is a temporary
/// directory would not survive a reading of the dependency list.
pub struct TempDirectory {
    path: PathBuf,
}

impl TempDirectory {
    pub fn new(label: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "teneral-api-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));

        std::fs::create_dir_all(&path).expect("a temporary directory for a test");

        Self { path }
    }

    /// Where this test's socket goes. Inside the directory the test
    /// owns, so that nothing of one test's is reachable from
    /// another's.
    pub fn socket(&self) -> PathBuf {
        self.path.join("teneral.sock")
    }

    pub fn store(&self) -> PathBuf {
        self.path.join("store.sqlite")
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        // The directory rather than the files: the write-ahead log
        // leaves a `-wal` and a `-shm` beside the database. A failure
        // to clean up is not the test's verdict and is not reported.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A listener serving a store, and the handle that stops it.
pub struct Daemon {
    socket: PathBuf,
    store: Arc<SqliteStore>,
    serving: JoinHandle<()>,
    stop: tokio::sync::oneshot::Sender<()>,
}

impl Daemon {
    /// Start a daemon on `directory`'s socket, serving as `owning_uid`.
    ///
    /// The uid is a parameter because the only way to check that a
    /// caller which is not the owning peer is refused is to serve as a
    /// uid the test is not running as. The kernel reports the test's
    /// own uid for every connection it makes, which is the whole point
    /// of reading it from the kernel.
    pub async fn start(directory: &TempDirectory, owning_uid: u32) -> Self {
        Self::started(directory, owning_uid, Retention::DEFAULT, |_| {}).await
    }

    /// A daemon serving as this test's own uid, with `adjust` applied
    /// to the numbers it serves with.
    ///
    /// The numbers are a parameter because two of them cannot be
    /// tested at their defaults: a heartbeat every fifteen seconds is
    /// longer than a test should take, and reaching a bound of
    /// sixty-four views means holding sixty-four threads open at once
    /// to prove a refusal that is the same refusal at two.
    pub async fn tuned(directory: &TempDirectory, adjust: impl FnOnce(&mut ServerConfig)) -> Self {
        Self::started(
            directory,
            teneral_api::owning_uid(),
            Retention::DEFAULT,
            adjust,
        )
        .await
    }

    /// A daemon whose store keeps only as much history as `retention`
    /// says, for the tests about a position that has fallen out of it.
    pub async fn retaining(directory: &TempDirectory, retention: Retention) -> Self {
        Self::started(directory, teneral_api::owning_uid(), retention, |_| {}).await
    }

    /// A daemon whose store keeps only `retention`, serving with the
    /// numbers `adjust` leaves.
    ///
    /// Both at once, for the tests about a consumer that stays inside
    /// a short history by advancing its position from heartbeats.
    /// Neither number can be left at its default there: the default
    /// history outlasts any test, and the default heartbeat is longer
    /// than a test should take.
    pub async fn retaining_tuned(
        directory: &TempDirectory,
        retention: Retention,
        adjust: impl FnOnce(&mut ServerConfig),
    ) -> Self {
        Self::started(directory, teneral_api::owning_uid(), retention, adjust).await
    }

    /// The whole of starting one, which the five above differ from
    /// only in what they pass.
    async fn started(
        directory: &TempDirectory,
        owning_uid: u32,
        retention: Retention,
        adjust: impl FnOnce(&mut ServerConfig),
    ) -> Self {
        let store = Arc::new(
            SqliteStore::with_clock_and_retention(
                directory.store(),
                Arc::new(SystemClock),
                retention,
            )
            .await
            .expect("a store in the test's own directory"),
        );

        let mut config = ServerConfig::new(directory.socket());
        config.owning_uid = owning_uid;
        config.probe_timeout = Duration::from_millis(250);
        adjust(&mut config);

        let listener = Listener::bind(config)
            .await
            .expect("a listener on the test's own socket");
        let socket = listener.path().to_owned();

        let (stop, stopped) = tokio::sync::oneshot::channel();
        let binding: Arc<dyn BindingStore> = Arc::clone(&store) as Arc<dyn BindingStore>;
        let tokens: Arc<dyn TokenStore> = Arc::clone(&store) as Arc<dyn TokenStore>;

        let serving = tokio::spawn(async move {
            let _ = listener
                .serve(binding, tokens, async {
                    let _ = stopped.await;
                })
                .await;
        });

        // The accept loop is in another task, so a connection made the
        // instant this returns would race it. One yield is enough for
        // the spawned task to reach its first await.
        tokio::task::yield_now().await;

        Self {
            socket,
            store,
            serving,
            stop,
        }
    }

    /// A daemon serving as the uid this test runs as, so that its
    /// connections are the owning peer's.
    pub async fn owned(directory: &TempDirectory) -> Self {
        Self::start(directory, teneral_api::owning_uid()).await
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// The store itself, for the writes a test makes as a fixture
    /// rather than as the thing under test.
    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    /// The same store, for a test whose fixture writes from a task of
    /// its own: writes landing while a consumer connects are the only
    /// way to exercise the join between its first read and the stream.
    pub fn shared_store(&self) -> Arc<SqliteStore> {
        Arc::clone(&self.store)
    }

    /// Connect, as the owning peer or as a credential.
    pub async fn connect(&self, secret: Option<&str>) -> Connection {
        Connection::connect(self.socket(), secret)
            .await
            .expect("the daemon serves this caller")
    }

    /// Mint a credential through the store, as the owning peer does.
    pub async fn mint(
        &self,
        name: &str,
        scope: TokenScope,
        capabilities: &[Capability],
    ) -> MintedToken {
        self.store
            .mint(&Caller::OwningPeer(0), name, &scope, capabilities)
            .await
            .expect("the owning peer may mint anything")
    }

    /// Two networks, each with an interface, a range, and a binding for
    /// the same client on the same address.
    ///
    /// Written through the store rather than over the socket, because
    /// these are the fixture and not the thing under test. The two
    /// networks differ in exactly the places a cross-network answer
    /// would show: the far end of the range, and the hostname on the
    /// binding.
    pub async fn two_networks(&self) {
        for network in [ALPHA, BETA] {
            self.store
                .create_network(&actor(), network)
                .await
                .expect("a network");

            self.store
                .attach_interface(&actor(), network, &namespace(network), "eth0")
                .await
                .expect("an interface");

            self.store
                .create_address_range(
                    &actor(),
                    network,
                    "pool",
                    address("10.0.0.1"),
                    address(if network == ALPHA {
                        "10.0.0.100"
                    } else {
                        "10.0.0.200"
                    }),
                )
                .await
                .expect("a range");

            self.store
                .create_binding(
                    &actor(),
                    network,
                    &teneral_store::Selector::new(SelectorKind::Mac, CLIENT).expect("a selector"),
                    address(BOUND),
                    Some(&format!("{network}-host")),
                )
                .await
                .expect("a binding");
        }
    }

    /// A lease on `address` in `network`, held by `mac`.
    pub async fn lease(&self, network: &str, address_text: &str, mac: &str) -> Lease {
        let identity = ClientIdentity::new()
            .with(SelectorKind::Mac, mac)
            .expect("an identity");

        let (_, lease) = self
            .store
            .acquire_lease(
                &actor(),
                network,
                &identity,
                address(address_text),
                LEASE_SECONDS,
            )
            .await
            .expect("a lease");

        lease
    }

    /// Stop serving, and wait until it has.
    pub async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.serving.await;
    }
}

/// The actor a fixture's writes are recorded against.
pub fn actor() -> Actor {
    Actor::Peer(teneral_api::owning_uid())
}

/// The namespace a network's interface lives in, which is its own:
/// every consumer in view puts each network in a namespace of its own.
///
/// Deliberately not named after its network. A refusal about an
/// interface names the namespace and the interface the caller supplied
/// itself, which is not a leak -- and a test asserting that a refusal
/// never names a network it may not see would be unable to tell the
/// two apart if the namespace carried the network's name inside it.
pub fn namespace(network: &str) -> String {
    match network {
        ALPHA => "red".to_owned(),
        BETA => "blue".to_owned(),
        other => format!("ns-{other}"),
    }
}

/// An address from its written form, for a test that wants to read as
/// an operator writes.
pub fn address(text: &str) -> Ipv4Addr {
    text.parse().expect("a literal address in a test")
}

/// That address as the wire carries it.
pub fn wire_address(text: &str) -> u32 {
    proto::address(address(text))
}

/// A selector on a hardware address, as the wire carries it.
pub fn mac(value: &str) -> proto::Selector {
    proto::Selector {
        kind: proto::SelectorKind::Mac as i32,
        value: value.to_owned(),
    }
}

/// A client presenting one hardware address, as the wire carries it.
pub fn presenting(value: &str) -> proto::ClientIdentity {
    proto::ClientIdentity {
        presents: vec![mac(value)],
    }
}

/// A position, as the wire carries it.
pub fn wire_cursor(cursor: &Cursor) -> proto::Cursor {
    proto::cursor(cursor)
}

/// The refusal an answer carries, or a panic naming what arrived
/// instead.
pub fn refusal_in(response: &proto::Response) -> proto::Refusal {
    match response.answer.as_ref() {
        Some(response::Answer::Refusal(refusal)) => refusal.clone(),
        other => panic!("that request was answered with {other:?} rather than a refusal"),
    }
}

/// The resource an answer carries, or a panic naming what arrived
/// instead.
pub fn resource_in(response: &proto::Response) -> proto::Resource {
    match response.answer.as_ref() {
        Some(response::Answer::Resource(resource)) => resource.clone(),
        other => panic!("that request was answered with {other:?} rather than a resource"),
    }
}

/// The binding an answer carries, or a panic naming what arrived
/// instead.
pub fn binding_in(response: &proto::Response) -> proto::Binding {
    match resource_in(response).state {
        Some(proto::resource::State::Binding(binding)) => binding,
        other => panic!("that answer carried {other:?} rather than a binding"),
    }
}

/// The lease an answer carries, or a panic naming what arrived
/// instead.
pub fn lease_in(response: &proto::Response) -> proto::Lease {
    match resource_in(response).state {
        Some(proto::resource::State::Lease(lease)) => lease,
        other => panic!("that answer carried {other:?} rather than a lease"),
    }
}

/// The count a collection's marker carries, or a panic naming what
/// ended it instead.
pub fn collection_count(responses: &[proto::Response]) -> u64 {
    let last = responses.last().expect("a collection has a last message");

    match last.answer.as_ref() {
        Some(response::Answer::CollectionEnd(end)) => end.count,
        other => panic!("a collection ended with {other:?} rather than its marker"),
    }
}

/// Every request this suite aims at one network, built against
/// `network`.
///
/// It is the whole of the operations that name a network, which is
/// asserted rather than claimed: each one is checked against the
/// function that decides, exhaustively, which network an operation is
/// scoped by, and the count is checked against how many there are.
pub fn every_scoped_request(network: &str) -> Vec<request::Kind> {
    let selector = mac(CLIENT);
    let identity = presenting(CLIENT);
    let position = proto::Cursor {
        epoch: "not-this-store".to_owned(),
        sequence: 1,
    };

    vec![
        request::Kind::GetNetwork(proto::GetNetworkRequest {
            name: network.to_owned(),
        }),
        request::Kind::CreateNetwork(proto::CreateNetworkRequest {
            name: network.to_owned(),
        }),
        request::Kind::UpdateNetwork(proto::UpdateNetworkRequest {
            name: network.to_owned(),
            expected: Some(position.clone()),
            lease_seconds: 3_600,
        }),
        request::Kind::DeleteNetwork(proto::DeleteNetworkRequest {
            name: network.to_owned(),
            expected: Some(position.clone()),
        }),
        request::Kind::ListAttachments(proto::ListAttachmentsRequest {
            network: network.to_owned(),
        }),
        request::Kind::GetAttachment(proto::GetAttachmentRequest {
            network: network.to_owned(),
            namespace: namespace(network),
            ifname: "eth0".to_owned(),
        }),
        request::Kind::AttachInterface(proto::AttachInterfaceRequest {
            network: network.to_owned(),
            namespace: namespace(network),
            ifname: "eth1".to_owned(),
        }),
        request::Kind::DetachInterface(proto::DetachInterfaceRequest {
            network: network.to_owned(),
            namespace: namespace(network),
            ifname: "eth0".to_owned(),
            expected: Some(position.clone()),
        }),
        request::Kind::ListAddressRanges(proto::ListAddressRangesRequest {
            network: network.to_owned(),
        }),
        request::Kind::GetAddressRange(proto::GetAddressRangeRequest {
            network: network.to_owned(),
            name: "pool".to_owned(),
        }),
        request::Kind::CreateAddressRange(proto::CreateAddressRangeRequest {
            network: network.to_owned(),
            name: "second".to_owned(),
            first: wire_address("10.0.1.1"),
            last: wire_address("10.0.1.9"),
        }),
        request::Kind::UpdateAddressRange(proto::UpdateAddressRangeRequest {
            network: network.to_owned(),
            name: "pool".to_owned(),
            expected: Some(position.clone()),
            first: wire_address("10.0.0.1"),
            last: wire_address("10.0.0.50"),
        }),
        request::Kind::DeleteAddressRange(proto::DeleteAddressRangeRequest {
            network: network.to_owned(),
            name: "pool".to_owned(),
            expected: Some(position.clone()),
        }),
        request::Kind::ListBindings(proto::ListBindingsRequest {
            network: network.to_owned(),
        }),
        request::Kind::GetBinding(proto::GetBindingRequest {
            network: network.to_owned(),
            selector: Some(selector.clone()),
        }),
        request::Kind::CreateBinding(proto::CreateBindingRequest {
            network: network.to_owned(),
            selector: Some(mac("66:77:88:99:aa:bb")),
            address: wire_address(FREE),
            hostname: None,
            revoke_lease: None,
        }),
        request::Kind::UpdateBinding(proto::UpdateBindingRequest {
            network: network.to_owned(),
            selector: Some(selector.clone()),
            expected: Some(position.clone()),
            address: wire_address(FREE),
            hostname: None,
        }),
        request::Kind::DeleteBinding(proto::DeleteBindingRequest {
            network: network.to_owned(),
            selector: Some(selector),
            expected: Some(position.clone()),
        }),
        request::Kind::LookupBinding(proto::LookupBindingRequest {
            network: network.to_owned(),
            identity: Some(identity.clone()),
        }),
        request::Kind::LookupBindingOnInterface(proto::LookupBindingOnInterfaceRequest {
            network: network.to_owned(),
            namespace: namespace(network),
            ifname: "eth0".to_owned(),
            identity: Some(identity.clone()),
        }),
        request::Kind::ListLeases(proto::ListLeasesRequest {
            network: network.to_owned(),
        }),
        request::Kind::GetLease(proto::GetLeaseRequest {
            network: network.to_owned(),
            address: wire_address(LEASED),
        }),
        request::Kind::LeasesHeldBy(proto::LeasesHeldByRequest {
            network: network.to_owned(),
            identity: Some(identity),
        }),
        request::Kind::RevokeLease(proto::RevokeLeaseRequest {
            network: network.to_owned(),
            address: wire_address(LEASED),
            expected: Some(position),
        }),
        request::Kind::Digest(proto::DigestRequest {
            network: network.to_owned(),
        }),
    ]
}

// ---------------------------------------------------------------------
// The watch stream.
//
// Every read of a stream here is bounded. A test that waited forever
// for an event that is never coming would hang the suite rather than
// fail it, and the failure mode being tested -- an event that does not
// arrive -- is exactly the one that would do it.
// ---------------------------------------------------------------------

/// How long a test waits for an event before deciding there is not one.
pub const PATIENCE: Duration = Duration::from_secs(10);

/// A watch over `scope`, for every kind of resource there is.
pub fn watching(scope: TokenScope, after: Option<proto::Cursor>) -> proto::WatchRequest {
    watching_kinds(scope, &EventKind::ALL, after)
}

/// A watch over `scope`, for `kinds`.
pub fn watching_kinds(
    scope: TokenScope,
    kinds: &[EventKind],
    after: Option<proto::Cursor>,
) -> proto::WatchRequest {
    proto::WatchRequest {
        scope: Some(proto::scope(&scope)),
        kinds: kinds
            .iter()
            .map(|kind| proto::resource_kind(*kind) as i32)
            .collect(),
        after,
    }
}

/// A scope of one network, which is the one a cross-network defect
/// shows up in.
pub fn only(network: &str) -> TokenScope {
    TokenScope::Networks(vec![network.to_owned()])
}

/// The next event on a stream, or a panic saying none arrived.
pub async fn next_event(stream: &mut Stream) -> watch_event::Event {
    let event = tokio::time::timeout(PATIENCE, stream.next())
        .await
        .expect("an event arrived within the time a test waits")
        .expect("the stream carried an event rather than ending");

    event
        .event
        .expect("a watch event says which kind of event it is")
}

/// The refusal a stream was met with, or a panic naming what arrived
/// instead.
pub async fn refusal_of(stream: &mut Stream) -> proto::Refusal {
    let outcome = tokio::time::timeout(PATIENCE, stream.next())
        .await
        .expect("an answer arrived within the time a test waits");

    match outcome {
        Ok(event) => panic!("the watch was served with {event:?} rather than refused"),
        Err(error) => match error.refusal() {
            Some(refusal) => refusal.clone(),
            None => panic!("the watch failed rather than being refused: {error}"),
        },
    }
}

/// Everything up to the in-sync marker, and the position it carries.
///
/// A resync before it is a panic rather than a skip: the tests that
/// expect one read the event themselves, and a test that silently
/// tolerated one would be a test that passes when a consumer is told
/// to throw its state away.
pub async fn in_sync(stream: &mut Stream) -> (Vec<proto::ResourceEvent>, proto::Cursor) {
    let mut events = Vec::new();

    loop {
        match next_event(stream).await {
            watch_event::Event::Change(change) => events.push(change),
            watch_event::Event::InSync(marker) => {
                let at = marker.cursor.expect("an in-sync marker carries a position");

                return (events, at);
            }
            other => panic!("the stream carried {other:?} before it was in sync"),
        }
    }
}

/// The next change on a stream, skipping the heartbeats.
///
/// Heartbeats are skipped and nothing else is: a resync or a
/// disconnection in the middle of a test that wanted a change is the
/// test's answer and not noise.
pub async fn next_change(stream: &mut Stream) -> proto::ResourceEvent {
    loop {
        match next_event(stream).await {
            watch_event::Event::Change(change) => return change,
            watch_event::Event::Heartbeat(_) => continue,
            other => panic!("the stream carried {other:?} rather than a change"),
        }
    }
}

/// The resync a stream opened with, or a panic naming what arrived
/// instead.
pub async fn resync(stream: &mut Stream) -> proto::Resync {
    match next_event(stream).await {
        watch_event::Event::Resync(resync) => resync,
        other => panic!("the stream opened with {other:?} rather than a resync"),
    }
}

/// The lease a change event carries, or a panic naming what it carried
/// instead.
pub fn lease_state(change: &proto::ResourceEvent) -> proto::Lease {
    match change.state.as_ref().and_then(|state| state.state.clone()) {
        Some(proto::resource::State::Lease(lease)) => lease,
        other => panic!("that event carried {other:?} rather than a lease"),
    }
}

/// The binding a change event carries, or a panic naming what it
/// carried instead.
pub fn binding_state(change: &proto::ResourceEvent) -> proto::Binding {
    match change.state.as_ref().and_then(|state| state.state.clone()) {
        Some(proto::resource::State::Binding(binding)) => binding,
        other => panic!("that event carried {other:?} rather than a binding"),
    }
}

/// A hardware address that is this test's `index`th, so that a test
/// writing a hundred bindings has a hundred clients rather than one
/// written over.
pub fn nth_mac(index: usize) -> String {
    let index = u32::try_from(index).unwrap_or(u32::MAX);
    let bytes = index.to_be_bytes();

    format!(
        "02:00:{:02x}:{:02x}:{:02x}:{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3]
    )
}

/// The two requests whose answer is a network, aimed at `network`'s
/// own interface.
///
/// The lookup on an interface is not among them: it names the network
/// the caller believes the interface belongs to, so it is an ordinary
/// network-named operation and is in the list above.
pub fn every_interface_request(network: &str) -> Vec<request::Kind> {
    vec![
        request::Kind::InterfaceOwner(proto::InterfaceOwnerRequest {
            namespace: namespace(network),
            ifname: "eth0".to_owned(),
        }),
        request::Kind::Explain(proto::ExplainRequest {
            namespace: namespace(network),
            ifname: "eth0".to_owned(),
            identity: Some(presenting(CLIENT)),
        }),
    ]
}
