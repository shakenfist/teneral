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

use crate::error::Error;
use crate::model::Binding;
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
