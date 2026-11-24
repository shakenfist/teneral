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
use crate::store::BindingStore;
use crate::test_support::{
    ALPHA, ALPHA_NAMESPACE, BETA, BETA_NAMESPACE, FIXTURE_NOW, IFNAME, POOL, POOL_FIRST, POOL_LAST,
    TwoNetworks,
};

/// An address inside both networks' pools, written once.
fn in_pool(last_octet: u8) -> Ipv4Addr {
    Ipv4Addr::new(10, 0, 0, last_octet)
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
