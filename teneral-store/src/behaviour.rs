//! What the store does, exercised through the interface it publishes.
//!
//! These tests call the trait and nothing else, with two exceptions
//! that are deliberate and marked where they happen: a test that
//! needs a state the interface cannot produce writes it with the
//! engine directly, so that the database can be seen refusing it, and
//! a test that reads the event log does so with the engine because no
//! trait method reads it yet.
//!
//! Every one of them starts from two networks that use the same
//! addresses. One network proves nothing about the property that
//! matters here, and a suite that grew up single-network would be a
//! suite that passes on a store with a cross-network leak in it.

use std::net::Ipv4Addr;

use rusqlite::{Connection, ErrorCode};

use crate::cursor::Cursor;
use crate::error::Error;
use crate::history::{Event, EventKind, History, Retention};
use crate::identity::Epoch;
use crate::model::{Binding, Lease};
use crate::selector::{ClientIdentity, Selector, SelectorKind};
use crate::store::BindingStore;
use crate::test_support::{
    ALPHA, ALPHA_NAMESPACE, BETA, BETA_NAMESPACE, FIXTURE_NOW, IFNAME, POOL, POOL_FIRST, POOL_LAST,
    TwoNetworks,
};

/// An address inside both networks' pools, written once.
fn in_pool(last_octet: u8) -> Ipv4Addr {
    Ipv4Addr::new(10, 0, 0, last_octet)
}

/// One value per selector kind, for a client that presents all five.
///
/// Held in precedence order, so that a test can walk it and say what
/// it expects next without restating the order anywhere.
const PRESENTED: [(SelectorKind, &str); 5] = [
    (SelectorKind::ClientId, "01:de:ad:be:ef"),
    (
        SelectorKind::DuidIaid,
        "00:03:00:01:52:54:00:aa:bb:cc/00:00:00:01",
    ),
    (SelectorKind::Mac, "52:54:00:aa:bb:cc"),
    (SelectorKind::UserClass, "tills"),
    (SelectorKind::VendorClass, "udhcp 1.0"),
];

/// The hardware address the two-network tests bind, written once
/// because the whole point of them is that it is the same one.
const SHARED_MAC: &str = "52:54:00:aa:bb:cc";

/// A selector, for a test that knows its value is well formed.
fn selector(kind: SelectorKind, value: &str) -> Selector {
    Selector::new(kind, value).unwrap()
}

/// A client presenting one attribute.
fn presenting(kind: SelectorKind, value: &str) -> ClientIdentity {
    ClientIdentity::new().with(kind, value).unwrap()
}

/// A client presenting all five.
fn presenting_everything() -> ClientIdentity {
    PRESENTED
        .into_iter()
        .try_fold(ClientIdentity::new(), |identity, (kind, value)| {
            identity.with(kind, value)
        })
        .unwrap()
}

/// Configure a binding and read it back as the store now holds it.
async fn configure(
    fixture: &TwoNetworks,
    network: &str,
    selector: &Selector,
    address: Ipv4Addr,
) -> Binding {
    fixture
        .store
        .create_binding(network, selector, address, None)
        .await
        .expect("a binding");

    let view = fixture.store.read().await.unwrap();
    view.binding(network, selector)
        .await
        .unwrap()
        .expect("the binding that was just configured")
}

/// Every event in the log, as `(seq, network, kind, resource_id,
/// state, recorded_at)`.
///
/// Read with the engine because no trait method reads the history
/// yet. What these tests use it for is the one thing that cannot be
/// checked from the outside: that a write and the event describing it
/// are the same transaction, so a write that was refused leaves no
/// event behind.
fn events(connection: &Connection) -> Vec<(i64, String, String, String, Option<String>, i64)> {
    let mut statement = connection
        .prepare("SELECT seq, network, kind, resource_id, state, recorded_at FROM event_log ORDER BY seq")
        .unwrap();

    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .unwrap();

    rows.map(|row| row.unwrap()).collect()
}

#[tokio::test]
async fn two_networks_hold_the_same_addresses_and_each_gets_its_own_answer() {
    let fixture = TwoNetworks::new("two-networks").await;
    let view = fixture.store.read().await.unwrap();

    let alpha = view.address_ranges(ALPHA).await.unwrap();
    let beta = view.address_ranges(BETA).await.unwrap();

    // Identical spans, identical names, and two different ranges.
    assert_eq!(alpha.len(), 1);
    assert_eq!(beta.len(), 1);
    assert_eq!(alpha[0].first, beta[0].first);
    assert_eq!(alpha[0].last, beta[0].last);
    assert_eq!(alpha[0].name, beta[0].name);

    // Each row says which network it came from, which is what lets a
    // layer above check that the answer it got is the answer it asked
    // for.
    assert_eq!(alpha[0].network, ALPHA);
    assert_eq!(beta[0].network, BETA);

    // And they are distinct rows, written at different positions.
    assert_ne!(alpha[0].cursor, beta[0].cursor);
}

#[tokio::test]
async fn every_write_returns_a_position_later_than_the_last() {
    let fixture = TwoNetworks::new("positions").await;

    let first = fixture.store.create_network("gamma").await.unwrap();
    let second = fixture
        .store
        .create_address_range("gamma", POOL, POOL_FIRST, POOL_LAST)
        .await
        .unwrap();

    assert_eq!(first.epoch(), fixture.store.epoch());
    assert_eq!(second.epoch(), fixture.store.epoch());
    assert!(
        second.sequence() > first.sequence(),
        "{second} should be after {first}"
    );
}

#[tokio::test]
async fn a_claim_on_an_interface_another_network_holds_is_refused_and_names_it() {
    let fixture = TwoNetworks::new("claim-conflict").await;

    // The second network tries to claim the interface the first has.
    let refusal = fixture
        .store
        .attach_interface(BETA, ALPHA_NAMESPACE, IFNAME)
        .await
        .unwrap_err();

    match &refusal {
        Error::Conflict { kind, id, held_by } => {
            assert_eq!(*kind, "attachment");
            assert_eq!(id, &format!("{ALPHA_NAMESPACE}/{IFNAME}"));
            assert!(held_by.contains(ALPHA), "{held_by}");
        }
        other => panic!("expected a refusal naming the holder, got {other:?}"),
    }

    // An operator reads the sentence rather than the variant, so the
    // sentence has to name both the interface and the network that
    // holds it.
    let message = refusal.to_string();
    assert!(message.contains(IFNAME), "{message}");
    assert!(message.contains(ALPHA), "{message}");
}

#[tokio::test]
async fn the_database_refuses_a_second_networks_claim_without_the_trait() {
    let fixture = TwoNetworks::new("claim-constraint").await;

    // Deliberately behind the interface. The refusal above is good
    // manners; this is the guarantee. An interface belonging to
    // exactly one network is the one validation where a cross-network
    // leak would be configured rather than coded, so it is a key in
    // the schema and not a check in a function -- and the only way to
    // show that is to write the conflicting row with no function in
    // the way.
    let connection = fixture.behind_the_store();
    let refused = connection.execute(
        "INSERT INTO attachment (namespace, ifname, network, cursor_seq)
         VALUES (?1, ?2, ?3, 1)",
        rusqlite::params![ALPHA_NAMESPACE, IFNAME, BETA],
    );

    match refused {
        Err(rusqlite::Error::SqliteFailure(failure, Some(message))) => {
            assert_eq!(failure.code, ErrorCode::ConstraintViolation);
            assert_eq!(
                message,
                "UNIQUE constraint failed: attachment.namespace, attachment.ifname"
            );
        }
        other => panic!("the database accepted a second claim on an interface: {other:?}"),
    }

    // And the first network still holds it.
    let owner = fixture
        .store
        .interface_owner(ALPHA_NAMESPACE, IFNAME)
        .await
        .unwrap()
        .expect("the interface is claimed");
    assert_eq!(owner.network, ALPHA);
}

#[tokio::test]
async fn the_same_interface_name_in_two_namespaces_belongs_to_two_networks() {
    let fixture = TwoNetworks::new("namespaces").await;

    // The fixture claimed `eth0` in both namespaces, for different
    // networks. Without the namespace in the identity the second
    // claim would have been the refusal above, and a consumer that
    // named its interfaces by role rather than by network would be
    // unable to configure two of them.
    let alpha = fixture
        .store
        .interface_owner(ALPHA_NAMESPACE, IFNAME)
        .await
        .unwrap()
        .expect("the first network's interface");
    let beta = fixture
        .store
        .interface_owner(BETA_NAMESPACE, IFNAME)
        .await
        .unwrap()
        .expect("the second network's interface");

    assert_eq!(alpha.network, ALPHA);
    assert_eq!(beta.network, BETA);
    assert_eq!(alpha.ifname, beta.ifname);
}

#[tokio::test]
async fn an_unclaimed_interface_has_no_owner() {
    let fixture = TwoNetworks::new("unclaimed").await;

    assert!(
        fixture
            .store
            .interface_owner(ALPHA_NAMESPACE, "eth9")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn one_networks_view_does_not_show_another_networks_interface() {
    let fixture = TwoNetworks::new("scoped-attachment").await;
    let view = fixture.store.read().await.unwrap();

    // The interface exists and is claimed, but not by this network,
    // so the answer scoped to this network is that there is nothing
    // here. The store-global question is the only one that can say
    // otherwise, and it says so by returning the network it found.
    assert!(
        view.attachment(ALPHA, BETA_NAMESPACE, IFNAME)
            .await
            .unwrap()
            .is_none()
    );

    let alphas = view.attachments(ALPHA).await.unwrap();
    assert_eq!(alphas.len(), 1);
    assert_eq!(alphas[0].namespace, ALPHA_NAMESPACE);
}

#[tokio::test]
async fn a_view_does_not_see_a_write_committed_after_it_opened() {
    let fixture = TwoNetworks::new("snapshot").await;

    let view = fixture.store.read().await.unwrap();
    let before = view.networks().await.unwrap();
    let position = view.cursor().clone();

    // A write commits while the view is held. This is the whole of
    // the second demand on this layer: a consumer takes a snapshot,
    // then resumes a stream at the snapshot's position, and the two
    // must not overlap or leave a gap. If the view could see this
    // write, the consumer would be delivered it twice.
    let written = fixture.store.create_network("gamma").await.unwrap();
    assert!(written.sequence() > position.sequence());

    let after = view.networks().await.unwrap();
    assert_eq!(before, after);
    assert!(view.network("gamma").await.unwrap().is_none());

    // The view's position has not moved either, which is what makes
    // it the position to resume from.
    assert_eq!(view.cursor(), &position);

    // A view opened now does see it, so the absence above is the
    // view's position and not a write that failed.
    let later = fixture.store.read().await.unwrap();
    assert!(later.network("gamma").await.unwrap().is_some());
    assert!(later.cursor().sequence() >= written.sequence());
}

#[tokio::test]
async fn a_view_answers_several_reads_at_one_position() {
    let fixture = TwoNetworks::new("consistent-reads").await;

    let view = fixture.store.read().await.unwrap();
    let networks_before = view.networks().await.unwrap();

    // Two writes in different networks, between two reads of the same
    // view. A collection of independent getters would answer the
    // first read from before them and the second from after, and
    // nothing in the result would say which.
    let range = fixture
        .store
        .create_address_range(ALPHA, "second", in_pool(200), in_pool(210))
        .await
        .unwrap();
    fixture.store.create_network("gamma").await.unwrap();

    assert_eq!(view.networks().await.unwrap(), networks_before);
    assert_eq!(view.address_ranges(ALPHA).await.unwrap().len(), 1);
    assert!(view.cursor().sequence() < range.sequence());
}

#[tokio::test]
async fn dropping_a_view_ends_the_thread_holding_it() {
    let fixture = TwoNetworks::new("view-lifetime").await;

    assert_eq!(fixture.store.live_views(), 0);

    let view = fixture.store.read().await.unwrap();
    assert_eq!(fixture.store.live_views(), 1);

    let second = fixture.store.read().await.unwrap();
    assert_eq!(fixture.store.live_views(), 2);

    drop(second);
    // Deterministic rather than eventually true: dropping a view
    // joins its thread, so by the time the drop has returned the
    // thread has gone. A store that left the thread running would
    // hold a read transaction open for the life of the process, and
    // the first anybody would know of it is a much later test waiting
    // on something unrelated.
    assert_eq!(fixture.store.live_views(), 1);

    drop(view);
    assert_eq!(fixture.store.live_views(), 0);
}

#[tokio::test]
async fn dropping_the_store_stops_its_writer_and_refuses_further_work() {
    let fixture = TwoNetworks::new("store-lifetime").await;
    let census = fixture.store.census_for_test();
    assert_eq!(census.writers(), 1);

    drop(fixture);

    // The store's drop joins the writer thread, so this is true as
    // soon as the drop has returned rather than soon afterwards.
    assert_eq!(census.writers(), 0);
    assert_eq!(census.views(), 0);
}

#[tokio::test]
async fn a_change_computed_from_a_stale_position_is_refused() {
    let fixture = TwoNetworks::new("compare-and-set").await;

    let view = fixture.store.read().await.unwrap();
    let original = view
        .address_range(ALPHA, POOL)
        .await
        .unwrap()
        .expect("the fixture's pool");
    drop(view);

    // One resize succeeds and moves the range's position.
    let moved = fixture
        .store
        .update_address_range(ALPHA, POOL, &original.cursor, in_pool(20), in_pool(90))
        .await
        .unwrap();
    assert!(moved.sequence() > original.cursor.sequence());

    // A second resize computed from the position that was read before
    // the first one is refused, naming both positions. Expressed as a
    // read and then a write, this is the sequence that would have
    // silently moved a pool over addresses something else was told it
    // owned.
    let refusal = fixture
        .store
        .update_address_range(ALPHA, POOL, &original.cursor, in_pool(30), in_pool(40))
        .await
        .unwrap_err();

    match &refusal {
        Error::Changed {
            kind,
            id,
            expected,
            found,
        } => {
            assert_eq!(*kind, "address range");
            assert_eq!(id, POOL);
            assert_eq!(expected, &original.cursor.to_string());
            assert_eq!(found, &moved.to_string());
        }
        other => panic!("expected a refusal naming both positions, got {other:?}"),
    }

    // The range is still where the successful resize left it.
    let view = fixture.store.read().await.unwrap();
    let current = view.address_range(ALPHA, POOL).await.unwrap().unwrap();
    assert_eq!(current.first, in_pool(20));
    assert_eq!(current.last, in_pool(90));
    assert_eq!(current.cursor, moved);
}

#[tokio::test]
async fn an_overlap_is_refused_within_a_network_and_ignored_between_them() {
    let fixture = TwoNetworks::new("range-overlap").await;

    // A span neither network's pool covers, given to one of them.
    fixture
        .store
        .create_address_range(ALPHA, "high", in_pool(200), in_pool(210))
        .await
        .expect("a second range in a network that has room for it");

    // Overlapping it in the same network is refused, because "the
    // lowest free address in this network" would then have two
    // answers, and the refusal names the range in the way.
    let refusal = fixture
        .store
        .create_address_range(ALPHA, "higher", in_pool(205), in_pool(215))
        .await
        .unwrap_err();

    match &refusal {
        Error::Conflict { kind, held_by, .. } => {
            assert_eq!(*kind, "address range");
            assert!(held_by.contains("high"), "{held_by}");
        }
        other => panic!("expected an overlap to be refused, got {other:?}"),
    }

    // The same span in the other network overlaps nothing that
    // network owns, and is accepted. The two networks' addresses are
    // written the same way and are not the same addresses -- which is
    // why the overlap check names the network in its statement rather
    // than asking about the address space.
    fixture
        .store
        .create_address_range(BETA, "higher", in_pool(205), in_pool(215))
        .await
        .expect("the other network's addresses are its own");

    // The fixture itself is the other half of this: both networks
    // already own a range called the same thing over exactly the same
    // addresses, which is not a conflict and never was.
    let view = fixture.store.read().await.unwrap();
    let alpha = view.address_range(ALPHA, POOL).await.unwrap().unwrap();
    let beta = view.address_range(BETA, POOL).await.unwrap().unwrap();
    assert_eq!((alpha.first, alpha.last), (beta.first, beta.last));
}

#[tokio::test]
async fn a_range_that_runs_backwards_is_refused() {
    let fixture = TwoNetworks::new("backwards-range").await;

    let refusal = fixture
        .store
        .create_address_range(ALPHA, "backwards", in_pool(200), in_pool(190))
        .await
        .unwrap_err();

    assert!(matches!(refusal, Error::Invalid { .. }), "{refusal:?}");
}

#[tokio::test]
async fn a_range_cannot_be_given_to_a_network_that_does_not_exist() {
    let fixture = TwoNetworks::new("unknown-network").await;

    let refusal = fixture
        .store
        .create_address_range("delta", POOL, POOL_FIRST, POOL_LAST)
        .await
        .unwrap_err();

    match &refusal {
        Error::UnknownNetwork { network } => assert_eq!(network, "delta"),
        other => panic!("expected the network to be named, got {other:?}"),
    }
}

#[tokio::test]
async fn a_network_with_anything_still_in_it_will_not_be_removed() {
    let fixture = TwoNetworks::new("delete-network").await;

    let view = fixture.store.read().await.unwrap();
    let network = view.network(ALPHA).await.unwrap().expect("the network");
    let range = view.address_range(ALPHA, POOL).await.unwrap().unwrap();
    let attachment = view
        .attachment(ALPHA, ALPHA_NAMESPACE, IFNAME)
        .await
        .unwrap()
        .unwrap();
    drop(view);

    let refusal = fixture
        .store
        .delete_network(ALPHA, &network.cursor)
        .await
        .unwrap_err();
    assert!(matches!(refusal, Error::Conflict { .. }), "{refusal:?}");

    // Emptied in the right order, it goes.
    fixture
        .store
        .detach_interface(ALPHA, ALPHA_NAMESPACE, IFNAME, &attachment.cursor)
        .await
        .unwrap();
    fixture
        .store
        .delete_address_range(ALPHA, POOL, &range.cursor)
        .await
        .unwrap();
    fixture
        .store
        .delete_network(ALPHA, &network.cursor)
        .await
        .unwrap();

    // And the other network is untouched, which is the assertion a
    // single-network test could not make.
    let view = fixture.store.read().await.unwrap();
    assert!(view.network(ALPHA).await.unwrap().is_none());
    assert!(view.network(BETA).await.unwrap().is_some());
    assert_eq!(view.address_ranges(BETA).await.unwrap().len(), 1);
    assert_eq!(view.attachments(BETA).await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_interface_another_network_holds_cannot_be_detached() {
    let fixture = TwoNetworks::new("detach-scope").await;

    let view = fixture.store.read().await.unwrap();
    let held = view
        .attachment(ALPHA, ALPHA_NAMESPACE, IFNAME)
        .await
        .unwrap()
        .unwrap();
    drop(view);

    // The second network presents the first network's interface, with
    // the correct position for it. Scoped to the network that asked,
    // there is nothing of that name to release.
    let refusal = fixture
        .store
        .detach_interface(BETA, ALPHA_NAMESPACE, IFNAME, &held.cursor)
        .await
        .unwrap_err();

    assert!(matches!(refusal, Error::NotFound { .. }), "{refusal:?}");
    assert_eq!(
        fixture
            .store
            .interface_owner(ALPHA_NAMESPACE, IFNAME)
            .await
            .unwrap()
            .unwrap()
            .network,
        ALPHA
    );
}

#[tokio::test]
async fn every_write_leaves_one_event_at_the_position_it_returned() {
    let fixture = TwoNetworks::new("events").await;

    let created = fixture.store.create_network("gamma").await.unwrap();
    let range = fixture
        .store
        .create_address_range("gamma", "pool-two", in_pool(150), in_pool(160))
        .await
        .unwrap();

    let connection = fixture.behind_the_store();
    let log = events(&connection);

    let network_event = log
        .iter()
        .find(|event| event.0 == i64::try_from(created.sequence()).unwrap())
        .expect("the network's write left an event at its own position");
    assert_eq!(network_event.1, "gamma");
    assert_eq!(network_event.2, "network");
    assert_eq!(network_event.3, "gamma");
    assert_eq!(network_event.5, FIXTURE_NOW);

    let range_event = log
        .iter()
        .find(|event| event.0 == i64::try_from(range.sequence()).unwrap())
        .expect("the range's write left an event at its own position");
    assert_eq!(range_event.1, "gamma");
    assert_eq!(range_event.2, "address-range");
    assert_eq!(range_event.3, "pool-two");

    // The state is the resource's whole post-state, built by the
    // engine so that a name containing a quote is quoted rather than
    // producing a payload that parses as something else.
    let first: i64 = connection
        .query_row(
            "SELECT json_extract(state, '$.first') FROM event_log WHERE seq = ?1",
            rusqlite::params![range_event.0],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(first, i64::from(u32::from(in_pool(150))));

    // Every event names its network, including when it was written
    // through a method that already did.
    assert!(log.iter().all(|event| !event.1.is_empty()));
}

#[tokio::test]
async fn a_deletion_is_an_event_with_no_state() {
    let fixture = TwoNetworks::new("deletion-events").await;

    let view = fixture.store.read().await.unwrap();
    let range = view.address_range(BETA, POOL).await.unwrap().unwrap();
    drop(view);

    let removed = fixture
        .store
        .delete_address_range(BETA, POOL, &range.cursor)
        .await
        .unwrap();

    let connection = fixture.behind_the_store();
    let state: Option<String> = connection
        .query_row(
            "SELECT state FROM event_log WHERE seq = ?1",
            rusqlite::params![i64::try_from(removed.sequence()).unwrap()],
            |row| row.get(0),
        )
        .unwrap();

    // Absence rather than a kind of its own, so that applying an
    // event is one rule and applying it twice does no harm.
    assert!(state.is_none());
}

#[tokio::test]
async fn a_refused_write_leaves_no_event() {
    let fixture = TwoNetworks::new("refused-events").await;

    let connection = fixture.behind_the_store();
    let before = events(&connection).len();

    // The event is appended inside the write's own transaction, so a
    // write the database refuses takes its event down with it. The
    // alternative -- an event appended first, or afterwards -- gives
    // a history that either reports changes that never happened or
    // misses ones that did, and this suite would not notice either
    // without this assertion.
    fixture
        .store
        .attach_interface(BETA, ALPHA_NAMESPACE, IFNAME)
        .await
        .unwrap_err();
    fixture
        .store
        .create_address_range(ALPHA, "overlapping", in_pool(50), in_pool(60))
        .await
        .unwrap_err();
    fixture.store.create_network(ALPHA).await.unwrap_err();

    assert_eq!(events(&connection).len(), before);
}

#[tokio::test]
async fn an_event_is_stamped_with_the_store_clock() {
    let fixture = TwoNetworks::new("clock").await;

    fixture.clock.advance(60);
    let created = fixture.store.create_network("gamma").await.unwrap();

    let connection = fixture.behind_the_store();
    let recorded: i64 = connection
        .query_row(
            "SELECT recorded_at FROM event_log WHERE seq = ?1",
            rusqlite::params![i64::try_from(created.sequence()).unwrap()],
            |row| row.get(0),
        )
        .unwrap();

    // The injected clock is the only clock: the column has no
    // default, so a second source of time would have to be written in
    // code rather than arriving silently from the engine.
    assert_eq!(recorded, FIXTURE_NOW + 60);
}

#[tokio::test]
async fn a_name_the_store_could_not_report_back_is_refused() {
    let fixture = TwoNetworks::new("names").await;

    assert!(matches!(
        fixture.store.create_network("").await,
        Err(Error::Invalid { .. })
    ));
    assert!(matches!(
        fixture
            .store
            .attach_interface(ALPHA, "ns/with-a-separator", "eth1")
            .await,
        Err(Error::Invalid { .. })
    ));
}

#[tokio::test]
async fn the_epoch_and_the_schema_version_describe_the_whole_store() {
    let fixture = TwoNetworks::new("identity").await;

    assert_eq!(
        fixture.store.schema_version(),
        crate::schema::SCHEMA_VERSION
    );

    // Every cursor the store issues is in its epoch, which is what a
    // consumer compares before trying to resume.
    let written = fixture.store.create_network("gamma").await.unwrap();
    assert_eq!(written.epoch(), fixture.store.epoch());

    let view = fixture.store.read().await.unwrap();
    assert_eq!(view.cursor().epoch(), fixture.store.epoch());
}

#[tokio::test]
async fn two_networks_with_the_same_client_bound_differently_each_answer_their_own() {
    let fixture = TwoNetworks::new("same-mac").await;
    let mac = selector(SelectorKind::Mac, SHARED_MAC);

    // One hardware address, configured in both networks, at
    // different addresses. This is the project's central property
    // expressed as a lookup: the same client is two unrelated clients
    // in two networks, legitimately, and an answer out of the wrong
    // one is the failure the whole store is built to prevent.
    configure(&fixture, ALPHA, &mac, in_pool(11)).await;
    configure(&fixture, BETA, &mac, in_pool(22)).await;

    let identity = presenting(SelectorKind::Mac, SHARED_MAC);
    let view = fixture.store.read().await.unwrap();

    let alpha = view
        .lookup_binding(ALPHA, &identity)
        .await
        .unwrap()
        .expect("the first network's binding");
    let beta = view
        .lookup_binding(BETA, &identity)
        .await
        .unwrap()
        .expect("the second network's binding");

    assert_eq!(alpha.address, in_pool(11));
    assert_eq!(beta.address, in_pool(22));

    // Each answer says which network it came from, which is what lets
    // a layer above check that the answer it got is the answer it
    // asked for.
    assert_eq!(alpha.network, ALPHA);
    assert_eq!(beta.network, BETA);

    // Both matched the same selector and are nonetheless two
    // different bindings, written at two different positions.
    assert_eq!(alpha.selector, mac);
    assert_eq!(beta.selector, mac);
    assert_ne!(alpha.cursor, beta.cursor);

    drop(view);
    assert_eq!(fixture.store.cross_network_answers(), 0);
}

#[tokio::test]
async fn the_same_client_arriving_on_each_networks_interface_is_answered_from_that_network() {
    let fixture = TwoNetworks::new("same-mac-interfaces").await;
    let mac = selector(SelectorKind::Mac, SHARED_MAC);

    configure(&fixture, ALPHA, &mac, in_pool(11)).await;
    configure(&fixture, BETA, &mac, in_pool(22)).await;

    // The fixture gives both networks an interface called `eth0`, in
    // a namespace of its own. Starting from the interface rather than
    // from the network is what the datapath does, so this is the same
    // two-network question asked the way a packet asks it.
    let identity = presenting(SelectorKind::Mac, SHARED_MAC);
    let view = fixture.store.read().await.unwrap();

    let alpha = view
        .explain(ALPHA_NAMESPACE, IFNAME, &identity)
        .await
        .unwrap();
    let beta = view
        .explain(BETA_NAMESPACE, IFNAME, &identity)
        .await
        .unwrap();

    assert_eq!(alpha.network.as_deref(), Some(ALPHA));
    assert_eq!(beta.network.as_deref(), Some(BETA));
    assert_eq!(alpha.matched.expect("a match").address, in_pool(11));
    assert_eq!(beta.matched.expect("a match").address, in_pool(22));

    // And asked through the form that names the network as well as
    // the interface, the same pair of answers.
    assert_eq!(
        view.lookup_binding_on_interface(ALPHA, ALPHA_NAMESPACE, IFNAME, &identity)
            .await
            .unwrap()
            .expect("a match")
            .address,
        in_pool(11)
    );
    assert_eq!(
        view.lookup_binding_on_interface(BETA, BETA_NAMESPACE, IFNAME, &identity)
            .await
            .unwrap()
            .expect("a match")
            .address,
        in_pool(22)
    );

    drop(view);
    assert_eq!(fixture.store.cross_network_answers(), 0);
}

#[tokio::test]
async fn the_highest_precedence_selector_a_client_presents_is_the_one_that_matches() {
    let fixture = TwoNetworks::new("precedence").await;

    // One binding per kind, all of them for the same client, each at
    // its own address so that the answer says which rule was applied
    // rather than only that something matched.
    let mut configured = Vec::new();
    for (index, (kind, value)) in PRESENTED.into_iter().enumerate() {
        let address = in_pool(11 + u8::try_from(index).unwrap());
        configured.push(configure(&fixture, ALPHA, &selector(kind, value), address).await);
    }

    let identity = presenting_everything();

    // Walk the published order from the top. The highest-precedence
    // binding still configured is the one that matches; removing it
    // brings the next into play, so one test walks the whole order
    // instead of asserting only its first element. A reordering of
    // the precedence fails here on the first kind it moved.
    for (expected, (kind, _)) in configured.iter().zip(PRESENTED) {
        let view = fixture.store.read().await.unwrap();
        let matched = view
            .lookup_binding(ALPHA, &identity)
            .await
            .unwrap()
            .expect("a client presenting five bound attributes matches one of them");

        assert_eq!(
            matched.selector.kind(),
            kind,
            "expected {kind} to win, and {} did",
            matched.selector.kind()
        );
        assert_eq!(&matched, expected);
        drop(view);

        fixture
            .store
            .delete_binding(ALPHA, &expected.selector, &expected.cursor)
            .await
            .unwrap();
    }

    // With none of them left the client matches nothing, which is a
    // complete answer and not a failure: a network may legitimately
    // have configured nothing for a client.
    let view = fixture.store.read().await.unwrap();
    assert!(
        view.lookup_binding(ALPHA, &identity)
            .await
            .unwrap()
            .is_none()
    );

    drop(view);
    assert_eq!(fixture.store.cross_network_answers(), 0);
}

#[tokio::test]
async fn a_binding_on_a_class_does_not_beat_one_on_the_client_itself() {
    let fixture = TwoNetworks::new("class-precedence").await;

    // The case the specification names: an operator who configured a
    // reservation against a hardware address and is told that a
    // vendor class matched has found a precedence problem, and would
    // rather find it here than in a guest.
    let mac = selector(SelectorKind::Mac, SHARED_MAC);
    let vendor = selector(SelectorKind::VendorClass, "udhcp 1.0");
    configure(&fixture, ALPHA, &mac, in_pool(11)).await;
    configure(&fixture, ALPHA, &vendor, in_pool(12)).await;

    let identity = presenting(SelectorKind::Mac, SHARED_MAC)
        .with(SelectorKind::VendorClass, "udhcp 1.0")
        .unwrap();

    let view = fixture.store.read().await.unwrap();
    let matched = view
        .lookup_binding(ALPHA, &identity)
        .await
        .unwrap()
        .expect("a match");

    assert_eq!(matched.selector, mac);
    assert_eq!(matched.address, in_pool(11));

    // And a client of the same model that is not the reserved one
    // falls to the class binding, which is what the broad bindings
    // are for.
    let other = presenting(SelectorKind::Mac, "52:54:00:ff:ff:ff")
        .with(SelectorKind::VendorClass, "udhcp 1.0")
        .unwrap();
    let matched = view
        .lookup_binding(ALPHA, &other)
        .await
        .unwrap()
        .expect("a match");
    assert_eq!(matched.selector, vendor);
}

#[tokio::test]
async fn an_identity_the_client_asserts_beats_the_hardware_it_happens_to_be_in() {
    let fixture = TwoNetworks::new("client-id-precedence").await;

    // The half of the order most likely to be argued with, so it has
    // a test of its own. A client identifier is set deliberately and
    // exists precisely so that a replaced network card keeps its
    // address; a hardware address is a property of a card, changes
    // with it, and is duplicated the moment a template is cloned.
    let client_id = selector(SelectorKind::ClientId, "01:de:ad:be:ef");
    let mac = selector(SelectorKind::Mac, SHARED_MAC);
    configure(&fixture, ALPHA, &client_id, in_pool(11)).await;
    configure(&fixture, ALPHA, &mac, in_pool(12)).await;

    let identity = presenting(SelectorKind::ClientId, "01:de:ad:be:ef")
        .with(SelectorKind::Mac, SHARED_MAC)
        .unwrap();

    let view = fixture.store.read().await.unwrap();
    let matched = view
        .lookup_binding(ALPHA, &identity)
        .await
        .unwrap()
        .expect("a match");

    assert_eq!(matched.selector, client_id);
    assert_eq!(matched.address, in_pool(11));
}

#[tokio::test]
async fn an_explanation_names_the_network_the_match_and_what_it_would_have_tried() {
    let fixture = TwoNetworks::new("explain").await;
    let mac = selector(SelectorKind::Mac, SHARED_MAC);
    configure(&fixture, ALPHA, &mac, in_pool(11)).await;

    let identity = presenting_everything();
    let view = fixture.store.read().await.unwrap();
    let explanation = view
        .explain(ALPHA_NAMESPACE, IFNAME, &identity)
        .await
        .unwrap();

    assert_eq!(explanation.namespace, ALPHA_NAMESPACE);
    assert_eq!(explanation.ifname, IFNAME);
    assert_eq!(explanation.network.as_deref(), Some(ALPHA));

    let matched = explanation.matched.expect("the hardware address matched");
    assert_eq!(matched.selector, mac);
    assert_eq!(matched.address, in_pool(11));

    // The selectors it would have consulted, in the order it would
    // have consulted them, whether or not each had a binding. This is
    // what turns "the hardware address matched" into "these two
    // outranked it and neither was configured", which is the only
    // complete check of an order this project invented.
    assert_eq!(explanation.considered, identity.selectors());
    assert_eq!(explanation.considered.len(), PRESENTED.len());
    assert_eq!(
        explanation.considered[0].kind(),
        SelectorKind::ClientId,
        "the explanation should report the order it would try, highest first"
    );

    // An explanation is an answer like any other and carries the
    // position it was computed at, so a consumer that is also
    // watching can order it against its stream.
    assert_eq!(&explanation.cursor, view.cursor());
}

#[tokio::test]
async fn an_explanation_for_an_interface_no_network_has_claimed_says_so() {
    let fixture = TwoNetworks::new("explain-unclaimed").await;

    // The fault that looks exactly like a binding that does not
    // match, and the reason the explanation reports the network it
    // resolved rather than only the match.
    let identity = presenting(SelectorKind::Mac, SHARED_MAC);
    let view = fixture.store.read().await.unwrap();
    let explanation = view
        .explain(ALPHA_NAMESPACE, "eth9", &identity)
        .await
        .unwrap();

    assert!(explanation.network.is_none());
    assert!(explanation.matched.is_none());

    // What the client presented is still reported: the attributes are
    // what they are whether or not there was a network to try them
    // in.
    assert_eq!(explanation.considered.len(), 1);
}

#[tokio::test]
async fn an_explanation_and_a_lookup_answer_from_one_code_path() {
    let fixture = TwoNetworks::new("explain-agrees").await;
    let mac = selector(SelectorKind::Mac, SHARED_MAC);
    let vendor = selector(SelectorKind::VendorClass, "udhcp 1.0");

    configure(&fixture, ALPHA, &mac, in_pool(11)).await;
    configure(&fixture, BETA, &vendor, in_pool(22)).await;

    // A diagnostic that reimplemented the matching could agree with
    // the documentation while the datapath did something else, which
    // would make it worse than having none. So: the same client, in
    // both networks, asked both ways.
    let identity = presenting_everything();
    let view = fixture.store.read().await.unwrap();

    for (network, namespace) in [(ALPHA, ALPHA_NAMESPACE), (BETA, BETA_NAMESPACE)] {
        let direct = view.lookup_binding(network, &identity).await.unwrap();
        let explained = view.explain(namespace, IFNAME, &identity).await.unwrap();

        assert_eq!(explained.network.as_deref(), Some(network));
        assert_eq!(explained.matched, direct, "in the network {network}");
    }
}

#[tokio::test]
async fn a_lookup_on_an_interface_another_network_holds_is_refused_and_counted() {
    let fixture = TwoNetworks::new("asserted-network").await;
    let mac = selector(SelectorKind::Mac, SHARED_MAC);
    configure(&fixture, ALPHA, &mac, in_pool(11)).await;
    configure(&fixture, BETA, &mac, in_pool(22)).await;
    assert_eq!(fixture.store.cross_network_answers(), 0);

    // The caller asserts that an interface belongs to its network and
    // it belongs to the other one. Answering from the asserted
    // network would serve one network's configuration to a client on
    // another's wire, and answering from the resolved one would serve
    // the caller an answer about a network it did not ask about, so
    // the only safe thing is to refuse.
    let identity = presenting(SelectorKind::Mac, SHARED_MAC);
    let view = fixture.store.read().await.unwrap();
    let refusal = view
        .lookup_binding_on_interface(ALPHA, BETA_NAMESPACE, IFNAME, &identity)
        .await
        .unwrap_err();

    match &refusal {
        Error::CrossNetwork {
            kind,
            id,
            asked,
            found,
        } => {
            assert_eq!(*kind, "attachment");
            assert_eq!(id, &format!("{BETA_NAMESPACE}/{IFNAME}"));
            assert_eq!(asked, ALPHA);
            assert_eq!(found, BETA);
        }
        other => panic!("expected a refusal naming both networks, got {other:?}"),
    }

    drop(view);
    assert_eq!(fixture.store.cross_network_answers(), 1);
}

#[tokio::test]
async fn an_interface_moved_behind_the_store_refuses_the_lookup_and_counts_it() {
    let fixture = TwoNetworks::new("cross-network-guard").await;
    let mac = selector(SelectorKind::Mac, SHARED_MAC);
    configure(&fixture, ALPHA, &mac, in_pool(11)).await;
    configure(&fixture, BETA, &mac, in_pool(22)).await;
    assert_eq!(fixture.store.cross_network_answers(), 0);

    // Deliberately behind the interface, because this is a state the
    // trait cannot produce. Through the trait an interface moves
    // networks by a detach and then an attach, each of which leaves
    // an event; here the row moves with nothing in the history saying
    // that it did -- a database that disagrees with its own history,
    // which is what a restore from the wrong backup, or a defect in a
    // later write path, would leave behind.
    let connection = fixture.behind_the_store();
    connection
        .execute(
            "UPDATE attachment SET network = ?1 WHERE namespace = ?2 AND ifname = ?3",
            rusqlite::params![BETA, ALPHA_NAMESPACE, IFNAME],
        )
        .unwrap();

    let identity = presenting(SelectorKind::Mac, SHARED_MAC);
    let view = fixture.store.read().await.unwrap();
    let refusal = view
        .lookup_binding_on_interface(ALPHA, ALPHA_NAMESPACE, IFNAME, &identity)
        .await
        .unwrap_err();

    match &refusal {
        Error::CrossNetwork {
            kind,
            id,
            asked,
            found,
        } => {
            assert_eq!(*kind, "attachment");
            assert_eq!(id, &format!("{ALPHA_NAMESPACE}/{IFNAME}"));
            assert_eq!(asked, ALPHA);
            assert_eq!(found, BETA);
        }
        other => panic!("expected a refusal naming both networks, got {other:?}"),
    }

    // An operator reads the sentence rather than the variant, and the
    // pair of networks is the whole of what makes it actionable.
    let message = refusal.to_string();
    assert!(message.contains(ALPHA), "{message}");
    assert!(message.contains(BETA), "{message}");

    // Refused rather than served, and counted. A mechanism that
    // noticed this and answered anyway would have turned the one
    // failure this project exists to prevent into a metric.
    drop(view);
    assert_eq!(fixture.store.cross_network_answers(), 1);
}

#[tokio::test]
async fn an_interface_no_network_holds_is_not_found_rather_than_counted() {
    let fixture = TwoNetworks::new("asserted-unclaimed").await;

    let identity = presenting(SelectorKind::Mac, SHARED_MAC);
    let view = fixture.store.read().await.unwrap();
    let refusal = view
        .lookup_binding_on_interface(ALPHA, ALPHA_NAMESPACE, "eth9", &identity)
        .await
        .unwrap_err();

    // Nothing crossed between networks, so this is a different fault
    // with a different fix, and the counter that should always be
    // zero stays zero.
    assert!(matches!(refusal, Error::NotFound { .. }), "{refusal:?}");
    drop(view);
    assert_eq!(fixture.store.cross_network_answers(), 0);
}

#[tokio::test]
async fn the_ordinary_work_of_the_store_never_counts_a_cross_network_answer() {
    let fixture = TwoNetworks::new("counter-zero").await;
    let mac = selector(SelectorKind::Mac, SHARED_MAC);
    let identity = presenting_everything();

    // Every surface this step added, over both networks, plus the
    // reads the steps before it added. The assertion at the end is
    // the one that matters: the counter is an alarm with no baseline
    // to interpret, which is only true if correct work never moves
    // it.
    for network in [ALPHA, BETA] {
        let binding = configure(&fixture, network, &mac, in_pool(11)).await;

        let view = fixture.store.read().await.unwrap();
        assert!(view.network(network).await.unwrap().is_some());
        assert_eq!(view.networks().await.unwrap().len(), 2);
        assert_eq!(view.attachments(network).await.unwrap().len(), 1);
        assert_eq!(view.address_ranges(network).await.unwrap().len(), 1);
        assert!(view.address_range(network, POOL).await.unwrap().is_some());
        assert_eq!(view.bindings(network).await.unwrap().len(), 1);
        assert!(view.binding(network, &mac).await.unwrap().is_some());
        assert!(
            view.lookup_binding(network, &identity)
                .await
                .unwrap()
                .is_some()
        );
        drop(view);

        let changed = fixture
            .store
            .update_binding(
                network,
                &mac,
                &binding.cursor,
                in_pool(12),
                Some("till-one"),
            )
            .await
            .unwrap();
        fixture
            .store
            .delete_binding(network, &mac, &changed)
            .await
            .unwrap();

        // The lease surfaces, over both networks, including the
        // address selection that reads several tables to choose one
        // address.
        let (_, leased) = fixture
            .store
            .allocate_lease(network, &identity, None, 86_400)
            .await
            .unwrap();
        fixture
            .store
            .renew_lease(network, leased.address, &identity, 86_400)
            .await
            .unwrap();

        let view = fixture.store.read().await.unwrap();
        assert_eq!(view.leases(network).await.unwrap().len(), 1);
        assert!(view.lease(network, leased.address).await.unwrap().is_some());
        assert_eq!(
            view.leases_held_by(network, &identity).await.unwrap().len(),
            1
        );
        let held = view
            .lease(network, leased.address)
            .await
            .unwrap()
            .expect("the lease");
        drop(view);

        fixture
            .store
            .revoke_lease(network, leased.address, &held.cursor)
            .await
            .unwrap();
        fixture.store.sweep_expired_leases(network).await.unwrap();
    }

    let view = fixture.store.read().await.unwrap();
    for (network, namespace) in [(ALPHA, ALPHA_NAMESPACE), (BETA, BETA_NAMESPACE)] {
        view.explain(namespace, IFNAME, &identity).await.unwrap();
        view.attachment(network, namespace, IFNAME).await.unwrap();
        assert!(
            view.lookup_binding_on_interface(network, namespace, IFNAME, &identity)
                .await
                .unwrap()
                .is_none()
        );
    }
    drop(view);

    assert_eq!(fixture.store.cross_network_answers(), 0);
}

#[tokio::test]
async fn a_second_binding_for_one_selector_is_refused_and_the_other_network_is_unaffected() {
    let fixture = TwoNetworks::new("binding-conflict").await;
    let mac = selector(SelectorKind::Mac, SHARED_MAC);
    configure(&fixture, ALPHA, &mac, in_pool(11)).await;

    // Two configured answers for one client in one network is a
    // question with two answers, so the operator has to say which
    // they meant.
    let refusal = fixture
        .store
        .create_binding(ALPHA, &mac, in_pool(12), None)
        .await
        .unwrap_err();

    match &refusal {
        Error::Conflict { kind, id, held_by } => {
            assert_eq!(*kind, "binding");
            assert_eq!(id, &format!("mac:{SHARED_MAC}"));
            assert!(held_by.contains(ALPHA), "{held_by}");
        }
        other => panic!("expected a refusal naming the network, got {other:?}"),
    }

    // The same selector in the other network is not a conflict and
    // never was.
    fixture
        .store
        .create_binding(BETA, &mac, in_pool(12), None)
        .await
        .expect("the other network's clients are its own");

    let view = fixture.store.read().await.unwrap();
    assert_eq!(view.bindings(ALPHA).await.unwrap().len(), 1);
    assert_eq!(view.bindings(BETA).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_binding_changed_from_a_stale_position_is_refused() {
    let fixture = TwoNetworks::new("binding-compare-and-set").await;
    let mac = selector(SelectorKind::Mac, SHARED_MAC);
    let original = configure(&fixture, ALPHA, &mac, in_pool(11)).await;

    let moved = fixture
        .store
        .update_binding(ALPHA, &mac, &original.cursor, in_pool(12), Some("till-one"))
        .await
        .unwrap();

    // A second change computed from the position read before the
    // first one is refused, naming both positions. As a read followed
    // by a write, this is the sequence that silently moves a client
    // off an address somebody else has been told about.
    let refusal = fixture
        .store
        .update_binding(ALPHA, &mac, &original.cursor, in_pool(13), None)
        .await
        .unwrap_err();
    assert!(matches!(refusal, Error::Changed { .. }), "{refusal:?}");

    let view = fixture.store.read().await.unwrap();
    let current = view.binding(ALPHA, &mac).await.unwrap().unwrap();
    assert_eq!(current.address, in_pool(12));
    assert_eq!(current.hostname.as_deref(), Some("till-one"));
    assert_eq!(current.cursor, moved);
}

#[tokio::test]
async fn a_network_with_a_binding_still_in_it_will_not_be_removed() {
    let fixture = TwoNetworks::new("binding-restrict").await;
    let mac = selector(SelectorKind::Mac, SHARED_MAC);
    let binding = configure(&fixture, ALPHA, &mac, in_pool(11)).await;

    let view = fixture.store.read().await.unwrap();
    let network = view.network(ALPHA).await.unwrap().unwrap();
    let range = view.address_range(ALPHA, POOL).await.unwrap().unwrap();
    let attachment = view
        .attachment(ALPHA, ALPHA_NAMESPACE, IFNAME)
        .await
        .unwrap()
        .unwrap();
    drop(view);

    fixture
        .store
        .detach_interface(ALPHA, ALPHA_NAMESPACE, IFNAME, &attachment.cursor)
        .await
        .unwrap();
    fixture
        .store
        .delete_address_range(ALPHA, POOL, &range.cursor)
        .await
        .unwrap();

    // Emptied of everything but its bindings, the network still
    // refuses to go: deleting it and discarding what an operator
    // configured is not a thing to do quietly.
    let refusal = fixture
        .store
        .delete_network(ALPHA, &network.cursor)
        .await
        .unwrap_err();
    match &refusal {
        Error::Conflict { held_by, .. } => assert!(held_by.contains("binding"), "{held_by}"),
        other => panic!("expected the bindings to be named, got {other:?}"),
    }

    fixture
        .store
        .delete_binding(ALPHA, &mac, &binding.cursor)
        .await
        .unwrap();
    fixture
        .store
        .delete_network(ALPHA, &network.cursor)
        .await
        .unwrap();

    // And the other network is untouched, bindings included.
    let view = fixture.store.read().await.unwrap();
    assert!(view.network(ALPHA).await.unwrap().is_none());
    assert!(view.network(BETA).await.unwrap().is_some());
}

#[tokio::test]
async fn configuring_a_binding_leaves_one_event_carrying_its_selector() {
    let fixture = TwoNetworks::new("binding-events").await;
    let mac = selector(SelectorKind::Mac, SHARED_MAC);

    let written = fixture
        .store
        .create_binding(ALPHA, &mac, in_pool(11), Some("till-one"))
        .await
        .unwrap();

    let connection = fixture.behind_the_store();
    let log = events(&connection);
    let event = log
        .iter()
        .find(|event| event.0 == i64::try_from(written.sequence()).unwrap())
        .expect("the binding's write left an event at its own position");

    assert_eq!(event.1, ALPHA);
    assert_eq!(event.2, "binding");

    // The resource's identity within its network is its selector, and
    // the kind side of it cannot contain a colon, so the first colon
    // in this string is always the separator -- however many the
    // value has.
    assert_eq!(event.3, format!("mac:{SHARED_MAC}"));
    assert_eq!(event.5, FIXTURE_NOW);

    let (kind, address, hostname): (String, i64, String) = connection
        .query_row(
            "SELECT json_extract(state, '$.selector_kind'),
                    json_extract(state, '$.address'),
                    json_extract(state, '$.hostname')
             FROM event_log WHERE seq = ?1",
            rusqlite::params![event.0],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(kind, "mac");
    assert_eq!(address, i64::from(u32::from(in_pool(11))));
    assert_eq!(hostname, "till-one");

    // And a removal is the same identity with no state at all.
    let view = fixture.store.read().await.unwrap();
    let binding = view.binding(ALPHA, &mac).await.unwrap().unwrap();
    drop(view);
    let removed = fixture
        .store
        .delete_binding(ALPHA, &mac, &binding.cursor)
        .await
        .unwrap();

    let state: Option<String> = connection
        .query_row(
            "SELECT state FROM event_log WHERE seq = ?1",
            rusqlite::params![i64::try_from(removed.sequence()).unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(state.is_none());
}

#[tokio::test]
async fn the_database_refuses_a_selector_kind_no_lookup_would_consult() {
    let fixture = TwoNetworks::new("selector-kinds").await;
    let connection = fixture.behind_the_store();

    // Deliberately behind the interface, because what is being
    // checked is the constraint rather than the code above it. A
    // misspelled kind would otherwise be a binding an operator
    // configured, that the store stored, and that no lookup would
    // ever consult -- a configured answer silently never given.
    for kind in SelectorKind::ALL {
        connection
            .execute(
                "INSERT INTO binding
                     (network, selector_kind, selector_value, address, hostname, cursor_seq)
                 VALUES (?1, ?2, 'whatever', 1, NULL, 1)",
                rusqlite::params![ALPHA, kind.as_str()],
            )
            .unwrap_or_else(|error| panic!("the table refused the kind {kind}: {error}"));
    }

    let refused = connection.execute(
        "INSERT INTO binding
             (network, selector_kind, selector_value, address, hostname, cursor_seq)
         VALUES (?1, 'hostname', 'whatever', 1, NULL, 1)",
        rusqlite::params![ALPHA],
    );

    match refused {
        Err(rusqlite::Error::SqliteFailure(failure, _)) => {
            assert_eq!(failure.code, ErrorCode::ConstraintViolation);
        }
        other => panic!("the database accepted a kind no lookup knows: {other:?}"),
    }
}

#[tokio::test]
async fn a_hardware_address_matches_whichever_way_round_it_was_written() {
    let fixture = TwoNetworks::new("mac-form").await;

    // The operator copies the address off a label in upper case; the
    // client presents it in lower, with the other separator. A store
    // comparing the text as given would have a reservation that is
    // configured, visible in every listing, and never matched.
    let configured = selector(SelectorKind::Mac, "52-54-00-AA-BB-CC");
    configure(&fixture, ALPHA, &configured, in_pool(11)).await;

    let identity = presenting(SelectorKind::Mac, "52:54:00:aa:bb:cc");
    let view = fixture.store.read().await.unwrap();
    let matched = view
        .lookup_binding(ALPHA, &identity)
        .await
        .unwrap()
        .expect("a match whichever form each side used");

    assert_eq!(matched.address, in_pool(11));
    assert_eq!(matched.selector.value(), SHARED_MAC);
}

#[tokio::test]
async fn a_client_presenting_nothing_matches_nothing() {
    let fixture = TwoNetworks::new("no-attributes").await;
    configure(
        &fixture,
        ALPHA,
        &selector(SelectorKind::Mac, SHARED_MAC),
        in_pool(11),
    )
    .await;

    // A real question rather than a degenerate one: this is how a
    // caller asks what an entirely unknown client would be answered,
    // and the answer is nothing rather than whatever happens to be
    // configured first.
    let view = fixture.store.read().await.unwrap();
    assert!(
        view.lookup_binding(ALPHA, &ClientIdentity::new())
            .await
            .unwrap()
            .is_none()
    );

    let explanation = view
        .explain(ALPHA_NAMESPACE, IFNAME, &ClientIdentity::new())
        .await
        .unwrap();
    assert_eq!(explanation.network.as_deref(), Some(ALPHA));
    assert!(explanation.matched.is_none());
    assert!(explanation.considered.is_empty());
}

// ---------------------------------------------------------------------
// Leases.
// ---------------------------------------------------------------------

/// A lease long enough that advancing the clock by less than it
/// plainly leaves the lease inside its term.
const DAY: i64 = 86_400;

/// Four clients that are not each other.
const MAC_ONE: &str = "52:54:00:00:00:01";
/// The second.
const MAC_TWO: &str = "52:54:00:00:00:02";
/// The third.
const MAC_THREE: &str = "52:54:00:00:00:03";
/// The fourth.
const MAC_FOUR: &str = "52:54:00:00:00:04";

/// A client identified by its hardware address alone, which is the
/// ordinary case and the one an operator writes reservations against.
fn client(mac: &str) -> ClientIdentity {
    presenting(SelectorKind::Mac, mac)
}

/// Resize both networks' pools, so that a test can enumerate what an
/// allocation has to choose from.
///
/// Both of them, always. A test that shrank one network's pool and
/// left the other alone would be a test whose two-network assertion
/// had quietly become a comparison of two different configurations.
async fn repool(fixture: &TwoNetworks, first: Ipv4Addr, last: Ipv4Addr) {
    for network in [ALPHA, BETA] {
        let view = fixture.store.read().await.unwrap();
        let range = view
            .address_range(network, POOL)
            .await
            .unwrap()
            .expect("the pool");
        drop(view);

        fixture
            .store
            .update_address_range(network, POOL, &range.cursor, first, last)
            .await
            .expect("a resized pool");
    }
}

/// Take a lease on a named address and return it.
async fn take(
    fixture: &TwoNetworks,
    network: &str,
    identity: &ClientIdentity,
    address: Ipv4Addr,
    duration: i64,
) -> Lease {
    let (at, lease) = fixture
        .store
        .acquire_lease(network, identity, address, duration)
        .await
        .expect("a lease");

    // The position the write was assigned and the position the lease
    // records are the same thing, which every caller of this helper
    // is then free to stop checking.
    assert_eq!(lease.cursor, at);

    lease
}

/// Allocate an address and return the lease.
async fn allocate(
    fixture: &TwoNetworks,
    network: &str,
    identity: &ClientIdentity,
    requested: Option<Ipv4Addr>,
) -> Lease {
    let (at, lease) = fixture
        .store
        .allocate_lease(network, identity, requested, DAY)
        .await
        .expect("an allocated address");

    assert_eq!(lease.cursor, at);

    lease
}

/// The lease the store holds for an address now, if it holds one.
async fn read_lease(fixture: &TwoNetworks, network: &str, address: Ipv4Addr) -> Option<Lease> {
    let view = fixture.store.read().await.unwrap();

    view.lease(network, address).await.unwrap()
}

/// Every lease event in the log, as `(seq, network, resource_id,
/// state)`.
fn lease_events(connection: &Connection) -> Vec<(i64, String, String, Option<String>)> {
    events(connection)
        .into_iter()
        .filter(|(_, _, kind, _, _, _)| kind == "lease")
        .map(|(seq, network, _, id, state, _)| (seq, network, id, state))
        .collect()
}

#[tokio::test]
async fn two_networks_lease_the_same_address_to_different_clients_at_once() {
    let fixture = TwoNetworks::new("lease-two-networks").await;
    let one = client(MAC_ONE);
    let two = client(MAC_TWO);

    // One address, two networks, two unrelated clients. This is the
    // project's central property expressed as a lease: a lease is
    // keyed by the network and the address together, so the same
    // address in two networks is two leases and never one.
    let alpha = take(&fixture, ALPHA, &one, in_pool(50), DAY).await;
    let beta = take(&fixture, BETA, &two, in_pool(50), DAY).await;

    assert_eq!(alpha.address, beta.address);
    assert_eq!(alpha.network, ALPHA);
    assert_eq!(beta.network, BETA);
    assert_ne!(alpha.lease_id, beta.lease_id);

    let view = fixture.store.read().await.unwrap();
    assert_eq!(
        view.lease(ALPHA, in_pool(50))
            .await
            .unwrap()
            .expect("the first network's lease")
            .holder,
        selector(SelectorKind::Mac, MAC_ONE)
    );
    assert_eq!(
        view.lease(BETA, in_pool(50))
            .await
            .unwrap()
            .expect("the second network's lease")
            .holder,
        selector(SelectorKind::Mac, MAC_TWO)
    );

    // And the query that answers the question by client is scoped the
    // same way: the first network's client holds nothing in the
    // second, even though the address it holds exists in both.
    assert_eq!(view.leases_held_by(ALPHA, &one).await.unwrap().len(), 1);
    assert!(view.leases_held_by(BETA, &one).await.unwrap().is_empty());
    drop(view);

    // Revoking one leaves the other where it was, which is the
    // assertion a single-network test cannot make.
    fixture
        .store
        .revoke_lease(ALPHA, in_pool(50), &alpha.cursor)
        .await
        .unwrap();

    assert!(read_lease(&fixture, ALPHA, in_pool(50)).await.is_none());
    assert_eq!(
        read_lease(&fixture, BETA, in_pool(50))
            .await
            .expect("the second network's lease")
            .lease_id,
        beta.lease_id
    );

    assert_eq!(fixture.store.cross_network_answers(), 0);
}

#[tokio::test]
async fn the_pool_offers_the_lowest_free_address_and_each_network_has_its_own() {
    let fixture = TwoNetworks::new("lease-lowest-free").await;
    repool(&fixture, in_pool(10), in_pool(12)).await;

    // The tie-break within a pool is this project's own: nothing
    // specifies which address a server offers, and the lowest free
    // one is chosen because it is deterministic and reproducible in a
    // test where a counter or a hash of the client's identity would
    // be neither.
    for network in [ALPHA, BETA] {
        for (mac, expected) in [
            (MAC_ONE, in_pool(10)),
            (MAC_TWO, in_pool(11)),
            (MAC_THREE, in_pool(12)),
        ] {
            let lease = allocate(&fixture, network, &client(mac), None).await;
            assert_eq!(lease.address, expected, "in the network {network}");
            assert_eq!(lease.network, network);
        }

        // Three addresses, three clients, and the fourth is told why
        // rather than being given somebody else's.
        let refusal = fixture
            .store
            .allocate_lease(network, &client(MAC_FOUR), None, DAY)
            .await
            .unwrap_err();

        match &refusal {
            Error::NoAddress {
                network: named,
                detail,
            } => {
                assert_eq!(named, network);
                assert!(detail.contains("leased or reserved"), "{detail}");
            }
            other => panic!("expected a refusal naming the network, got {other:?}"),
        }
    }

    // Each network handed out its own three, and the first network's
    // clients hold nothing in the second.
    let view = fixture.store.read().await.unwrap();
    assert_eq!(view.leases(ALPHA).await.unwrap().len(), 3);
    assert_eq!(view.leases(BETA).await.unwrap().len(), 3);
    drop(view);

    assert_eq!(fixture.store.cross_network_answers(), 0);
}

#[tokio::test]
async fn the_address_a_client_already_holds_is_the_first_thing_considered() {
    let fixture = TwoNetworks::new("lease-order-held").await;
    let one = client(MAC_ONE);

    // The first step of the specified order. The client holds .15,
    // the pool's lowest free address is .10, and the client even asks
    // for .11 -- and it keeps .15, with the lease id it already had,
    // because an address a client holds outranks both the pool and
    // what it asked for.
    let held = take(&fixture, ALPHA, &one, in_pool(15), DAY).await;

    let again = allocate(&fixture, ALPHA, &one, Some(in_pool(11))).await;

    assert_eq!(again.address, in_pool(15));
    assert_eq!(again.lease_id, held.lease_id);
    assert!(again.cursor != held.cursor);
    assert_eq!(again.acquired_at, held.acquired_at);
}

#[tokio::test]
async fn the_address_a_client_held_until_it_lapsed_comes_back_before_the_pool_does() {
    let fixture = TwoNetworks::new("lease-order-previous").await;
    let one = client(MAC_ONE);

    // The second step of the specified order, and the one this plan's
    // own summary of it left out. The client held .15; the lease has
    // lapsed; nothing has swept it; the pool's lowest free address is
    // .10. The specification says it gets .15 back.
    let first = take(&fixture, ALPHA, &one, in_pool(15), DAY).await;
    fixture.clock.advance(DAY + 1);

    let again = allocate(&fixture, ALPHA, &one, None).await;

    assert_eq!(again.address, in_pool(15));

    // A new lease id, because the address was free in the interim and
    // the holding that ended is not the holding that begins. A
    // consumer comparing ids is told that, where a kept id would have
    // claimed a continuity the store cannot vouch for.
    assert_ne!(again.lease_id, first.lease_id);
    assert_eq!(again.acquired_at, FIXTURE_NOW + DAY + 1);

    // And .10 really was free: somebody else gets it.
    let other = allocate(&fixture, ALPHA, &client(MAC_TWO), None).await;
    assert_eq!(other.address, in_pool(10));
}

#[tokio::test]
async fn an_address_a_binding_has_since_reserved_does_not_come_back_to_its_old_client() {
    let fixture = TwoNetworks::new("lease-previous-reserved").await;
    let one = client(MAC_ONE);

    // The client held .15 in both networks and let both lapse. While
    // it was away an operator reserved .15 in the first network for a
    // different machine, which is allowed precisely because nothing
    // holds the address any more.
    take(&fixture, ALPHA, &one, in_pool(15), DAY).await;
    take(&fixture, BETA, &one, in_pool(15), DAY).await;
    fixture.clock.advance(DAY + 1);
    configure(
        &fixture,
        ALPHA,
        &selector(SelectorKind::Mac, MAC_TWO),
        in_pool(15),
    )
    .await;

    // So the second step of the order no longer applies in the first
    // network: the address the client used to have is not one the
    // pool may hand out, and handing it back would be handing out
    // somebody else's reservation. The pool answers instead.
    assert_eq!(
        allocate(&fixture, ALPHA, &one, None).await.address,
        in_pool(10)
    );

    // Nothing was reserved in the second network, so there the same
    // client does get its old address back -- which is what makes the
    // assertion above about the reservation rather than about the
    // step.
    assert_eq!(
        allocate(&fixture, BETA, &one, None).await.address,
        in_pool(15)
    );
}

#[tokio::test]
async fn the_address_a_client_asks_for_is_given_only_if_nothing_holds_it() {
    let fixture = TwoNetworks::new("lease-order-requested").await;

    // The third step: .18 rather than the pool's .10, because the
    // client asked.
    let asked = allocate(&fixture, ALPHA, &client(MAC_ONE), Some(in_pool(18))).await;
    assert_eq!(asked.address, in_pool(18));

    // And the fourth step catches a client that asks for an address
    // somebody else holds. The request is considered at its place in
    // the order and no higher: asking is not a way past a lease.
    let refused = allocate(&fixture, ALPHA, &client(MAC_TWO), Some(in_pool(18))).await;
    assert_eq!(refused.address, in_pool(10));

    // An address inside no range is not valid to ask for either, and
    // falls through the same way.
    let outside = allocate(
        &fixture,
        ALPHA,
        &client(MAC_THREE),
        Some(Ipv4Addr::new(10, 0, 9, 9)),
    )
    .await;
    assert_eq!(outside.address, in_pool(11));
}

#[tokio::test]
async fn the_pool_does_not_offer_an_address_a_binding_reserves() {
    let fixture = TwoNetworks::new("lease-skips-bindings").await;
    repool(&fixture, in_pool(10), in_pool(12)).await;

    // Not in the specification, and this project's for a plain
    // reason: an address reserved for a machine that happens to be
    // switched off is an address the pool must not hand to somebody
    // else, or the reservation becomes a conflict the moment the
    // machine comes back.
    configure(
        &fixture,
        ALPHA,
        &selector(SelectorKind::Mac, MAC_THREE),
        in_pool(10),
    )
    .await;

    assert_eq!(
        allocate(&fixture, ALPHA, &client(MAC_ONE), None)
            .await
            .address,
        in_pool(11)
    );

    // The other network has no such reservation, so the same pool
    // starts where it always did -- which is the assertion that the
    // skip is scoped to the network that configured it.
    assert_eq!(
        allocate(&fixture, BETA, &client(MAC_ONE), None)
            .await
            .address,
        in_pool(10)
    );
}

#[tokio::test]
async fn a_network_with_no_range_allocates_nothing_and_says_which_network() {
    let fixture = TwoNetworks::new("lease-no-range").await;

    for network in [ALPHA, BETA] {
        let view = fixture.store.read().await.unwrap();
        let range = view.address_range(network, POOL).await.unwrap().unwrap();
        drop(view);

        fixture
            .store
            .delete_address_range(network, POOL, &range.cursor)
            .await
            .unwrap();
    }

    let refusal = fixture
        .store
        .allocate_lease(ALPHA, &client(MAC_ONE), None, DAY)
        .await
        .unwrap_err();

    match &refusal {
        Error::NoAddress { network, detail } => {
            assert_eq!(network, ALPHA);
            assert!(detail.contains("no address range"), "{detail}");
        }
        other => panic!("expected a refusal naming the network, got {other:?}"),
    }

    // A network with no range is not broken, which is the point of
    // the message saying which of the two causes it was: every
    // address in it is one somebody else chose, and asserting one is
    // how a client gets it.
    let asserted = take(&fixture, ALPHA, &client(MAC_ONE), in_pool(50), DAY).await;
    assert_eq!(asserted.address, in_pool(50));
}

#[tokio::test]
async fn an_address_is_held_up_to_its_instant_and_allocatable_from_it_without_a_sweep() {
    let fixture = TwoNetworks::new("lease-lazy-expiry").await;
    repool(&fixture, in_pool(50), in_pool(50)).await;

    // A pool of exactly one address, so that whether the second
    // client gets an address at all is the whole of whether the first
    // client's lease still holds it.
    let first = take(&fixture, ALPHA, &client(MAC_ONE), in_pool(50), DAY).await;
    assert_eq!(first.expires_at, FIXTURE_NOW + DAY);

    // One second before the instant: still held, and the pool has
    // nothing to offer.
    fixture.clock.advance(DAY - 1);
    assert!(
        !read_lease(&fixture, ALPHA, in_pool(50))
            .await
            .unwrap()
            .expired
    );
    assert!(
        fixture
            .store
            .allocate_lease(ALPHA, &client(MAC_TWO), None, DAY)
            .await
            .is_err()
    );

    // The instant itself: expired. Read, not swept -- the row is
    // still there, and nothing has run that could have removed it.
    fixture.clock.advance(1);
    let lapsed = read_lease(&fixture, ALPHA, in_pool(50))
        .await
        .expect("the row is still there, unswept");
    assert!(lapsed.expired);
    assert_eq!(lapsed.expires_at, FIXTURE_NOW + DAY);
    assert_eq!(lapsed.lease_id, first.lease_id);

    // And the history says so: one lease event, the acquisition, and
    // nothing announcing an expiry. This is the assertion that makes
    // the next one mean something.
    let behind = fixture.behind_the_store();
    assert_eq!(lease_events(&behind).len(), 1);
    drop(behind);

    // So the allocation below is not relying on a sweeper having run,
    // because no sweeper has run and the store has not been asked to
    // pretend one did.
    let second = allocate(&fixture, ALPHA, &client(MAC_TWO), None).await;
    assert_eq!(second.address, in_pool(50));
    assert_ne!(second.lease_id, first.lease_id);
    assert_eq!(
        second.holder,
        selector(SelectorKind::Mac, MAC_TWO),
        "the address went to the client that asked for it"
    );

    // The other network's identical pool was never touched by any of
    // this.
    assert!(read_lease(&fixture, BETA, in_pool(50)).await.is_none());
    assert_eq!(fixture.store.cross_network_answers(), 0);
}

#[tokio::test]
async fn a_renewal_keeps_the_lease_id() {
    let fixture = TwoNetworks::new("lease-renewal").await;
    let one = client(MAC_ONE);

    let first = take(&fixture, ALPHA, &one, in_pool(50), DAY).await;
    assert_eq!(
        first.lease_id, first.cursor,
        "a lease just acquired was acquired at the position it was written at"
    );

    fixture.clock.advance(10);
    let renewed = fixture
        .store
        .renew_lease(ALPHA, in_pool(50), &one, DAY)
        .await
        .unwrap();

    let after = read_lease(&fixture, ALPHA, in_pool(50))
        .await
        .expect("the renewed lease");

    // The holder has not changed, so neither has the lease id -- and
    // that is the whole of what a consumer reads a lease id for.
    assert_eq!(after.lease_id, first.lease_id);
    assert_eq!(after.acquired_at, first.acquired_at);

    // What did change: the term, and the position of the last write.
    assert_eq!(after.expires_at, FIXTURE_NOW + 10 + DAY);
    assert_eq!(after.cursor, renewed);
    assert_ne!(after.cursor, after.lease_id);
}

#[tokio::test]
async fn a_reassignment_changes_the_lease_id() {
    let fixture = TwoNetworks::new("lease-reassignment").await;

    let first = take(&fixture, ALPHA, &client(MAC_ONE), in_pool(50), DAY).await;

    fixture
        .store
        .revoke_lease(ALPHA, in_pool(50), &first.cursor)
        .await
        .unwrap();
    let second = take(&fixture, ALPHA, &client(MAC_TWO), in_pool(50), DAY).await;

    // A different client, so a different lease id, which is how a
    // consumer tells this from a renewal without comparing
    // timestamps.
    assert_ne!(second.lease_id, first.lease_id);
    assert!(second.lease_id.sequence() > first.lease_id.sequence());

    // The same client coming back after its own lease lapsed also
    // gets a new id, and that is deliberate rather than incidental:
    // the address was free in between, so the holding ended, and a
    // kept id would claim a continuity the store cannot vouch for.
    fixture.clock.advance(DAY + 1);
    let again = take(&fixture, ALPHA, &client(MAC_TWO), in_pool(50), DAY).await;
    assert_ne!(again.lease_id, second.lease_id);
}

#[tokio::test]
async fn an_address_another_client_holds_is_refused_and_names_the_holder() {
    let fixture = TwoNetworks::new("lease-conflict").await;

    let held = take(&fixture, ALPHA, &client(MAC_ONE), in_pool(50), DAY).await;

    let refusal = fixture
        .store
        .acquire_lease(ALPHA, &client(MAC_TWO), in_pool(50), DAY)
        .await
        .unwrap_err();

    match &refusal {
        Error::Conflict { kind, id, held_by } => {
            assert_eq!(*kind, "lease");
            assert_eq!(id, &in_pool(50).to_string());
            assert!(held_by.contains(MAC_ONE), "{held_by}");
            assert!(held_by.contains(&held.lease_id.to_string()), "{held_by}");
        }
        other => panic!("expected a refusal naming the holder, got {other:?}"),
    }

    // And nothing moved: the refusal is a refusal, not a race the
    // second client might win on a retry.
    assert_eq!(
        read_lease(&fixture, ALPHA, in_pool(50)).await.unwrap(),
        held
    );

    // The same address in the other network is nobody's, so the
    // second client has it there.
    take(&fixture, BETA, &client(MAC_TWO), in_pool(50), DAY).await;
}

#[tokio::test]
async fn a_lease_stays_its_clients_own_when_the_client_presents_more_than_it_did() {
    let fixture = TwoNetworks::new("lease-holder-grows").await;

    // Acquired presenting a hardware address alone, which is what
    // most clients do.
    let first = take(&fixture, ALPHA, &client(MAC_ONE), in_pool(50), DAY).await;
    assert_eq!(first.holder, selector(SelectorKind::Mac, MAC_ONE));

    // The same client, now also presenting a client identifier. It is
    // the same client, so its lease is still its own: the question is
    // whether it still presents the attribute the lease was recorded
    // against, not whether that attribute is still the
    // highest-precedence thing it has.
    let richer = client(MAC_ONE)
        .with(SelectorKind::ClientId, "01:de:ad:be:ef")
        .unwrap();
    fixture
        .store
        .renew_lease(ALPHA, in_pool(50), &richer, DAY)
        .await
        .expect("the same client renewing its own lease");
    assert_eq!(
        read_lease(&fixture, ALPHA, in_pool(50))
            .await
            .unwrap()
            .lease_id,
        first.lease_id
    );

    // A client presenting a different hardware address is a different
    // client and is refused.
    assert!(matches!(
        fixture
            .store
            .renew_lease(ALPHA, in_pool(50), &client(MAC_TWO), DAY)
            .await,
        Err(Error::Conflict { .. })
    ));
}

#[tokio::test]
async fn a_lease_is_recorded_against_the_highest_precedence_attribute_presented() {
    let fixture = TwoNetworks::new("lease-holder-precedence").await;
    let everything = presenting_everything();

    let lease = take(&fixture, ALPHA, &everything, in_pool(50), DAY).await;

    // The published order, applied to a lease rather than to a
    // lookup, so that a client is identified the same way whether it
    // is being matched against configuration or being given an
    // address. The top of that order is not this project's choice: a
    // client that supplies a client identifier must be identified by
    // it.
    assert_eq!(lease.holder.kind(), SelectorKind::ClientId);

    let view = fixture.store.read().await.unwrap();
    assert_eq!(
        view.leases_held_by(ALPHA, &everything).await.unwrap().len(),
        1
    );

    // The consequence, asserted rather than left to be discovered: a
    // client that stops presenting its identifier is not recognised
    // as the holder of this lease. That is the protocol's own
    // warning about keying on a hardware address, in reverse, and it
    // is the honest behaviour -- the store cannot know that two
    // identities are one machine.
    assert!(
        view.leases_held_by(ALPHA, &client(PRESENTED[2].1))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn what_a_client_holds_is_a_list_answered_inside_one_network() {
    let fixture = TwoNetworks::new("lease-held-by").await;
    let one = client(MAC_ONE);

    // A client holding two addresses at once is the case that makes
    // the client the wrong key for a lease and the address the right
    // one: during a renumber it legitimately has both.
    take(&fixture, ALPHA, &one, in_pool(51), DAY).await;
    take(&fixture, ALPHA, &one, in_pool(50), DAY).await;
    take(&fixture, BETA, &one, in_pool(50), DAY).await;

    let view = fixture.store.read().await.unwrap();

    let held = view.leases_held_by(ALPHA, &one).await.unwrap();
    assert_eq!(held.len(), 2);
    assert_eq!(held[0].address, in_pool(50));
    assert_eq!(held[1].address, in_pool(51));
    for lease in &held {
        assert_eq!(lease.network, ALPHA);
    }

    // The other network's lease on the same address, for the same
    // client, is a different lease and is not in this answer.
    let elsewhere = view.leases_held_by(BETA, &one).await.unwrap();
    assert_eq!(elsewhere.len(), 1);
    assert_ne!(elsewhere[0].lease_id, held[0].lease_id);

    // A client that holds nothing holds nothing, which is an answer
    // rather than a failure.
    assert!(
        view.leases_held_by(ALPHA, &client(MAC_TWO))
            .await
            .unwrap()
            .is_empty()
    );
    drop(view);

    assert_eq!(fixture.store.cross_network_answers(), 0);
}

#[tokio::test]
async fn a_client_presenting_nothing_cannot_hold_a_lease() {
    let fixture = TwoNetworks::new("lease-anonymous").await;

    // A lookup for an identity presenting nothing is a legitimate
    // question -- it asks what a completely unknown client would be
    // answered. A lease is not: it would be held by nobody, and no
    // query could ever find it again.
    let refusal = fixture
        .store
        .allocate_lease(ALPHA, &ClientIdentity::new(), None, DAY)
        .await
        .unwrap_err();

    assert!(matches!(refusal, Error::Invalid { .. }), "{refusal:?}");
}

#[tokio::test]
async fn a_lease_that_would_expire_as_it_was_taken_is_refused() {
    let fixture = TwoNetworks::new("lease-zero-duration").await;

    for duration in [0, -1] {
        let refusal = fixture
            .store
            .acquire_lease(ALPHA, &client(MAC_ONE), in_pool(50), duration)
            .await
            .unwrap_err();
        assert!(matches!(refusal, Error::Invalid { .. }), "{refusal:?}");
    }

    assert!(read_lease(&fixture, ALPHA, in_pool(50)).await.is_none());
}

#[tokio::test]
async fn a_renewal_of_a_lapsed_lease_is_refused_rather_than_quietly_reacquiring() {
    let fixture = TwoNetworks::new("lease-renew-lapsed").await;
    let one = client(MAC_ONE);

    let first = take(&fixture, ALPHA, &one, in_pool(50), DAY).await;
    fixture.clock.advance(DAY);

    let refusal = fixture
        .store
        .renew_lease(ALPHA, in_pool(50), &one, DAY)
        .await
        .unwrap_err();

    match &refusal {
        Error::NotFound { kind, id } => {
            assert_eq!(*kind, "lease");
            assert!(id.contains("a live lease"), "{id}");
        }
        other => panic!("expected a refusal that there is no live lease, got {other:?}"),
    }

    // The row is untouched, so the refusal cost nothing and the
    // client's own address is still the one it used to have -- which
    // is what the next allocation gives it back.
    let unchanged = read_lease(&fixture, ALPHA, in_pool(50)).await.unwrap();
    assert_eq!(unchanged.expires_at, first.expires_at);
    assert_eq!(unchanged.cursor, first.cursor);

    let again = allocate(&fixture, ALPHA, &one, None).await;
    assert_eq!(again.address, in_pool(50));
    assert_ne!(again.lease_id, first.lease_id);
}

#[tokio::test]
async fn a_revocation_computed_from_a_stale_position_is_refused() {
    let fixture = TwoNetworks::new("lease-revoke-stale").await;
    let one = client(MAC_ONE);

    let first = take(&fixture, ALPHA, &one, in_pool(50), DAY).await;
    fixture.clock.advance(10);
    fixture
        .store
        .renew_lease(ALPHA, in_pool(50), &one, DAY)
        .await
        .unwrap();

    // The operator read the lease, the client renewed it, and the
    // revocation was computed from a state the lease has left.
    let refusal = fixture
        .store
        .revoke_lease(ALPHA, in_pool(50), &first.cursor)
        .await
        .unwrap_err();
    assert!(matches!(refusal, Error::Changed { .. }), "{refusal:?}");

    assert!(read_lease(&fixture, ALPHA, in_pool(50)).await.is_some());
}

#[tokio::test]
async fn a_lapsed_lease_may_be_revoked_rather_than_waited_out() {
    let fixture = TwoNetworks::new("lease-revoke-lapsed").await;

    let first = take(&fixture, ALPHA, &client(MAC_ONE), in_pool(50), DAY).await;
    fixture.clock.advance(DAY + 1);

    // Nothing holds the address, but the row is still there. Removing
    // it deliberately is a thing a caller can do rather than
    // something only a sweep can.
    fixture
        .store
        .revoke_lease(ALPHA, in_pool(50), &first.cursor)
        .await
        .unwrap();

    assert!(read_lease(&fixture, ALPHA, in_pool(50)).await.is_none());

    // And the client no longer has an address to come back to, so the
    // pool answers instead.
    let again = allocate(&fixture, ALPHA, &client(MAC_ONE), None).await;
    assert_eq!(again.address, in_pool(10));
}

#[tokio::test]
async fn a_binding_against_a_leased_address_is_refused_and_names_the_lease() {
    let fixture = TwoNetworks::new("binding-over-lease").await;
    let held = take(&fixture, ALPHA, &client(MAC_ONE), in_pool(50), DAY).await;
    let two = selector(SelectorKind::Mac, MAC_TWO);

    // "Create this binding only if no lease holds the address",
    // which is a property of the write rather than a second method
    // beside it: there is no unchecked create for a caller to reach
    // for.
    let refusal = fixture
        .store
        .create_binding(ALPHA, &two, in_pool(50), None)
        .await
        .unwrap_err();

    match &refusal {
        Error::Conflict { kind, held_by, .. } => {
            assert_eq!(*kind, "binding");
            assert!(held_by.contains(MAC_ONE), "{held_by}");
            assert!(held_by.contains(&held.lease_id.to_string()), "{held_by}");
        }
        other => panic!("expected a conflict naming the lease, got {other:?}"),
    }

    // The same binding in the other network is not a conflict,
    // because the lease that stands in the way is the first
    // network's.
    let elsewhere = configure(&fixture, BETA, &two, in_pool(50)).await;
    assert_eq!(elsewhere.address, in_pool(50));

    // A binding moved onto a leased address is refused for the same
    // reason. The check is on where the binding is going, so moving
    // one away from a leased address is not refused.
    let moving = configure(&fixture, ALPHA, &two, in_pool(60)).await;
    assert!(matches!(
        fixture
            .store
            .update_binding(ALPHA, &two, &moving.cursor, in_pool(50), None)
            .await,
        Err(Error::Conflict { .. })
    ));

    // A lapsed lease does not stand in the way of either. Nothing
    // holds the address, and refusing on the strength of a row no
    // sweep has reached yet would make a binding's acceptance depend
    // on a sweeper having run.
    fixture.clock.advance(DAY + 1);
    fixture
        .store
        .update_binding(ALPHA, &two, &moving.cursor, in_pool(50), None)
        .await
        .expect("a lapsed lease is not in the way");
}

#[tokio::test]
async fn a_binding_may_revoke_the_lease_in_its_way_as_one_operation() {
    let fixture = TwoNetworks::new("binding-revoking-lease").await;
    let held = take(&fixture, ALPHA, &client(MAC_ONE), in_pool(50), DAY).await;
    take(&fixture, BETA, &client(MAC_ONE), in_pool(50), DAY).await;
    let two = selector(SelectorKind::Mac, MAC_TWO);

    let at = fixture
        .store
        .create_binding_revoking_lease(ALPHA, &two, in_pool(50), Some("till-one"), &held.lease_id)
        .await
        .expect("the binding, and the lease taken for it");

    // Both halves happened: the lease is gone and the binding is
    // there, at the position the write reported.
    assert!(read_lease(&fixture, ALPHA, in_pool(50)).await.is_none());
    let view = fixture.store.read().await.unwrap();
    let binding = view
        .binding(ALPHA, &two)
        .await
        .unwrap()
        .expect("the binding");
    assert_eq!(binding.address, in_pool(50));
    assert_eq!(binding.cursor, at);
    drop(view);

    // Two events, the revocation and then the binding, so the
    // position this write reported is the later of the two and a
    // consumer resuming from it has both.
    let behind = fixture.behind_the_store();
    let all = events(&behind);
    let last_two = &all[all.len() - 2..];
    assert_eq!(last_two[0].2, "lease");
    assert_eq!(
        last_two[0].4, None,
        "a revocation is an event with no state"
    );
    assert_eq!(last_two[1].2, "binding");
    assert_eq!(last_two[1].0, i64::try_from(at.sequence()).unwrap());
    drop(behind);

    // The other network's lease on the same address was not part of
    // this operation and is still held.
    assert!(read_lease(&fixture, BETA, in_pool(50)).await.is_some());
}

#[tokio::test]
async fn a_binding_that_cannot_be_written_leaves_the_lease_it_was_to_revoke_in_place() {
    let fixture = TwoNetworks::new("binding-revoking-rollback").await;
    let held = take(&fixture, ALPHA, &client(MAC_ONE), in_pool(50), DAY).await;
    let two = selector(SelectorKind::Mac, MAC_TWO);

    // The selector already has a binding, so the create below is
    // refused -- after the revocation in the same transaction has
    // already run.
    configure(&fixture, ALPHA, &two, in_pool(60)).await;

    let refusal = fixture
        .store
        .create_binding_revoking_lease(ALPHA, &two, in_pool(50), None, &held.lease_id)
        .await
        .unwrap_err();
    assert!(matches!(refusal, Error::Conflict { .. }), "{refusal:?}");

    // This is what makes it one operation rather than two. The
    // revocation rolled back with the binding that justified it, so
    // there is no state in which an address was taken from a client
    // and nothing was written in its place.
    assert_eq!(
        read_lease(&fixture, ALPHA, in_pool(50)).await.unwrap(),
        held
    );

    // And no event claims otherwise.
    let behind = fixture.behind_the_store();
    assert_eq!(lease_events(&behind).len(), 1);
}

#[tokio::test]
async fn a_binding_cannot_revoke_a_lease_that_has_changed_holder() {
    let fixture = TwoNetworks::new("binding-revoking-stale").await;

    let first = take(&fixture, ALPHA, &client(MAC_ONE), in_pool(50), DAY).await;
    fixture
        .store
        .revoke_lease(ALPHA, in_pool(50), &first.cursor)
        .await
        .unwrap();
    let second = take(&fixture, ALPHA, &client(MAC_TWO), in_pool(50), DAY).await;

    // The caller was told about one lease and is asking for that one
    // to be taken. A different client holds the address now, which is
    // exactly the change the lease id exists to report.
    let refusal = fixture
        .store
        .create_binding_revoking_lease(
            ALPHA,
            &selector(SelectorKind::Mac, MAC_THREE),
            in_pool(50),
            None,
            &first.lease_id,
        )
        .await
        .unwrap_err();
    assert!(matches!(refusal, Error::Changed { .. }), "{refusal:?}");

    assert_eq!(
        read_lease(&fixture, ALPHA, in_pool(50)).await.unwrap(),
        second
    );

    // A renewal in between does not invalidate the instruction, the
    // id being unchanged by one.
    fixture.clock.advance(10);
    fixture
        .store
        .renew_lease(ALPHA, in_pool(50), &client(MAC_TWO), DAY)
        .await
        .unwrap();
    fixture
        .store
        .create_binding_revoking_lease(
            ALPHA,
            &selector(SelectorKind::Mac, MAC_THREE),
            in_pool(50),
            None,
            &second.lease_id,
        )
        .await
        .expect("a renewal is not a change of holder");
}

#[tokio::test]
async fn a_sweep_announces_the_expiries_that_had_already_happened_and_nothing_else() {
    let fixture = TwoNetworks::new("lease-sweep").await;

    let lapsing = take(&fixture, ALPHA, &client(MAC_ONE), in_pool(50), DAY).await;
    take(&fixture, ALPHA, &client(MAC_TWO), in_pool(51), DAY * 3).await;
    take(&fixture, BETA, &client(MAC_ONE), in_pool(50), DAY).await;

    fixture.clock.advance(DAY + 1);

    let (at, swept) = fixture.store.sweep_expired_leases(ALPHA).await.unwrap();

    // Exactly the leases whose instant had passed, in the network
    // asked about. The second network's identical lease has lapsed
    // too and is not this sweep's business: the sweep is scoped like
    // every other operation here.
    assert_eq!(swept.len(), 1);
    assert_eq!(swept[0].address, in_pool(50));
    assert_eq!(swept[0].lease_id, lapsing.lease_id);
    assert!(swept[0].expired);

    assert!(read_lease(&fixture, ALPHA, in_pool(50)).await.is_none());
    assert!(read_lease(&fixture, ALPHA, in_pool(51)).await.is_some());
    assert!(read_lease(&fixture, BETA, in_pool(50)).await.is_some());

    // The point of the whole operation: an event saying the address
    // was given up, which a consumer watching the history would
    // otherwise have had to infer by comparing an instant against its
    // own clock.
    let behind = fixture.behind_the_store();
    let announced: Vec<_> = lease_events(&behind)
        .into_iter()
        .filter(|(_, network, _, state)| network == ALPHA && state.is_none())
        .collect();
    assert_eq!(announced.len(), 1);
    assert_eq!(announced[0].2, in_pool(50).to_string());
    assert_eq!(
        announced[0].0,
        i64::try_from(at.expect("a position").sequence()).unwrap()
    );
    drop(behind);

    // A second sweep has nothing to do, writes nothing, and reports
    // no position -- because a sweep that removed nothing has no
    // position to report and inventing one would be a lie.
    let (nothing, empty) = fixture.store.sweep_expired_leases(ALPHA).await.unwrap();
    assert!(nothing.is_none());
    assert!(empty.is_empty());
}

#[tokio::test]
async fn a_lease_is_committed_before_the_caller_is_told_about_it() {
    let fixture = TwoNetworks::new("lease-durability").await;

    let lease = allocate(&fixture, ALPHA, &client(MAC_ONE), None).await;

    // The caller has the lease. Another connection to the same
    // database can see only what has been committed, so finding the
    // lease through one is the assertion that the commit had already
    // happened when the value above was returned -- which is the
    // order the whole design turns on, because the value above is
    // what a front-end would acknowledge to a client.
    let second = fixture.second_store().await;
    let view = second.read().await.unwrap();
    let durable = view
        .lease(ALPHA, lease.address)
        .await
        .unwrap()
        .expect("the lease, through a connection that only sees commits");

    assert_eq!(durable, lease);
    drop(view);
    drop(second);
}

#[tokio::test]
async fn every_lease_write_leaves_one_event_carrying_its_whole_post_state() {
    let fixture = TwoNetworks::new("lease-events").await;
    let one = client(MAC_ONE);

    let lease = take(&fixture, ALPHA, &one, in_pool(50), DAY).await;

    let behind = fixture.behind_the_store();
    let written = lease_events(&behind);
    assert_eq!(written.len(), 1);

    let (seq, network, id, state) = &written[0];
    assert_eq!(network, ALPHA);
    assert_eq!(id, &in_pool(50).to_string());
    assert_eq!(*seq, i64::try_from(lease.cursor.sequence()).unwrap());

    // The whole post-state, including the lease id -- which for an
    // acquisition is this event's own position, and is in the payload
    // anyway because it is the field a consumer reads the state for.
    let state = state.as_ref().expect("a post-state");
    assert!(state.contains(&format!("\"lease_id\":{seq}")), "{state}");
    assert!(
        state.contains(&format!("\"address\":{}", u32::from(in_pool(50)))),
        "{state}"
    );
    assert!(state.contains("\"holder_kind\":\"mac\""), "{state}");
    assert!(
        state.contains(&format!("\"holder_value\":\"{MAC_ONE}\"")),
        "{state}"
    );
    assert!(
        state.contains(&format!("\"expires_at\":{}", FIXTURE_NOW + DAY)),
        "{state}"
    );
    drop(behind);

    // A renewal's event carries the id it kept, which is an earlier
    // position than its own.
    fixture.clock.advance(10);
    let renewed = fixture
        .store
        .renew_lease(ALPHA, in_pool(50), &one, DAY)
        .await
        .unwrap();

    let behind = fixture.behind_the_store();
    let written = lease_events(&behind);
    assert_eq!(written.len(), 2);
    assert_eq!(written[1].0, i64::try_from(renewed.sequence()).unwrap());
    let state = written[1].3.as_ref().expect("a post-state");
    assert!(
        state.contains(&format!("\"lease_id\":{}", lease.lease_id.sequence())),
        "{state}"
    );
}

#[tokio::test]
async fn a_network_with_a_lease_on_it_will_not_be_removed() {
    let fixture = TwoNetworks::new("lease-blocks-delete").await;
    let lease = take(&fixture, ALPHA, &client(MAC_ONE), in_pool(50), DAY).await;

    let view = fixture.store.read().await.unwrap();
    let network = view.network(ALPHA).await.unwrap().expect("the network");
    let range = view.address_range(ALPHA, POOL).await.unwrap().unwrap();
    let attachment = view
        .attachment(ALPHA, ALPHA_NAMESPACE, IFNAME)
        .await
        .unwrap()
        .unwrap();
    drop(view);

    fixture
        .store
        .detach_interface(ALPHA, ALPHA_NAMESPACE, IFNAME, &attachment.cursor)
        .await
        .unwrap();
    fixture
        .store
        .delete_address_range(ALPHA, POOL, &range.cursor)
        .await
        .unwrap();

    // A lease is a child like any other, and a network that still has
    // one refuses to be deleted rather than quietly discarding an
    // address somebody is using.
    let refusal = fixture
        .store
        .delete_network(ALPHA, &network.cursor)
        .await
        .unwrap_err();
    match &refusal {
        Error::Conflict { held_by, .. } => assert!(held_by.contains("lease"), "{held_by}"),
        other => panic!("expected a conflict naming what is in the way, got {other:?}"),
    }

    fixture
        .store
        .revoke_lease(ALPHA, in_pool(50), &lease.cursor)
        .await
        .unwrap();
    fixture
        .store
        .delete_network(ALPHA, &network.cursor)
        .await
        .unwrap();

    let view = fixture.store.read().await.unwrap();
    assert!(view.network(ALPHA).await.unwrap().is_none());
    assert!(view.network(BETA).await.unwrap().is_some());
}

// ---------------------------------------------------------------------
// The history, the bound on it, and the digest.
// ---------------------------------------------------------------------

/// Long enough that a test of the count bound is not quietly a test
/// of the age bound.
const A_YEAR: u64 = 365 * 86_400;

/// The position before everything, which is what a consumer that has
/// read the whole history from the beginning would have presented
/// before the first write.
fn start(fixture: &TwoNetworks) -> Cursor {
    Cursor::new(fixture.store.epoch().clone(), 0)
}

/// A history that could be resumed, or a failure naming what came
/// back instead.
fn resumed(history: History) -> (Vec<Event>, Cursor) {
    match history {
        History::Resumed { events, cursor } => (events, cursor),
        other => panic!("expected a resumable history, got {other:?}"),
    }
}

/// How many of these events describe one resource in one network.
///
/// The question every "exactly once" assertion here is really asking:
/// a consumer applies events by resource, so a write delivered twice
/// and a write delivered never are both counted here.
fn occurrences(events: &[Event], network: &str, resource_id: &str) -> usize {
    events
        .iter()
        .filter(|event| event.network == network && event.resource_id == resource_id)
        .count()
}

/// Every position still in the log, oldest first, read with the engine
/// because the bound is a property of the table rather than of any
/// answer the store gives.
fn retained(connection: &Connection) -> Vec<i64> {
    let mut statement = connection
        .prepare("SELECT seq FROM event_log ORDER BY seq")
        .unwrap();
    let rows = statement.query_map([], |row| row.get(0)).unwrap();

    rows.map(|row| row.unwrap()).collect()
}

/// How far the log says it has been truncated.
fn evicted_through(connection: &Connection) -> i64 {
    connection
        .query_row(
            "SELECT evicted_through FROM event_history WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[tokio::test]
async fn a_snapshot_is_a_consistent_view_and_a_resume_joins_it_exactly_once() {
    let fixture = TwoNetworks::new("snapshot-resume").await;
    let reserved = selector(SelectorKind::Mac, SHARED_MAC);
    let id = format!("mac:{SHARED_MAC}");

    let view = fixture.store.read().await.unwrap();
    let before = view.snapshot(ALPHA).await.unwrap();
    let at = before.cursor.clone();

    // The snapshot's position is the view's, which is what makes it
    // the position to resume at rather than a position near it.
    assert_eq!(view.cursor(), &at);
    assert_eq!(occurrences(&before.events, ALPHA, &id), 0);

    // A writer commits while the view is held.
    let written = fixture
        .store
        .create_binding(ALPHA, &reserved, in_pool(50), None)
        .await
        .unwrap();
    assert!(written.sequence() > at.sequence());

    // The same view, asked again after that commit, gives the same
    // snapshot. This is the half that cannot be got by walking the
    // tables as they change: a snapshot that moved would deliver the
    // write below twice, and one that was assembled read by read
    // could contain half of it.
    let after = view.snapshot(ALPHA).await.unwrap();
    assert_eq!(before, after);

    // And the view's own history after its own position is empty, for
    // the same reason: the position it reports is exactly where its
    // snapshot ends.
    let (nothing, reached) = resumed(view.history(ALPHA, &at).await.unwrap());
    assert!(nothing.is_empty(), "{nothing:?}");
    assert_eq!(reached, at);
    drop(view);

    // Resuming from the snapshot's position delivers the write, once.
    let later = fixture.store.read().await.unwrap();
    let (events, reached) = resumed(later.history(ALPHA, &at).await.unwrap());
    assert_eq!(occurrences(&events, ALPHA, &id), 1);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].cursor, written);
    assert_eq!(events[0].kind, EventKind::Binding);
    assert_eq!(events[0].network, ALPHA);
    assert!(events[0].state.is_some());

    // No gap either, which is the other half of the join: the event
    // delivered is the one the snapshot did not contain, and nothing
    // sits between the two positions.
    assert_eq!(events[0].cursor.sequence(), at.sequence() + 1);

    // And nothing is delivered twice: from where that read reached,
    // there is nothing more to have.
    let (again, _) = resumed(later.history(ALPHA, &reached).await.unwrap());
    assert!(again.is_empty(), "{again:?}");
}

#[tokio::test]
async fn a_snapshots_events_say_what_the_live_events_said() {
    let fixture = TwoNetworks::new("snapshot-states").await;

    // All five kinds, so that the comparison below covers every state
    // this store knows how to write.
    configure(
        &fixture,
        ALPHA,
        &selector(SelectorKind::Mac, SHARED_MAC),
        in_pool(60),
    )
    .await;
    take(&fixture, ALPHA, &client(MAC_ONE), in_pool(50), DAY).await;

    let view = fixture.store.read().await.unwrap();
    let snapshot = view.snapshot(ALPHA).await.unwrap();
    let (events, _) = resumed(view.history(ALPHA, &start(&fixture)).await.unwrap());

    // The state in a snapshot and the state in the event the same
    // write emitted are built by two statements in two modules, and a
    // consumer applies both as descriptions of one resource. A drift
    // between them would have it believing two different things about
    // the same binding, so the two are compared here rather than
    // being kept in step by hand.
    let mut kinds = Vec::new();
    for event in &snapshot.events {
        let live = events
            .iter()
            .rev()
            .find(|candidate| {
                candidate.kind == event.kind
                    && candidate.resource_id == event.resource_id
                    && candidate.network == event.network
            })
            .unwrap_or_else(|| panic!("no event for {:?} {}", event.kind, event.resource_id));

        assert_eq!(event.state, live.state, "{:?}", event.kind);
        assert_eq!(event.cursor, live.cursor, "{:?}", event.kind);

        // A snapshot says where a resource was last written and not
        // when, a resource row carrying the one and not the other.
        assert!(event.recorded_at.is_none());
        assert_eq!(live.recorded_at, Some(FIXTURE_NOW));

        if !kinds.contains(&event.kind) {
            kinds.push(event.kind);
        }
    }

    for kind in EventKind::ALL {
        assert!(kinds.contains(&kind), "no {kind:?} in the snapshot");
    }
}

#[tokio::test]
async fn a_resume_delivers_the_intermediate_states_and_not_only_the_outcome() {
    let fixture = TwoNetworks::new("intermediate-states").await;

    let view = fixture.store.read().await.unwrap();
    let away = view.cursor().clone();
    drop(view);

    // A client takes an address and gives it up again while the
    // consumer is away. Collapsing these two into nothing would be
    // cheaper and would leave a consumer that was holding the lease
    // never seeing it end.
    let lease = take(&fixture, ALPHA, &client(MAC_ONE), in_pool(50), DAY).await;
    fixture
        .store
        .revoke_lease(ALPHA, in_pool(50), &lease.cursor)
        .await
        .unwrap();

    let view = fixture.store.read().await.unwrap();
    let (events, _) = resumed(view.history(ALPHA, &away).await.unwrap());

    let address = in_pool(50).to_string();
    assert_eq!(occurrences(&events, ALPHA, &address), 2);
    assert_eq!(events.len(), 2);
    assert!(events[0].state.is_some(), "the lease was taken");
    assert!(events[1].state.is_none(), "and then given up");
    assert!(events[0].cursor.sequence() < events[1].cursor.sequence());

    // The store's current state says nothing about either, which is
    // why the history has to.
    assert!(view.lease(ALPHA, in_pool(50)).await.unwrap().is_none());
}

#[tokio::test]
async fn a_history_read_scoped_to_one_network_sees_only_its_own_events() {
    let fixture = TwoNetworks::new("scoped-history").await;
    let shared = selector(SelectorKind::Mac, SHARED_MAC);
    let id = format!("mac:{SHARED_MAC}");

    let view = fixture.store.read().await.unwrap();
    let at = view.cursor().clone();
    drop(view);

    // The same client, bound to the same address, in both networks:
    // the two events are identical but for the network they name, so
    // a read that leaked between networks could not be told from a
    // correct one by looking at a resource identity.
    let in_alpha = fixture
        .store
        .create_binding(ALPHA, &shared, in_pool(50), None)
        .await
        .unwrap();
    let in_beta = fixture
        .store
        .create_binding(BETA, &shared, in_pool(50), None)
        .await
        .unwrap();

    let view = fixture.store.read().await.unwrap();

    let (alpha, _) = resumed(view.history(ALPHA, &at).await.unwrap());
    assert_eq!(alpha.len(), 1);
    assert_eq!(alpha[0].cursor, in_alpha);
    assert_eq!(alpha[0].network, ALPHA);
    assert_eq!(occurrences(&alpha, BETA, &id), 0);

    let (beta, _) = resumed(view.history(BETA, &at).await.unwrap());
    assert_eq!(beta.len(), 1);
    assert_eq!(beta[0].cursor, in_beta);
    assert_eq!(beta[0].network, BETA);
    assert_eq!(occurrences(&beta, ALPHA, &id), 0);

    // Each event names its network even on a read that named one. A
    // handler that learned to infer it from the question is wrong the
    // day the question widens -- as it does here.
    let (both, _) = resumed(view.history_everywhere(&at).await.unwrap());
    assert_eq!(both.len(), 2);
    assert_eq!(both[0].network, ALPHA);
    assert_eq!(both[1].network, BETA);
    assert_eq!(both[0].resource_id, both[1].resource_id);
    assert!(both[0].cursor.sequence() < both[1].cursor.sequence());
    assert_eq!(fixture.store.cross_network_answers(), 0);
}

#[tokio::test]
async fn a_position_the_store_has_evicted_is_told_so_rather_than_handed_nothing() {
    // Two events of history, which the fixture's own six writes have
    // already pushed past. The bound is the mechanism the daemon runs
    // with a larger number.
    let fixture = TwoNetworks::with_retention("evicted", Retention::new(2, A_YEAR).unwrap()).await;
    let early = start(&fixture);

    let view = fixture.store.read().await.unwrap();
    let answer = view.history(ALPHA, &early).await.unwrap();

    match &answer {
        History::Evicted {
            presented,
            evicted_through,
        } => {
            assert_eq!(presented, &early);
            assert_eq!(
                evicted_through.sequence(),
                view.cursor().sequence() - 2,
                "the mark should name the last position that is gone"
            );
            assert_eq!(evicted_through.epoch(), fixture.store.epoch());
        }
        other => panic!("expected the position to be reported as evicted, got {other:?}"),
    }

    // The thing this answer is not. An empty resume is what a store
    // without a durable low-water mark would have to say here, and it
    // would tell a consumer that has missed six events that it is up
    // to date.
    assert!(!matches!(answer, History::Resumed { .. }));

    // A position inside the window still resumes, and so does the
    // mark itself: what a consumer at a position needs is the events
    // *after* it, so the oldest resumable position is the one the
    // eviction reached, not the one after it.
    let (events, _) = resumed(view.history(ALPHA, view.cursor()).await.unwrap());
    assert!(events.is_empty());

    let at_the_mark = Cursor::new(fixture.store.epoch().clone(), view.cursor().sequence() - 2);
    assert!(matches!(
        view.history(ALPHA, &at_the_mark).await.unwrap(),
        History::Resumed { .. }
    ));
}

#[tokio::test]
async fn a_position_from_another_epoch_is_told_the_epoch_changed() {
    let fixture = TwoNetworks::new("epoch-mismatch").await;

    let view = fixture.store.read().await.unwrap();

    // A position whose sequence number is perfectly plausible here
    // and belongs to a different store's order. Without the epoch
    // this would resume, and the consumer would be handed another
    // history's events under numbers it recognised.
    let elsewhere = Cursor::new(Epoch::for_test("00000000000000000000000000000000"), 1);

    match view.history(ALPHA, &elsewhere).await.unwrap() {
        History::EpochChanged { presented, store } => {
            assert_eq!(&presented, elsewhere.epoch());
            assert_eq!(&store, fixture.store.epoch());
        }
        other => panic!("expected the epoch to be reported as changed, got {other:?}"),
    }
}

#[tokio::test]
async fn a_position_the_store_never_issued_is_not_reported_as_evicted() {
    let fixture = TwoNetworks::new("never-issued").await;

    let view = fixture.store.read().await.unwrap();
    let ahead = Cursor::new(
        fixture.store.epoch().clone(),
        view.cursor().sequence() + 100,
    );

    // The epoch matches and the position is ahead of everything this
    // store has issued, which is what a restore from a file-level
    // backup looks like from in here: the epoch comes back with the
    // file and the sequence counter comes back wound to where the
    // backup was taken.
    //
    // Reporting it as eviction would say something was discarded when
    // nothing was ever there, and that distinction is the whole
    // reason the low-water mark is recorded rather than inferred.
    match view.history_everywhere(&ahead).await.unwrap() {
        History::NeverIssued { presented, latest } => {
            assert_eq!(presented, ahead);
            assert_eq!(&latest, view.cursor());
        }
        other => panic!("expected the position to be reported as never issued, got {other:?}"),
    }
}

#[tokio::test]
async fn the_history_is_bounded_by_a_count_of_events() {
    let fixture =
        TwoNetworks::with_retention("bound-count", Retention::new(3, A_YEAR).unwrap()).await;

    // The fixture writes six events and the age bound cannot bite for
    // a year, so what is left is the count's doing.
    let behind = fixture.behind_the_store();
    let kept = retained(&behind);
    assert_eq!(kept.len(), 3);

    let view = fixture.store.read().await.unwrap();
    let latest = i64::try_from(view.cursor().sequence()).unwrap();
    assert_eq!(kept, vec![latest - 2, latest - 1, latest]);
    assert_eq!(evicted_through(&behind), latest - 3);
    drop(view);

    // And the window moves with each write rather than being trimmed
    // when something remembers to: one more write, one more eviction.
    fixture.store.create_network("gamma").await.unwrap();

    let kept = retained(&behind);
    assert_eq!(kept.len(), 3);
    assert_eq!(kept[2], latest + 1);
    assert_eq!(evicted_through(&behind), latest - 2);
}

#[tokio::test]
async fn the_history_is_also_bounded_by_the_age_of_its_events() {
    let fixture =
        TwoNetworks::with_retention("bound-age", Retention::new(50_000, 60).unwrap()).await;

    let behind = fixture.behind_the_store();
    assert_eq!(retained(&behind).len(), 6);

    // Two minutes later, everything the fixture wrote is outside a
    // one-minute window -- and the count bound, which has room for
    // fifty thousand, is not what removes it.
    fixture.clock.advance(120);
    let written = fixture.store.create_network("gamma").await.unwrap();

    let kept = retained(&behind);
    assert_eq!(kept, vec![i64::try_from(written.sequence()).unwrap()]);
    assert_eq!(
        evicted_through(&behind),
        i64::try_from(written.sequence()).unwrap() - 1
    );

    // The write that did the evicting is still resumable from, which
    // it has to be: its position is what the caller was just handed.
    let view = fixture.store.read().await.unwrap();
    assert!(matches!(
        view.history_everywhere(&written).await.unwrap(),
        History::Resumed { .. }
    ));
}

#[tokio::test]
async fn a_refused_write_evicts_nothing() {
    let fixture =
        TwoNetworks::with_retention("bound-atomic", Retention::new(3, A_YEAR).unwrap()).await;

    let behind = fixture.behind_the_store();
    let kept = retained(&behind);
    let mark = evicted_through(&behind);

    // A write this tight a bound would certainly evict for, refused
    // after its event was appended. The truncation is in the write's
    // own transaction, so it goes back with it: a store that
    // truncated outside the write would have moved the mark for a
    // change that never happened, and told a consumer afterwards that
    // it could not resume from a position whose events are all still
    // there.
    let refusal = fixture.store.create_network(ALPHA).await.unwrap_err();
    assert!(matches!(refusal, Error::Conflict { .. }));

    assert_eq!(retained(&behind), kept);
    assert_eq!(evicted_through(&behind), mark);
}

#[tokio::test]
async fn the_digest_changes_in_its_own_network_and_not_in_the_other() {
    let fixture = TwoNetworks::new("digest").await;
    let reserved = selector(SelectorKind::Mac, SHARED_MAC);

    let view = fixture.store.read().await.unwrap();
    let alpha_before = view.digest(ALPHA).await.unwrap();
    let beta_before = view.digest(BETA).await.unwrap();
    let position_before = view.cursor().clone();
    drop(view);

    // A binding configured in one network.
    let written = fixture
        .store
        .create_binding(BETA, &reserved, in_pool(50), None)
        .await
        .unwrap();

    let view = fixture.store.read().await.unwrap();
    assert_ne!(view.digest(BETA).await.unwrap(), beta_before);

    // And the assertion the digest exists for. The store's position
    // moved, so a digest derived from it -- or from anything global
    // -- would have moved here too, and every consumer mirroring
    // every other network would resync for a change that cannot
    // affect it.
    assert!(written.sequence() > position_before.sequence());
    assert_eq!(view.digest(ALPHA).await.unwrap(), alpha_before);
    drop(view);

    // The other way round, so that neither network is the one that
    // happens to work.
    let in_alpha = fixture
        .store
        .create_binding(ALPHA, &reserved, in_pool(50), None)
        .await
        .unwrap();

    let view = fixture.store.read().await.unwrap();
    let alpha_now = view.digest(ALPHA).await.unwrap();
    let beta_now = view.digest(BETA).await.unwrap();
    assert_ne!(alpha_now, alpha_before);

    // A configuration taken back to where it was reads as where it
    // was, which is what a consumer comparing one value against the
    // one it remembers needs: removing the binding leaves the
    // network's configured set exactly as it was before it existed.
    fixture
        .store
        .delete_binding(ALPHA, &reserved, &in_alpha)
        .await
        .unwrap();

    let view = fixture.store.read().await.unwrap();
    assert_eq!(view.digest(ALPHA).await.unwrap(), alpha_before);
    assert_eq!(view.digest(BETA).await.unwrap(), beta_now);
}

#[tokio::test]
async fn the_digest_does_not_move_when_a_lease_does() {
    let fixture = TwoNetworks::new("digest-leases").await;

    let view = fixture.store.read().await.unwrap();
    let before = view.digest(ALPHA).await.unwrap();
    drop(view);

    let lease = take(&fixture, ALPHA, &client(MAC_ONE), in_pool(50), DAY).await;

    // A digest answers "do we still agree about the configuration".
    // A lease is not configuration -- it changes whenever a client
    // appears -- so a digest that followed leases would change
    // constantly and would answer "has anything happened", which is
    // what the watch stream is for.
    let view = fixture.store.read().await.unwrap();
    assert_eq!(view.digest(ALPHA).await.unwrap(), before);

    // The lease is in the snapshot all the same, so the digest's
    // silence about it is a choice rather than a resource the store
    // forgot.
    let snapshot = view.snapshot(ALPHA).await.unwrap();
    assert_eq!(
        occurrences(&snapshot.events, ALPHA, &in_pool(50).to_string()),
        1
    );
    drop(view);

    fixture
        .store
        .revoke_lease(ALPHA, in_pool(50), &lease.cursor)
        .await
        .unwrap();

    let view = fixture.store.read().await.unwrap();
    assert_eq!(view.digest(ALPHA).await.unwrap(), before);
}

#[tokio::test]
async fn a_network_the_store_does_not_hold_has_no_snapshot_and_no_digest() {
    let fixture = TwoNetworks::new("absent-network").await;
    let view = fixture.store.read().await.unwrap();

    // Not an empty snapshot and not a digest of nothing. A consumer's
    // next act on a complete snapshot is to delete whatever the store
    // did not mention, so "there is no such network" and "this
    // network is configured with nothing" must not share an answer.
    assert!(matches!(
        view.snapshot("gamma").await.unwrap_err(),
        Error::UnknownNetwork { .. }
    ));
    assert!(matches!(
        view.digest("gamma").await.unwrap_err(),
        Error::UnknownNetwork { .. }
    ));

    // And the two networks that do exist are unaffected by having
    // been asked about one that does not.
    assert!(view.snapshot(ALPHA).await.is_ok());
    assert!(view.digest(BETA).await.is_ok());
}
