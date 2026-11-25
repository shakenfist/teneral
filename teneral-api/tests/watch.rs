//! What a watch stream is allowed to be.
//!
//! The snapshot, the resume, the three resyncs, the heartbeat, the
//! disconnection of a consumer that fell behind, and the bound on how
//! many views of the store the daemon will hold while serving all of
//! it. The handshake is the subject of the connection suite and the
//! resources are the subject of the resource suite; the harness all
//! three share is in `support`.
//!
//! Every test here runs against two networks holding the same address,
//! because a stream is the one place in this daemon where one
//! network's state is read for every consumer and filtered afterwards
//! -- the pump reads every network's history -- so a filtering defect
//! here is a cross-network leak that a single-network test could not
//! see.
//!
//! Each test stops the daemon it started, and every wait for an event
//! is bounded: the failure being tested for is an event that does not
//! arrive, which is exactly the failure that would otherwise hang the
//! suite instead of failing it.

mod support;

use std::net::Ipv4Addr;
use std::time::Duration;

use support::{
    ALPHA, BETA, Daemon, FREE, LEASED, TempDirectory, actor, address, binding_state, in_sync,
    lease_state, next_change, next_event, nth_mac, only, refusal_in, refusal_of, resync, watching,
    watching_kinds,
};
use teneral_api::proto::{self, DisconnectReason, RefusalCode, ResyncReason, request, watch_event};
use teneral_store::{
    BindingStore, Capability, EventKind, Retention, Selector, SelectorKind, TokenScope,
};

/// A year, for a retention bound that is not meant to bite on age.
const A_YEAR: u64 = 365 * 24 * 60 * 60;

/// A kind of resource, as an event on the wire names it.
fn of_kind(kind: EventKind) -> i32 {
    proto::resource_kind(kind) as i32
}

/// A selector on a hardware address, as the store stores one.
fn selector(value: &str) -> Selector {
    Selector::new(SelectorKind::Mac, value).expect("a selector in a test")
}

/// An address for the `index`th binding a test writes, so that a test
/// writing sixty of them writes sixty rather than one over and over.
fn nth_address(index: usize) -> Ipv4Addr {
    let index = u32::try_from(index).unwrap_or(0);

    Ipv4Addr::from(0x0a00_0100_u32.wrapping_add(index))
}

/// The events of `events` that are about `kind` in `network`.
fn about(
    events: &[proto::ResourceEvent],
    network: &str,
    kind: EventKind,
) -> Vec<proto::ResourceEvent> {
    events
        .iter()
        .filter(|event| event.network == network && event.kind == of_kind(kind))
        .cloned()
        .collect()
}

#[tokio::test]
async fn a_stream_scoped_to_one_network_sees_none_of_the_others_events() {
    let directory = TempDirectory::new("one-network-stream");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;
    daemon.lease(ALPHA, LEASED, "11:11:11:11:11:11").await;
    daemon.lease(BETA, LEASED, "22:22:22:22:22:22").await;

    let connection = daemon.connect(None).await;
    let mut stream = connection
        .watch(watching(only(ALPHA), None))
        .await
        .expect("the owning peer may watch a network");

    let (snapshot, at) = in_sync(&mut stream).await;

    // The snapshot, which is where a filtering defect is a
    // cross-network leak with nothing to notice it: the two networks
    // hold the same address for a different client, so one of the
    // other network's resources would read as a plausible one of this
    // network's.
    assert!(!snapshot.is_empty(), "the snapshot carried nothing at all");
    for event in &snapshot {
        assert_eq!(event.network, ALPHA, "the snapshot carried {event:?}");
        assert_eq!(
            event.origin,
            proto::EventOrigin::Snapshot as i32,
            "a snapshot's events say the resource exists rather than that it has just changed"
        );
        assert_eq!(
            event.recorded_at, None,
            "a resource row records where it was written and not when"
        );
    }

    // And the state in it is this network's rather than the other's,
    // which the fixture makes distinguishable: both networks bind the
    // same client to the same address and differ in the hostname.
    let bindings = about(&snapshot, ALPHA, EventKind::Binding);
    assert_eq!(bindings.len(), 1, "one binding is configured in alpha");
    assert_eq!(
        binding_state(&bindings[0]).hostname.as_deref(),
        Some("alpha-host")
    );

    let leases = about(&snapshot, ALPHA, EventKind::Lease);
    assert_eq!(leases.len(), 1, "one address is leased in alpha");
    let holder = lease_state(&leases[0])
        .holder
        .expect("a lease names its holder");
    assert_eq!(
        holder.value, "11:11:11:11:11:11",
        "the other network's holder of the same address reached this stream"
    );

    // Now one write in each network, the other network's first. The
    // order is total and the pump broadcasts in it, so a stream that
    // was going to be told about beta's write would have been told
    // before alpha's arrived.
    let elsewhere = daemon
        .store()
        .create_binding(
            &actor(),
            BETA,
            &selector(&nth_mac(1)),
            address(FREE),
            Some("beta-later"),
        )
        .await
        .expect("a binding in the other network");
    let here = daemon
        .store()
        .create_binding(
            &actor(),
            ALPHA,
            &selector(&nth_mac(1)),
            address(FREE),
            Some("alpha-later"),
        )
        .await
        .expect("a binding in this network");

    assert!(
        at.sequence < elsewhere.sequence() && elsewhere.sequence() < here.sequence(),
        "the fixture did not write in the order this test depends on"
    );

    let change = next_change(&mut stream).await;

    assert_eq!(change.network, ALPHA);
    assert_eq!(change.origin, proto::EventOrigin::Change as i32);
    assert_eq!(
        change.cursor.as_ref().map(|cursor| cursor.sequence),
        Some(here.sequence()),
        "the first change delivered was not this network's"
    );
    assert_eq!(
        binding_state(&change).hostname.as_deref(),
        Some("alpha-later")
    );

    drop(stream);
    daemon.stop().await;
}

#[tokio::test]
async fn a_lease_created_and_released_while_a_consumer_was_away_produces_both_events() {
    let directory = TempDirectory::new("away-and-back");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let scope = TokenScope::Networks(vec![ALPHA.to_owned(), BETA.to_owned()]);

    // Where the consumer was when it went away.
    let connection = daemon.connect(None).await;
    let mut stream = connection
        .watch(watching(scope.clone(), None))
        .await
        .expect("the owning peer may watch both networks");
    let (_, away_at) = in_sync(&mut stream).await;
    drop(stream);

    // Both networks lease the same address to different clients while
    // it is away, and this network's is given up again. Collapsing the
    // two would be cheaper and would make the stream unusable as a
    // sequence: a consumer holding the earlier state would never see
    // it end.
    let held = daemon.lease(ALPHA, LEASED, "11:11:11:11:11:11").await;
    let elsewhere = daemon.lease(BETA, LEASED, "22:22:22:22:22:22").await;
    let revoked = daemon
        .store()
        .revoke_lease(&actor(), ALPHA, address(LEASED), &held.cursor)
        .await
        .expect("the lease is still where it was read");

    let connection = daemon.connect(None).await;
    let mut stream = connection
        .watch(watching(scope, Some(away_at.clone())))
        .await
        .expect("a consumer may come back with the position it left at");

    let (events, resumed) = in_sync(&mut stream).await;

    for event in &events {
        assert_eq!(
            event.origin,
            proto::EventOrigin::Change as i32,
            "a resume delivers changes rather than a snapshot: {event:?}"
        );
        assert!(
            event
                .cursor
                .as_ref()
                .is_some_and(|cursor| cursor.sequence > away_at.sequence),
            "a resume delivered an event at or before the position presented: {event:?}"
        );
    }

    let here = about(&events, ALPHA, EventKind::Lease);
    assert_eq!(
        here.len(),
        2,
        "both events, not neither: {:?}",
        here.iter().map(|event| &event.cursor).collect::<Vec<_>>()
    );
    assert_eq!(
        here[0].cursor.as_ref().map(|cursor| cursor.sequence),
        Some(held.cursor.sequence())
    );
    assert_eq!(
        lease_state(&here[0])
            .holder
            .expect("a lease names its holder")
            .value,
        "11:11:11:11:11:11"
    );

    assert_eq!(
        here[1].cursor.as_ref().map(|cursor| cursor.sequence),
        Some(revoked.sequence())
    );
    assert!(
        here[1].state.is_none(),
        "a resource that is gone is the same event with no state: {:?}",
        here[1]
    );
    assert!(
        !here[1].actor.is_empty(),
        "a deletion has no state, so the actor on the event is the only place who did it can be \
         read"
    );

    // And the other network's lease on the same address, which this
    // scope covers too and which is a different client.
    let there = about(&events, BETA, EventKind::Lease);
    assert_eq!(there.len(), 1);
    assert_eq!(
        there[0].cursor.as_ref().map(|cursor| cursor.sequence),
        Some(elsewhere.cursor.sequence())
    );
    assert_eq!(
        lease_state(&there[0])
            .holder
            .expect("a lease names its holder")
            .value,
        "22:22:22:22:22:22"
    );

    assert!(
        resumed.sequence >= revoked.sequence(),
        "the in-sync marker is behind the events it followed"
    );

    drop(stream);
    daemon.stop().await;
}

#[tokio::test]
async fn a_position_from_before_a_mint_resyncs_because_the_epoch_changed() {
    let directory = TempDirectory::new("minted-epoch");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let connection = daemon.connect(None).await;
    let mut stream = connection
        .watch(watching(TokenScope::EveryNetwork, None))
        .await
        .expect("the owning peer may watch everything");
    let (_, before) = in_sync(&mut stream).await;
    drop(stream);

    // The most consequential write an operator can make: every
    // position any consumer remembers stops matching.
    let minted = daemon
        .store()
        .mint_epoch(&actor())
        .await
        .expect("the owning peer may mint an epoch");

    let connection = daemon.connect(None).await;
    let mut stream = connection
        .watch(watching(TokenScope::EveryNetwork, Some(before.clone())))
        .await
        .expect("a consumer may present a position from the order that has gone");

    let resync = resync(&mut stream).await;

    assert_eq!(resync.reason, ResyncReason::EpochChanged as i32);
    assert_eq!(resync.presented, Some(before.clone()));
    assert_eq!(resync.store_epoch.as_deref(), Some(minted.epoch().as_str()));
    assert_eq!(
        resync.evicted_through, None,
        "nothing was evicted, and a resync that said so would send an operator looking for a \
         consumer that was away too long"
    );

    // And then exactly what a new consumer gets.
    let (snapshot, now_at) = in_sync(&mut stream).await;

    assert!(!snapshot.is_empty(), "the resync was not followed by state");
    assert_eq!(now_at.epoch, minted.epoch().as_str());
    assert_ne!(now_at.epoch, before.epoch);

    drop(stream);
    daemon.stop().await;
}

#[tokio::test]
async fn a_position_whose_events_are_gone_resyncs_because_it_was_evicted() {
    let directory = TempDirectory::new("evicted-position");
    // A history of two events, so that a consumer is pushed out of it
    // by three writes rather than by fifty thousand.
    let daemon = Daemon::retaining(
        &directory,
        Retention::new(2, A_YEAR).expect("a retention of two events"),
    )
    .await;
    daemon.two_networks().await;

    let connection = daemon.connect(None).await;
    let mut stream = connection
        .watch(watching(TokenScope::EveryNetwork, None))
        .await
        .expect("the owning peer may watch everything");
    let (_, away_at) = in_sync(&mut stream).await;
    drop(stream);

    for index in 0..3 {
        daemon
            .store()
            .create_binding(
                &actor(),
                ALPHA,
                &selector(&nth_mac(index)),
                nth_address(index),
                None,
            )
            .await
            .expect("a binding");
    }

    let connection = daemon.connect(None).await;
    let mut stream = connection
        .watch(watching(TokenScope::EveryNetwork, Some(away_at.clone())))
        .await
        .expect("a consumer may present a position that has aged out");

    let resync = resync(&mut stream).await;

    assert_eq!(resync.reason, ResyncReason::Evicted as i32);
    assert_eq!(resync.presented, Some(away_at.clone()));

    let evicted_through = resync
        .evicted_through
        .expect("an eviction says how far the history has gone");
    assert_eq!(evicted_through.epoch, away_at.epoch);
    assert!(
        evicted_through.sequence > away_at.sequence,
        "the eviction mark is not past the position it refused"
    );
    assert_eq!(resync.latest, None);

    // And then what a new consumer gets, which is the only thing left
    // that can be true.
    let (snapshot, _) = in_sync(&mut stream).await;
    assert!(!snapshot.is_empty(), "the resync was not followed by state");

    drop(stream);
    daemon.stop().await;
}

#[tokio::test]
async fn a_position_this_store_never_issued_resyncs_saying_so() {
    let directory = TempDirectory::new("never-issued");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let connection = daemon.connect(None).await;

    // This store's own epoch with a sequence far past anything it has
    // assigned, which is what a store restored from a file-level
    // backup looks like from a consumer's side: the epoch comes back
    // with the file and the counter comes back wound.
    let epoch = connection
        .welcome()
        .position
        .as_ref()
        .expect("a welcome names a position")
        .epoch
        .clone();
    let ahead = proto::Cursor {
        epoch: epoch.clone(),
        sequence: 1_000_000,
    };

    let mut stream = connection
        .watch(watching(TokenScope::EveryNetwork, Some(ahead.clone())))
        .await
        .expect("a consumer may present a position from the future");

    let resync = resync(&mut stream).await;

    assert_eq!(resync.reason, ResyncReason::NeverIssued as i32);
    assert_eq!(resync.presented, Some(ahead));

    let latest = resync
        .latest
        .expect("the answer says what this store has issued, which is what makes it legible");
    assert_eq!(latest.epoch, epoch);
    assert!(latest.sequence < 1_000_000);
    assert_eq!(
        resync.evicted_through, None,
        "nothing was evicted: reporting this as eviction would say something was discarded when \
         nothing was ever there"
    );

    let (snapshot, _) = in_sync(&mut stream).await;
    assert!(!snapshot.is_empty(), "the resync was not followed by state");

    drop(stream);
    daemon.stop().await;
}

#[tokio::test]
async fn nothing_is_delivered_twice_and_nothing_is_missing_across_the_join() {
    /// How many writes land while the consumer is connecting.
    ///
    /// Enough of them, and written as fast as the store will take
    /// them, that the window between a stream subscribing and the
    /// position its first read is taken at has writes in it. A handful
    /// would leave the window empty on a fast machine, and a test that
    /// never exercises the join is a test that would pass with the
    /// join written the other way round -- which is why the two
    /// assertions after the loop below check that both sides of it
    /// happened rather than trusting the timing.
    const WRITES: usize = 200;

    let directory = TempDirectory::new("the-join");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let connection = daemon.connect(None).await;

    // Two writes before the connect, so that the snapshot certainly
    // has something of this test's to describe rather than that being
    // a thing the timing decides.
    let mut before = Vec::new();
    for index in 0..2 {
        before.push(
            daemon
                .store()
                .create_binding(
                    &actor(),
                    ALPHA,
                    &selector(&nth_mac(index)),
                    nth_address(index),
                    None,
                )
                .await
                .expect("a binding"),
        );
    }

    // And then writes landing while the consumer connects, which is
    // the window the join is about: a stream that read its snapshot
    // before it subscribed to the stream would lose whatever landed
    // between the two, and nothing in what it delivered would say so.
    let store = daemon.shared_store();
    let writing = tokio::spawn(async move {
        let mut written = Vec::new();

        for index in 2..WRITES {
            let network = if index % 2 == 0 { ALPHA } else { BETA };
            let position = store
                .create_binding(
                    &actor(),
                    network,
                    &selector(&nth_mac(index)),
                    nth_address(index),
                    None,
                )
                .await
                .expect("a binding");

            written.push(position);
        }

        written
    });

    let mut stream = connection
        .watch(watching(TokenScope::EveryNetwork, None))
        .await
        .expect("the owning peer may watch everything");

    let (snapshot, at) = in_sync(&mut stream).await;

    let mut written = before;
    written.extend(writing.await.expect("the writes all landed"));
    let last = written.last().expect("the test wrote something").sequence();

    // Read until the last write has arrived, or until it turns out to
    // have been inside the snapshot's position.
    let mut delivered: Vec<u64> = Vec::new();
    while last > at.sequence && !delivered.contains(&last) {
        let change = next_change(&mut stream).await;
        let position = change.cursor.expect("an event carries its position");

        assert_eq!(position.epoch, at.epoch);
        assert!(
            position.sequence > at.sequence,
            "an event the snapshot already covered was delivered again, at {}",
            position.sequence
        );
        assert!(
            !delivered.contains(&position.sequence),
            "the event at {} was delivered twice",
            position.sequence
        );
        assert!(
            delivered
                .last()
                .is_none_or(|previous| *previous < position.sequence),
            "the event at {} arrived after one above it",
            position.sequence
        );

        delivered.push(position.sequence);
    }

    // Both sides of the join were exercised, which is what makes
    // everything below a statement about it: writes that landed before
    // the position the snapshot was read at, and writes that landed
    // after the stream had joined.
    assert!(
        !delivered.is_empty(),
        "every write landed before the join, so nothing here says what happens across it"
    );
    assert!(
        written
            .iter()
            .any(|position| position.sequence() <= at.sequence),
        "every write landed after the join, so nothing here says what the snapshot covered"
    );

    // Nothing missing. Every write is either at or below the position
    // the snapshot was read at -- in which case the snapshot describes
    // it, which is asserted rather than assumed below -- or it was
    // delivered as a change exactly once.
    for position in &written {
        let sequence = position.sequence();

        if sequence <= at.sequence {
            assert!(
                snapshot
                    .iter()
                    .any(|event| event.cursor.as_ref().map(|cursor| cursor.sequence)
                        == Some(sequence)),
                "the write at {position} is below the snapshot's position and not in it"
            );
            assert!(
                !delivered.contains(&sequence),
                "the write at {position} was in the snapshot and delivered again"
            );
        } else {
            assert!(
                delivered.contains(&sequence),
                "the write at {position} reached neither the snapshot nor the stream"
            );
        }
    }

    drop(stream);
    daemon.stop().await;
}

#[tokio::test]
async fn an_event_written_while_a_stream_was_opening_is_delivered_rather_than_lost() {
    /// How many bindings the fixture configures before the stream
    /// opens.
    ///
    /// Enough that the snapshot of them is larger than the socket
    /// buffer any host gives a connection, so that the daemon is
    /// still writing that snapshot while the writes below land. That
    /// is what makes this test a statement about the order of two
    /// operations rather than about how fast a machine runs them: the
    /// daemon cannot have finished its first read until the consumer
    /// starts reading, and the consumer below does not start until
    /// the writes are in and the daemon's one reader has passed them.
    const CONFIGURED: usize = 1_200;

    /// How many events land while the stream is opening.
    const DURING: usize = 5;

    let directory = TempDirectory::new("opening-window");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    // A hostname at the length the store will take, which is what
    // makes each event large enough for the count above to be enough.
    let hostname = "h".repeat(255);
    for index in 0..CONFIGURED {
        daemon
            .store()
            .create_binding(
                &actor(),
                ALPHA,
                &selector(&nth_mac(index)),
                nth_address(index),
                Some(&hostname),
            )
            .await
            .expect("a binding");
    }

    let connection = daemon.connect(None).await;
    let mut stream = connection
        .watch(watching(only(ALPHA), None))
        .await
        .expect("the request is written");

    // One event read, which is proof that the daemon has taken its
    // first read -- the position it will report is fixed -- and has
    // begun writing what it found.
    let first = next_change(&mut stream).await;
    assert_eq!(first.origin, proto::EventOrigin::Snapshot as i32);

    // Now the writes, into the window between that read and whatever
    // the daemon does next. A stream that subscribed to the daemon's
    // one reader only after reading would not be subscribed yet, and
    // these are the events it would never hear about: the reader reads
    // the history once, from its own position, so an event it has
    // already passed is one no stream can ask for again.
    let mut written = Vec::new();
    for index in 0..DURING {
        written.push(
            daemon
                .store()
                .create_binding(
                    &actor(),
                    ALPHA,
                    &selector(&nth_mac(CONFIGURED + index)),
                    nth_address(CONFIGURED + index),
                    None,
                )
                .await
                .expect("a binding"),
        );
    }

    // Long enough for that reader to have woken and passed them.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (rest, at) = in_sync(&mut stream).await;

    assert!(
        rest.len() + 1 >= CONFIGURED,
        "the snapshot carried {} events, which is not the whole of what was configured",
        rest.len() + 1
    );
    assert!(
        at.sequence < written[0].sequence(),
        "the daemon's first read happened after the writes this test depends on landing during it"
    );

    // And every one of them arrives, in the order the store applied
    // them.
    for position in &written {
        let change = next_change(&mut stream).await;

        assert_eq!(
            change.cursor.as_ref().map(|cursor| cursor.sequence),
            Some(position.sequence()),
            "an event written while the stream was opening never reached it"
        );
        assert_eq!(change.origin, proto::EventOrigin::Change as i32);
    }

    drop(stream);
    daemon.stop().await;
}

#[tokio::test]
async fn the_position_a_heartbeat_carries_is_one_a_resume_accepts() {
    let directory = TempDirectory::new("heartbeat");
    // Faster than the fifteen seconds a deployment gets, which is
    // longer than a test should take.
    let daemon = Daemon::tuned(&directory, |config| {
        config.heartbeat = Duration::from_millis(50);
    })
    .await;
    daemon.two_networks().await;

    let connection = daemon.connect(None).await;
    let mut stream = connection
        .watch(watching(only(ALPHA), None))
        .await
        .expect("the owning peer may watch a network");
    let (_, at) = in_sync(&mut stream).await;

    // A write in the network this stream is not watching. The stream
    // receives it, delivers nothing, and may still resume from where
    // it was -- which is the whole reason a heartbeat is worth
    // anything: without it a quiet network is the one most likely to
    // fall out of the retained history.
    let elsewhere = daemon
        .store()
        .create_binding(&actor(), BETA, &selector(&nth_mac(1)), nth_address(1), None)
        .await
        .expect("a binding in the other network");

    let mut beat = None;
    for _ in 0..100 {
        if let watch_event::Event::Heartbeat(heartbeat) = next_event(&mut stream).await {
            let cursor = heartbeat.cursor.expect("a heartbeat carries a position");
            if cursor.sequence >= elsewhere.sequence() {
                beat = Some(cursor);
                break;
            }
        }
    }

    let beat = beat.expect("a heartbeat carried the position the store had reached");
    assert!(
        beat.sequence > at.sequence,
        "the heartbeat did not advance the consumer's resume point"
    );

    drop(stream);

    // Something in this stream's own network, after the heartbeat, so
    // that resuming from it has to deliver an event rather than
    // merely be accepted.
    let here = daemon
        .store()
        .create_binding(
            &actor(),
            ALPHA,
            &selector(&nth_mac(2)),
            nth_address(2),
            None,
        )
        .await
        .expect("a binding in this network");

    let connection = daemon.connect(None).await;
    let mut stream = connection
        .watch(watching(only(ALPHA), Some(beat.clone())))
        .await
        .expect("a consumer may come back with the position a heartbeat gave it");

    let (events, _) = in_sync(&mut stream).await;

    assert_eq!(
        events.len(),
        1,
        "a resume from a heartbeat's position delivered {events:?}"
    );
    assert_eq!(
        events[0].cursor.as_ref().map(|cursor| cursor.sequence),
        Some(here.sequence())
    );

    drop(stream);
    daemon.stop().await;
}

#[tokio::test]
async fn a_daemon_at_its_view_bound_refuses_rather_than_serving_part_of_an_answer() {
    let directory = TempDirectory::new("no-views");
    // A daemon that will hold no view at all, which is the bound with
    // its last permit taken: the refusal past the bound is the same
    // refusal whatever the number, and this is the one a test can
    // reach without holding sixty-four threads open to prove it. The
    // bound itself, and that a permit comes back when a view is
    // dropped, are checked where the permits are.
    let daemon = Daemon::tuned(&directory, |config| config.max_live_views = 0).await;
    daemon.two_networks().await;

    let connection = daemon.connect(None).await;
    let mut stream = connection
        .watch(watching(TokenScope::EveryNetwork, None))
        .await
        .expect("the request is written");

    let refusal = refusal_of(&mut stream).await;

    assert_eq!(refusal.code, RefusalCode::TooManyViews as i32);
    assert_eq!(
        refusal.retry_after_seconds,
        Some(1),
        "a caller refused for want of a view is told when to come back"
    );

    // An ordinary read is refused by the same bound, because every
    // view this daemon opens is opened in one place.
    let mut connection = daemon.connect(None).await;
    let response = connection
        .request(request::Kind::ListNetworks(proto::ListNetworksRequest {}))
        .await
        .expect("the daemon answered");

    assert_eq!(refusal_in(&response).code, RefusalCode::TooManyViews as i32);

    assert_eq!(
        daemon.store().live_views(),
        0,
        "a view was opened past a bound of none"
    );

    daemon.stop().await;
}

#[tokio::test]
async fn a_watch_whose_scope_is_wider_than_its_credential_is_refused_rather_than_narrowed() {
    let directory = TempDirectory::new("wider-scope");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let credential = daemon
        .mint("alpha-watcher", only(ALPHA), &[Capability::Watch])
        .await;

    // Every network, asked for by a credential that may reach one of
    // them. Refused rather than served as the one it may reach: a
    // consumer handed the narrower stream would believe it was seeing
    // everything, which is the silently wrong answer this design
    // exists to prevent.
    let connection = daemon.connect(Some(&credential.secret)).await;
    let mut stream = connection
        .watch(watching(TokenScope::EveryNetwork, None))
        .await
        .expect("the request is written");

    let refusal = refusal_of(&mut stream).await;

    assert_eq!(refusal.code, RefusalCode::OutsideScope as i32);
    assert_eq!(
        refusal.resource_id, None,
        "the refusal named what it refused, and the name of a network is its identity"
    );
    assert!(
        !refusal.detail.contains(BETA),
        "the refusal named a network the caller may not see: {}",
        refusal.detail
    );

    // Two networks, one of which it may reach, which is the same
    // answer for the same reason.
    let connection = daemon.connect(Some(&credential.secret)).await;
    let mut stream = connection
        .watch(watching(
            TokenScope::Networks(vec![ALPHA.to_owned(), BETA.to_owned()]),
            None,
        ))
        .await
        .expect("the request is written");

    assert_eq!(
        refusal_of(&mut stream).await.code,
        RefusalCode::OutsideScope as i32
    );

    // And its own network is served, so what is being checked above is
    // the width of the scope and not the credential.
    let connection = daemon.connect(Some(&credential.secret)).await;
    let mut stream = connection
        .watch(watching(only(ALPHA), None))
        .await
        .expect("a credential may watch the network it was scoped to");

    let (snapshot, _) = in_sync(&mut stream).await;

    assert!(!snapshot.is_empty());
    assert!(snapshot.iter().all(|event| event.network == ALPHA));

    drop(stream);
    daemon.stop().await;
}

#[tokio::test]
async fn a_watch_that_names_no_scope_no_kind_or_an_unknown_kind_is_refused() {
    let directory = TempDirectory::new("refused-watches");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let unknown_kind = proto::WatchRequest {
        // A kind from a vocabulary this daemon does not serve, which a
        // consumer written against a later schema would send.
        kinds: vec![7],
        ..watching(TokenScope::EveryNetwork, None)
    };

    let refused = [
        (
            proto::WatchRequest {
                scope: None,
                ..watching(TokenScope::EveryNetwork, None)
            },
            RefusalCode::InvalidRequest,
            "a scope nobody set",
        ),
        (
            watching(TokenScope::Networks(Vec::new()), None),
            RefusalCode::InvalidRequest,
            "a scope of no networks",
        ),
        (
            watching_kinds(TokenScope::EveryNetwork, &[], None),
            RefusalCode::InvalidRequest,
            "a subscription to no kinds",
        ),
        (
            unknown_kind,
            RefusalCode::UnknownResourceKind,
            "a kind this daemon does not serve",
        ),
    ];

    for (request, code, what) in refused {
        let connection = daemon.connect(None).await;
        let mut stream = connection
            .watch(request)
            .await
            .expect("the request is written");

        assert_eq!(
            refusal_of(&mut stream).await.code,
            code as i32,
            "{what} was not refused as it should have been"
        );
    }

    daemon.stop().await;
}

#[tokio::test]
async fn a_stream_is_told_the_daemon_is_stopping_rather_than_going_quiet() {
    let directory = TempDirectory::new("shutting-down");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let connection = daemon.connect(None).await;
    let mut stream = connection
        .watch(watching(only(ALPHA), None))
        .await
        .expect("the owning peer may watch a network");
    let (_, at) = in_sync(&mut stream).await;

    daemon.stop().await;

    // A closed socket is indistinguishable from a network that went
    // away, and the two want different behaviour from a consumer --
    // which is why the daemon says which this was.
    match next_event(&mut stream).await {
        watch_event::Event::Disconnecting(disconnecting) => {
            assert_eq!(
                disconnecting.reason,
                DisconnectReason::ShuttingDown as i32,
                "{disconnecting:?}"
            );
            assert_eq!(
                disconnecting.cursor.map(|cursor| cursor.sequence),
                Some(at.sequence),
                "the final event does not say where the consumer may resume from"
            );
        }
        other => panic!("a stopping daemon left the stream carrying {other:?}"),
    }
}
