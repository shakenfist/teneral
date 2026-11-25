//! What a watch that outlives its connections is allowed to be.
//!
//! The suite next door checks what one connection's worth of stream
//! carries. This one checks the thing above it: the helper that holds
//! the position, reconnects, presents it, and tells its caller what
//! happened rather than smoothing it over. The defects it is looking
//! for are the ones a consumer writing this for itself would ship --
//! a position taken from a snapshot's events, a heartbeat ignored
//! until a quiet network falls out of the retained history, a refused
//! credential presented again forever, and a backoff that is a busy
//! loop.
//!
//! Every test here runs against two networks holding the same
//! address, because a resume is filtered by scope exactly as a
//! snapshot is and a reconnection is the moment at which a filter is
//! applied again from scratch. The one test with no store in it is
//! the backoff, which never reaches a daemon.
//!
//! Each test stops the daemon it started, and nothing waits without a
//! bound: the failure being tested for is an event that does not
//! arrive, which is the failure that would otherwise hang the suite
//! instead of failing it. The backoff is measured on a paused clock
//! rather than slept through, because a test that waited for real
//! delays is one somebody later makes faster by shortening the very
//! margin it was measuring.

mod support;

use std::net::Ipv4Addr;
use std::time::Duration;

use support::{ALPHA, BETA, Daemon, PATIENCE, TempDirectory, actor, nth_mac, only, watching};
use teneral_api::client::{Observation, Watch, WatchConfig};
use teneral_api::proto::{self, DisconnectReason, RefusalCode, ResyncReason, watch_event};
use teneral_store::{BindingStore, Retention, Selector, SelectorKind, TokenScope};

/// A year, for a retention bound that is not meant to bite on age.
const A_YEAR: u64 = 365 * 24 * 60 * 60;

/// A history of a handful of events, so that a position falls out of
/// it after a handful of writes rather than after fifty thousand.
const RETAINED: u64 = 4;

/// That history, as the store takes it.
fn short_history() -> Retention {
    Retention::new(RETAINED, A_YEAR).expect("a retention of a handful of events")
}

/// A selector on a hardware address, as the store stores one.
fn selector(value: &str) -> Selector {
    Selector::new(SelectorKind::Mac, value).expect("a selector in a test")
}

/// An address for the `index`th binding a test writes, so that a test
/// writing six of them writes six rather than one over and over.
fn nth_address(index: usize) -> Ipv4Addr {
    let index = u32::try_from(index).unwrap_or(0);

    Ipv4Addr::from(0x0a00_0100_u32.wrapping_add(index))
}

/// A watch of `request` on `directory`'s socket, reconnecting faster
/// than a deployment would.
///
/// The delays are the two numbers a test cannot take at their
/// defaults: a hundred milliseconds before the first attempt is
/// longer than the gap this suite leaves between stopping a daemon
/// and starting the next one, and a thirty-second ceiling is longer
/// than the suite.
fn prompt(directory: &TempDirectory, request: proto::WatchRequest) -> WatchConfig {
    let mut config = WatchConfig::new(directory.socket(), request);
    config.first_delay = Duration::from_millis(5);
    config.max_delay = Duration::from_millis(20);

    config
}

/// The next observation, or a panic saying none arrived.
async fn observe(watch: &mut Watch) -> Observation {
    tokio::time::timeout(PATIENCE, watch.next())
        .await
        .expect("an observation arrived within the time a test waits")
        .expect("the watch was still going")
        .expect("the watch kept reading rather than handing over an error")
}

/// The next event, or a panic naming the account of a reconnection
/// that arrived where an event was wanted.
async fn observed_event(watch: &mut Watch) -> watch_event::Event {
    match observe(watch).await {
        Observation::Event(event) => event.event.expect("an event says which kind it is"),
        Observation::Reconnecting(reconnecting) => panic!(
            "the watch lost its connection where an event was expected: {:?}",
            reconnecting.cause
        ),
    }
}

/// Everything up to the in-sync marker, and the position it carries.
///
/// A resync here is a panic rather than a skip: the tests that expect
/// one read it themselves, and a helper that silently tolerated one
/// would be one that passes while a consumer is being told to throw
/// its state away.
async fn watched_in_sync(watch: &mut Watch) -> proto::Cursor {
    loop {
        match observed_event(watch).await {
            watch_event::Event::Change(_) => {}
            watch_event::Event::InSync(marker) => {
                return marker.cursor.expect("an in-sync marker carries a position");
            }
            other => panic!("the watch carried {other:?} before it was in sync"),
        }
    }
}

#[tokio::test]
async fn a_daemon_restarted_under_a_watch_delivers_what_was_written_while_it_was_gone() {
    let directory = TempDirectory::new("restart-under-watch");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    // Kept past the daemon's own life, so that the writes below land
    // in the store while nothing at all is serving the socket. That
    // is the case this helper exists for: a daemon restarted under a
    // consumer that is not restarted with it.
    let store = daemon.shared_store();

    let mut watch = Watch::new(prompt(&directory, watching(only(ALPHA), None)));
    let at = watched_in_sync(&mut watch).await;

    daemon.stop().await;

    // Alternating networks, so that the scope has to be applied to a
    // resume and not only to a snapshot: a helper that reconnected
    // without its position would see the present, and one whose
    // resume was unfiltered would see the other network's bindings.
    let mut written = Vec::new();
    for (index, network) in [ALPHA, BETA, ALPHA, BETA, ALPHA].into_iter().enumerate() {
        let cursor = store
            .create_binding(
                &actor(),
                network,
                &selector(&nth_mac(index)),
                nth_address(index),
                None,
            )
            .await
            .expect("a binding written while the daemon is gone");

        if network == ALPHA {
            written.push(cursor.sequence());
        }
    }

    // Before the next daemon opens the same file, so that one store
    // object serves it rather than two.
    drop(store);

    let daemon = Daemon::owned(&directory).await;

    let mut delivered = Vec::new();
    let mut told_it_was_stopping = 0;
    let mut reconnections = 0;

    while delivered.len() < written.len() {
        match observe(&mut watch).await {
            Observation::Reconnecting(_) => reconnections += 1,
            Observation::Event(event) => match event.event.expect("an event says which kind it is")
            {
                watch_event::Event::Disconnecting(ending) => {
                    assert_eq!(
                        ending.reason,
                        DisconnectReason::ShuttingDown as i32,
                        "a daemon being stopped said {ending:?}"
                    );
                    told_it_was_stopping += 1;
                }
                watch_event::Event::Change(change) => {
                    assert_eq!(
                        change.network, ALPHA,
                        "a watch of one network was delivered {change:?} on a resume"
                    );
                    delivered.push(
                        change
                            .cursor
                            .expect("an event carries its position")
                            .sequence,
                    );
                }
                watch_event::Event::InSync(_) | watch_event::Event::Heartbeat(_) => {}
                watch_event::Event::Resync(resync) => panic!(
                    "the position the watch presented was answered with {resync:?} rather than \
                     resumed, so a consumer was told to throw away state it did not have to"
                ),
            },
        }
    }

    assert_eq!(
        delivered, written,
        "the resume delivered a different set of events from the ones written while the daemon \
         was gone"
    );
    assert_eq!(
        told_it_was_stopping, 1,
        "the caller was not told once that the daemon was stopping"
    );
    assert!(
        reconnections >= 1,
        "the caller was never told that the connection had gone"
    );
    assert_eq!(
        watch.position().map(|position| position.sequence),
        written.last().copied(),
        "the watch is not holding the last position it was delivered"
    );
    assert!(
        watch
            .position()
            .is_some_and(|position| position.sequence > at.sequence),
        "the watch did not move past where it was when the daemon stopped"
    );

    drop(watch);
    daemon.stop().await;
}

#[tokio::test]
async fn a_position_evicted_while_the_daemon_was_gone_reaches_the_caller_as_a_resync() {
    let directory = TempDirectory::new("evicted-under-watch");
    let daemon = Daemon::retaining(&directory, short_history()).await;
    daemon.two_networks().await;
    let store = daemon.shared_store();

    let mut watch = Watch::new(prompt(&directory, watching(TokenScope::EveryNetwork, None)));
    let at = watched_in_sync(&mut watch).await;

    daemon.stop().await;

    for (index, network) in [ALPHA, BETA, ALPHA, BETA, ALPHA, BETA]
        .into_iter()
        .enumerate()
    {
        store
            .create_binding(
                &actor(),
                network,
                &selector(&nth_mac(index)),
                nth_address(index),
                None,
            )
            .await
            .expect("a binding written while the daemon is gone");
    }

    drop(store);

    let daemon = Daemon::retaining(&directory, short_history()).await;

    let resync = loop {
        match observe(&mut watch).await {
            Observation::Reconnecting(_) => {}
            Observation::Event(event) => match event.event.expect("an event says which kind it is")
            {
                watch_event::Event::Resync(resync) => break resync,
                watch_event::Event::Disconnecting(_) => {}
                other => panic!(
                    "a position the daemon no longer holds was answered with {other:?} rather \
                     than a resync, which is a consumer carrying on across a gap"
                ),
            },
        }
    };

    assert_eq!(resync.reason, ResyncReason::Evicted as i32);
    assert_eq!(
        resync.presented.as_ref(),
        Some(&at),
        "the resync is about a position other than the one the watch was holding"
    );

    // The snapshot that follows is where a hand-rolled helper goes
    // wrong: its events carry the position each resource was last
    // written at, and a helper that took the highest of them as its
    // resume point would ask a later daemon to carry on from a place
    // in the stream it never reached, losing the rest of the snapshot
    // for good. So the position must not move until the marker.
    loop {
        match observed_event(&mut watch).await {
            watch_event::Event::Change(_) => assert_eq!(
                watch.position(),
                Some(&at),
                "the watch advanced its resume point inside a snapshot it had not finished"
            ),
            watch_event::Event::InSync(marker) => {
                let now = marker.cursor.expect("an in-sync marker carries a position");

                assert!(
                    now.sequence > at.sequence,
                    "the marker that completed the snapshot did not move the position on"
                );
                assert_eq!(
                    watch.position(),
                    Some(&now),
                    "the watch did not take the position its snapshot completed at"
                );

                break;
            }
            other => panic!("the snapshot carried {other:?}"),
        }
    }

    drop(watch);
    daemon.stop().await;
}

#[tokio::test]
async fn an_epoch_minted_while_the_daemon_was_gone_reaches_the_caller_as_a_resync() {
    let directory = TempDirectory::new("minted-under-watch");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;
    let store = daemon.shared_store();

    let mut watch = Watch::new(prompt(&directory, watching(TokenScope::EveryNetwork, None)));
    let at = watched_in_sync(&mut watch).await;

    daemon.stop().await;

    // The restore procedure, from the store's side: every position
    // any consumer remembers stops matching.
    let minted = store
        .mint_epoch(&actor())
        .await
        .expect("the owning peer may mint an epoch");

    drop(store);

    let daemon = Daemon::owned(&directory).await;

    let resync = loop {
        match observe(&mut watch).await {
            Observation::Reconnecting(_) => {}
            Observation::Event(event) => match event.event.expect("an event says which kind it is")
            {
                watch_event::Event::Resync(resync) => break resync,
                watch_event::Event::Disconnecting(_) => {}
                other => panic!(
                    "a position from the order that has gone was answered with {other:?} rather \
                     than a resync"
                ),
            },
        }
    };

    assert_eq!(resync.reason, ResyncReason::EpochChanged as i32);
    assert_eq!(resync.presented.as_ref(), Some(&at));
    assert_eq!(resync.store_epoch.as_deref(), Some(minted.epoch().as_str()));

    let now = watched_in_sync(&mut watch).await;

    assert_eq!(now.epoch, minted.epoch().as_str());
    assert_eq!(
        watch.position(),
        Some(&now),
        "the watch is still holding a position from an order the daemon no longer issues, which \
         it would present again and be resynced for again"
    );

    drop(watch);
    daemon.stop().await;
}

#[tokio::test]
async fn a_quiet_watch_stays_inside_the_retained_history_by_way_of_its_heartbeats() {
    let directory = TempDirectory::new("quiet-watch");
    // A heartbeat far faster than the fifteen seconds a deployment
    // gets, which is longer than a test should take.
    let daemon = Daemon::retaining_tuned(&directory, short_history(), |config| {
        config.heartbeat = Duration::from_millis(20);
    })
    .await;
    daemon.two_networks().await;
    let store = daemon.shared_store();

    // One network, so that the other's writes are the idle period:
    // this watch is sent nothing while the history moves past it.
    let mut watch = Watch::new(prompt(&directory, watching(only(ALPHA), None)));
    let at = watched_in_sync(&mut watch).await;

    let mut last = None;
    for index in 0..6 {
        last = Some(
            store
                .create_binding(
                    &actor(),
                    BETA,
                    &selector(&nth_mac(index)),
                    nth_address(index),
                    None,
                )
                .await
                .expect("a binding in the network this watch is not watching"),
        );
    }

    let last = last.expect("six bindings were written in the other network");

    // The position this watch started from is outside the retained
    // history now, which is what makes the resume below a statement
    // about the heartbeats: the suite's eviction test is the same
    // arrangement without them, and it resyncs.
    assert!(
        last.sequence() - at.sequence > RETAINED,
        "the other network's writes did not push the watch's starting position out of the \
         retained history, so resuming from it would prove nothing"
    );

    // The position is now outside the retained history, so a watch
    // that had ignored its heartbeats would be resynced below. Wait
    // for one that has caught up, bounded: a heartbeat that never
    // arrives is the defect, not a reason to wait forever.
    let mut caught_up = false;
    for _ in 0..400 {
        if let watch_event::Event::Heartbeat(beat) = observed_event(&mut watch).await {
            let cursor = beat.cursor.expect("a heartbeat carries a position");
            if cursor.sequence >= last.sequence() {
                caught_up = true;
                break;
            }
        }
    }

    assert!(
        caught_up,
        "no heartbeat carried the position the store had reached"
    );
    assert_eq!(
        watch.position().map(|position| position.sequence),
        Some(last.sequence()),
        "the watch did not take the position its heartbeats were carrying, which is the whole \
         reason a quiet stream is sent them"
    );

    daemon.stop().await;

    // Something in this watch's own network, so that resuming has to
    // deliver an event rather than merely be accepted.
    let here = store
        .create_binding(
            &actor(),
            ALPHA,
            &selector("aa:bb:cc:dd:ee:ff"),
            nth_address(90),
            None,
        )
        .await
        .expect("a binding in the network this watch is watching");

    drop(store);

    let daemon = Daemon::retaining(&directory, short_history()).await;

    let mut delivered = None;
    while delivered.is_none() {
        match observe(&mut watch).await {
            Observation::Reconnecting(_) => {}
            Observation::Event(event) => match event.event.expect("an event says which kind it is")
            {
                watch_event::Event::Change(change) => {
                    delivered = Some(change);
                }
                watch_event::Event::Disconnecting(_) | watch_event::Event::Heartbeat(_) => {}
                watch_event::Event::Resync(resync) => panic!(
                    "the position the heartbeats gave this watch was answered with {resync:?}, so \
                     a quiet consumer fell out of the retained history with the mechanism meant \
                     to stop that working"
                ),
                watch_event::Event::InSync(marker) => panic!(
                    "the resume was in sync at {marker:?} without delivering the event written \
                     while the daemon was gone"
                ),
            },
        }
    }

    let delivered = delivered.expect("the loop ended with an event");

    assert_eq!(delivered.network, ALPHA);
    assert_eq!(
        delivered.cursor.map(|cursor| cursor.sequence),
        Some(here.sequence())
    );

    drop(watch);
    daemon.stop().await;
}

#[tokio::test]
async fn a_refused_credential_is_surfaced_rather_than_presented_again() {
    let directory = TempDirectory::new("refused-credential");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    let mut watch = Watch::new(
        prompt(&directory, watching(TokenScope::EveryNetwork, None))
            .with_credential("not-a-secret"),
    );

    let outcome = tokio::time::timeout(PATIENCE, watch.next())
        .await
        .expect("the watch answered within the time a test waits")
        .expect("the watch said something before finishing");

    let error = match outcome {
        Err(error) => error,
        Ok(observation) => panic!(
            "a credential the daemon does not hold produced {observation:?} rather than the \
             refusal, so a consumer would wait on a stream it is never going to be served"
        ),
    };

    let refusal = error
        .refusal()
        .expect("the refusal reached the caller as the refusal it is");
    assert_eq!(refusal.code, RefusalCode::Unauthenticated as i32);

    // And it stops there. A refusal is an answer rather than a
    // failure, so going back with the same credential would be a
    // helper hammering a socket with something that will never work.
    let after = tokio::time::timeout(PATIENCE, watch.next())
        .await
        .expect("the watch answered within the time a test waits");

    assert!(
        after.is_none(),
        "the watch went back to a daemon that had already refused this credential"
    );

    daemon.stop().await;
}

#[tokio::test]
async fn a_refusal_is_presented_again_only_when_the_caller_asked_for_that() {
    let directory = TempDirectory::new("retried-refusal");
    let daemon = Daemon::owned(&directory).await;
    daemon.two_networks().await;

    // The one deployment this is for: a consumer started before the
    // credential it is to present has been minted. It is off by
    // default because everywhere else it turns a loud failure into a
    // quiet one.
    let mut config =
        prompt(&directory, watching(TokenScope::EveryNetwork, None)).with_credential("not-yet");
    config.retry_refusals = true;

    let mut watch = Watch::new(config);

    for attempt in 1..=2 {
        match observe(&mut watch).await {
            Observation::Reconnecting(reconnecting) => {
                assert_eq!(reconnecting.attempt, attempt);
                assert_eq!(
                    reconnecting
                        .cause
                        .refusal()
                        .map(|refusal| refusal.code)
                        .unwrap_or_default(),
                    RefusalCode::Unauthenticated as i32,
                    "the watch reconnected for something other than the refusal: {:?}",
                    reconnecting.cause
                );
            }
            Observation::Event(event) => {
                panic!("a refused credential was served {event:?}")
            }
        }
    }

    drop(watch);
    daemon.stop().await;
}

#[tokio::test(start_paused = true)]
async fn the_delay_between_attempts_doubles_to_a_ceiling_rather_than_spinning() {
    // No daemon, and no store: the socket path is one nothing has
    // ever bound, so every attempt fails in `connect` without waiting
    // on anything but this test's own clock. That clock is paused, so
    // the delays below are the delays the watch asked for rather than
    // what a loaded machine happened to deliver.
    let directory = TempDirectory::new("no-daemon");

    let mut config = WatchConfig::new(directory.socket(), watching(TokenScope::EveryNetwork, None));
    config.first_delay = Duration::from_millis(100);
    config.max_delay = Duration::from_millis(800);

    let mut watch = Watch::new(config);

    let started = tokio::time::Instant::now();
    let mut delays = Vec::new();

    for _ in 0..8 {
        match observe(&mut watch).await {
            Observation::Reconnecting(reconnecting) => {
                assert_eq!(
                    usize::try_from(reconnecting.attempt).unwrap_or(0),
                    delays.len() + 1,
                    "the attempts are not being counted one at a time"
                );
                delays.push(reconnecting.delay);
            }
            Observation::Event(event) => {
                panic!("a socket nothing has ever served produced {event:?}")
            }
        }
    }

    assert_eq!(
        delays,
        [100, 200, 400, 800, 800, 800, 800, 800]
            .map(Duration::from_millis)
            .to_vec(),
        "the delay between attempts is not the first delay doubled to the ceiling"
    );

    // The eighth attempt's delay has been announced and not yet
    // waited, so what has elapsed is the first seven. Equality rather
    // than a bound: on a paused clock the only thing that moves time
    // is a timer this watch set, so an extra wait or a missing one
    // both show here.
    assert_eq!(
        started.elapsed(),
        delays[..7].iter().sum::<Duration>(),
        "the watch spent a different amount of time waiting than it said it would"
    );

    assert!(
        watch.position().is_none(),
        "a watch that has never been served holds a position, which it would present as though a \
         daemon had given it one"
    );
}
