//! What this layer decides, with two networks holding the same
//! address.
//!
//! Every test below that touches the store runs against *both* of the
//! fixture's networks, and the four that do not say so in their own
//! comment. That is not thoroughness for its own sake: two networks
//! legitimately using the same addresses is the property this project
//! exists for, the interface a packet arrived on is the only thing
//! that distinguishes them, and a single-network test cannot see a
//! lookup that forgot to name one. A cross-network defect here is a
//! silently wrong answer inside somebody's guest hours later.
//!
//! The messages are built by assembling a datagram field by field and
//! then *parsing* it, rather than by constructing a request. A test
//! that built the request directly would skip the layer that reads
//! `hlen`, the options and the client identifier -- which is exactly
//! the layer whose rendering of those bytes into selector values this
//! file has to check.

use std::net::Ipv4Addr;

use teneral_store::{BindingStore, SelectorKind};

use crate::decide::outcome::{Note, Outcome, Silence};
use crate::decide::test_support::{
    ALPHA, ALPHA_IF, BETA, BETA_IF, DEFAULT_LEASE, Lab, POOL_FIRST, SERVER_ADDRESS, UNNAMESPACED,
};
use crate::wire::test_support::{Datagram, option, options};
use crate::wire::{AnswerKind, Request, parse};

/// The nine options a client that asked for nothing was observed to
/// receive, in the order it received them.
const MEASURED_ORDER: [u8; 9] = [53, 54, 51, 58, 59, 1, 28, 3, 6];

/// The client every test below is about.
const CLIENT: [u8; 6] = [0x52, 0x54, 0x00, 0x11, 0x22, 0x33];

/// Its client identifier: a type byte and the same six octets, which
/// is the shape a client that sets one from its hardware address
/// sends.
const CLIENT_IDENTIFIER: [u8; 7] = [0x01, 0x52, 0x54, 0x00, 0x11, 0x22, 0x33];

/// Option 53's values, as a client sends them.
const DISCOVER: u8 = 1;
/// A REQUEST.
const REQUEST: u8 = 3;
/// A DECLINE.
const DECLINE: u8 = 4;
/// A RELEASE.
const RELEASE: u8 = 7;
/// An INFORM.
const INFORM: u8 = 8;

/// Option 53's values, as a server sends them.
const OFFER_TYPE: u8 = 2;
/// An ACK.
const ACK_TYPE: u8 = 5;
/// A NAK.
const NAK_TYPE: u8 = 6;

/// No leases, and no notes.
///
/// Written out because an empty `vec![]` has no element type of its
/// own for the comparison to infer.
fn nothing() -> Vec<Ipv4Addr> {
    Vec::new()
}

/// Nothing for the daemon to say.
fn no_notes() -> Vec<Note> {
    Vec::new()
}

/// A message of `kind` from `CLIENT`, carrying `extra` beyond its
/// message type.
fn message(kind: u8, extra: &[Vec<u8>]) -> Datagram {
    let mut datagram = Datagram::discover();
    datagram.chaddr = CLIENT.to_vec();
    datagram.hlen = 6;

    let mut area = vec![option(53, &[kind])];
    area.extend_from_slice(extra);
    datagram.options = options(&area);

    datagram
}

/// A datagram, parsed, as it would reach this layer from a socket.
fn parsed(datagram: &Datagram) -> Request {
    parse(&datagram.bytes()).expect("a fixture datagram parses")
}

/// A bare DISCOVER: the hardware address and nothing else.
fn discover() -> Request {
    parsed(&message(DISCOVER, &[]))
}

/// A REQUEST accepting an offer from `server` for `wanted`.
fn selecting(server: Ipv4Addr, wanted: Ipv4Addr) -> Request {
    parsed(&message(
        REQUEST,
        &[option(54, &server.octets()), option(50, &wanted.octets())],
    ))
}

/// A REQUEST renewing `held`, which the client carries in `ciaddr`
/// and which names no server.
///
/// `ciaddr` set is a client *stating* that it holds the address, which
/// is what separates this from the fixture below.
fn renewing(held: Ipv4Addr) -> Request {
    let mut datagram = message(REQUEST, &[]);
    datagram.client_address = held;

    parsed(&datagram)
}

/// A REQUEST from a client checking after a reboot that `remembered`
/// is still its own: no server, no `ciaddr`, and the address in option
/// 50.
///
/// Those are the fields RFC 2131 section 4.3.2 uses to tell this state
/// from a renewal, and the difference in what a refusal means is the
/// difference between a SHOULD and a MUST.
fn rebooting(remembered: Ipv4Addr) -> Request {
    parsed(&message(REQUEST, &[option(50, &remembered.octets())]))
}

/// A RELEASE of `held`, in `ciaddr` as the protocol puts it.
fn releasing(held: Ipv4Addr) -> Request {
    let mut datagram = message(RELEASE, &[]);
    datagram.client_address = held;

    parsed(&datagram)
}

/// A DECLINE of `refused`, in the requested address option as the
/// protocol puts it.
fn declining(refused: Ipv4Addr) -> Request {
    parsed(&message(DECLINE, &[option(50, &refused.octets())]))
}

/// An INFORM from a client that already holds `held`.
fn informing(held: Ipv4Addr) -> Request {
    let mut datagram = message(INFORM, &[]);
    datagram.client_address = held;

    parsed(&datagram)
}

#[tokio::test]
async fn a_discover_is_offered_each_networks_own_reserved_address() {
    // The round trip decision 15 turns on, for the hardware address,
    // and the DISCOVER row of the table at the same time. The binding
    // is configured with the value an *operator* would type -- in two
    // of the three written forms the store accepts, neither of them
    // the form the packet renders -- and the packet has to match it
    // anyway. A test that asserted the renderer against itself could
    // not see a divergence here, because it would be comparing this
    // crate's choice with this crate's choice.
    //
    // Both addresses are outside the pool, which is also the point:
    // an address a binding reserves is asserted rather than allocated,
    // so a reservation can hold an address no pool would ever offer.
    let lab = Lab::new("discover-reserved").await;
    lab.reserve(
        ALPHA,
        SelectorKind::Mac,
        "52-54-00-11-22-33",
        Ipv4Addr::new(10, 0, 0, 77),
    )
    .await;
    lab.reserve(
        BETA,
        SelectorKind::Mac,
        "525400112233",
        Ipv4Addr::new(10, 0, 0, 88),
    )
    .await;

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &discover())
        .await
        .expect("alpha answers");
    assert_eq!(decided.outcome, Outcome::Sent(AnswerKind::Offer));
    assert_eq!(decided.notes, no_notes());
    let offer = lab.sent().only();
    assert_eq!(offer.offered_address(), Ipv4Addr::new(10, 0, 0, 77));
    assert_eq!(offer.message_type(), OFFER_TYPE);
    assert_eq!(offer.ifname, ALPHA_IF);
    assert_eq!(offer.namespace, UNNAMESPACED);
    assert_eq!(
        offer.hardware_address,
        CLIENT.to_vec(),
        "the only address a client with no address can be reached at"
    );
    assert_eq!(
        offer.client_address,
        Ipv4Addr::new(10, 0, 0, 77),
        "and the address the reply is unicast to, which only works below IP"
    );
    assert!(
        !offer.broadcast,
        "the flag is carried as the client set it and is not permission to unicast"
    );
    lab.sent().clear();

    let decided = lab
        .decider
        .answer(&Lab::arrival(BETA_IF), &discover())
        .await
        .expect("beta answers");
    assert_eq!(decided.outcome, Outcome::Sent(AnswerKind::Offer));
    let offer = lab.sent().only();
    assert_eq!(
        offer.offered_address(),
        Ipv4Addr::new(10, 0, 0, 88),
        "the same client on the other interface gets the other network's answer"
    );

    // And each network recorded only its own lease.
    assert_eq!(lab.leased(ALPHA).await, vec![Ipv4Addr::new(10, 0, 0, 77)]);
    assert_eq!(lab.leased(BETA).await, vec![Ipv4Addr::new(10, 0, 0, 88)]);
}

#[tokio::test]
async fn a_client_identifier_an_operator_typed_matches_the_packet_that_rendered_it() {
    // The same round trip for the client identifier, which is the one
    // that matters most: the store normalises a hardware address and
    // explicitly does not normalise this, so the form is wholly this
    // front-end's choice and the store cannot check it. The value
    // configured below is the form the reference implementation's
    // lease file renders, so an operator migrating can paste what
    // they already have.
    //
    // Alpha also has a binding against the hardware address, and the
    // client identifier has to win it: that precedence is RFC 2131's
    // and not this project's invention.
    let lab = Lab::new("discover-client-id").await;
    lab.reserve(
        ALPHA,
        SelectorKind::ClientId,
        "01:52:54:00:11:22:33",
        Ipv4Addr::new(10, 0, 0, 66),
    )
    .await;
    lab.reserve(
        ALPHA,
        SelectorKind::Mac,
        "52:54:00:11:22:33",
        Ipv4Addr::new(10, 0, 0, 55),
    )
    .await;
    lab.reserve(
        BETA,
        SelectorKind::ClientId,
        "01:52:54:00:11:22:33",
        Ipv4Addr::new(10, 0, 0, 99),
    )
    .await;

    let identified = parsed(&message(DISCOVER, &[option(61, &CLIENT_IDENTIFIER)]));

    lab.decider
        .answer(&Lab::arrival(ALPHA_IF), &identified)
        .await
        .expect("alpha answers");
    assert_eq!(
        lab.sent().only().offered_address(),
        Ipv4Addr::new(10, 0, 0, 66),
        "the client identifier outranks the hardware address"
    );
    lab.sent().clear();

    lab.decider
        .answer(&Lab::arrival(BETA_IF), &identified)
        .await
        .expect("beta answers");
    assert_eq!(
        lab.sent().only().offered_address(),
        Ipv4Addr::new(10, 0, 0, 99)
    );
}

#[tokio::test]
async fn a_discover_with_no_reservation_is_offered_from_its_own_networks_pool() {
    // Both networks allocate from the same span, so the only way to
    // tell which pool was read is to make one of them answer
    // differently: in alpha the lowest address is already somebody
    // else's.
    let lab = Lab::new("discover-pool").await;
    lab.held_by_somebody_else(ALPHA, POOL_FIRST).await;

    lab.decider
        .answer(&Lab::arrival(ALPHA_IF), &discover())
        .await
        .expect("alpha answers");
    assert_eq!(
        lab.sent().only().offered_address(),
        Ipv4Addr::new(10, 0, 0, 11),
        "alpha's lowest free address, the first being taken there"
    );
    lab.sent().clear();

    lab.decider
        .answer(&Lab::arrival(BETA_IF), &discover())
        .await
        .expect("beta answers");
    assert_eq!(
        lab.sent().only().offered_address(),
        POOL_FIRST,
        "and beta's, where the same address is free"
    );
}

#[tokio::test]
async fn an_offer_carries_the_nine_measured_options_and_its_own_interfaces_address() {
    let lab = Lab::new("offer-options").await;

    for ifname in [ALPHA_IF, BETA_IF] {
        lab.sent().clear();
        lab.decider
            .answer(&Lab::arrival(ifname), &discover())
            .await
            .expect("an offer");

        let offer = lab.sent().only();
        assert_eq!(offer.option_codes(), MEASURED_ORDER.to_vec());
        assert_eq!(
            offer.option(54),
            Some(vec![10, 0, 0, 1]),
            "the server identifier is the arrival interface's address"
        );
        assert_eq!(offer.option(1), Some(vec![255, 255, 255, 0]));
    }
}

#[tokio::test]
async fn each_network_hands_out_its_own_lease_time() {
    // The one thing an answer carries that cannot be observed from an
    // interface, so the one thing the store holds for it -- on the
    // network, which is why two networks must be able to disagree
    // about it.
    let lab = Lab::new("lease-time").await;
    lab.set_lease_seconds(ALPHA, 600).await;

    lab.decider
        .answer(&Lab::arrival(ALPHA_IF), &discover())
        .await
        .expect("alpha answers");
    let offer = lab.sent().only();
    assert_eq!(offer.seconds(51), Some(600));
    assert_eq!(offer.seconds(58), Some(300), "half the lease");
    assert_eq!(offer.seconds(59), Some(525), "seven eighths of it");
    lab.sent().clear();

    lab.decider
        .answer(&Lab::arrival(BETA_IF), &discover())
        .await
        .expect("beta answers");
    let offer = lab.sent().only();
    assert_eq!(offer.seconds(51), Some(43200));
    assert_eq!(offer.seconds(58), Some(21600));
    assert_eq!(offer.seconds(59), Some(37800));
}

#[tokio::test]
async fn a_request_selecting_this_server_is_acknowledged_where_the_address_is_free_to_give() {
    // The REQUEST-selecting row, and with it the cross-network case
    // the brief names: the client holds this address in alpha, and in
    // beta the same address belongs to somebody else. A front-end
    // that asked "does this client hold this address" without naming
    // a network would acknowledge both.
    let lab = Lab::new("request-selecting").await;
    lab.decider
        .answer(&Lab::arrival(ALPHA_IF), &discover())
        .await
        .expect("an offer in alpha");
    let offered = lab.sent().only().offered_address();
    lab.sent().clear();
    lab.held_by_somebody_else(BETA, offered).await;

    let accepting = selecting(SERVER_ADDRESS, offered);

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &accepting)
        .await
        .expect("alpha answers");
    assert_eq!(
        decided.outcome,
        Outcome::Sent(AnswerKind::Acknowledgement),
        "the address alpha offered is acknowledged there"
    );
    let ack = lab.sent().only();
    assert_eq!(ack.message_type(), ACK_TYPE);
    assert_eq!(ack.offered_address(), offered);
    lab.sent().clear();

    let decided = lab
        .decider
        .answer(&Lab::arrival(BETA_IF), &accepting)
        .await
        .expect("beta answers");
    assert_eq!(
        decided.outcome,
        Outcome::Sent(AnswerKind::Refusal),
        "and refused in the network where another client holds it"
    );
    assert_eq!(lab.sent().only().message_type(), NAK_TYPE);
    assert!(
        matches!(
            decided.notes.first(),
            Some(Note::AddressRefused { network, address, .. })
                if network == BETA && *address == offered
        ),
        "the refusal names the network it was decided in: {:?}",
        decided.notes
    );
}

#[tokio::test]
async fn a_request_naming_another_server_is_not_answered_on_either_interface() {
    let lab = Lab::new("request-other-server").await;
    let elsewhere = Ipv4Addr::new(10, 0, 0, 9);
    let accepting = selecting(elsewhere, POOL_FIRST);

    for ifname in [ALPHA_IF, BETA_IF] {
        let decided = lab
            .decider
            .answer(&Lab::arrival(ifname), &accepting)
            .await
            .expect("a decision");

        assert_eq!(
            decided.outcome,
            Outcome::Silent(Silence::AnotherServerSelected { server: elsewhere })
        );
        assert_eq!(decided.notes, no_notes(), "and not a line about it");
    }

    assert_eq!(lab.sent().count(), 0, "nothing was sent at all");
    assert_eq!(lab.leased(ALPHA).await, nothing());
    assert_eq!(lab.leased(BETA).await, nothing());
}

#[tokio::test]
async fn a_request_for_an_address_outside_this_networks_ranges_is_refused() {
    // The pool is not advisory. `acquire_lease` asserts an address
    // without consulting a range, by design, so a front-end that
    // handed a client whatever it asked for would let any client in
    // the network take any address in it.
    let lab = Lab::new("request-outside-range").await;
    let outside_beta = Ipv4Addr::new(10, 0, 0, 30);
    lab.set_range(ALPHA, POOL_FIRST, outside_beta).await;

    let accepting = selecting(SERVER_ADDRESS, outside_beta);

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &accepting)
        .await
        .expect("alpha answers");
    assert_eq!(
        decided.outcome,
        Outcome::Sent(AnswerKind::Acknowledgement),
        "alpha's pool was widened to include it"
    );
    lab.sent().clear();

    let decided = lab
        .decider
        .answer(&Lab::arrival(BETA_IF), &accepting)
        .await
        .expect("beta answers");
    assert_eq!(
        decided.outcome,
        Outcome::Sent(AnswerKind::Refusal),
        "and beta's was not"
    );
    assert_eq!(lab.leased(BETA).await, nothing(), "and beta wrote nothing");
}

#[tokio::test]
async fn a_request_for_an_address_another_clients_binding_reserves_is_refused() {
    // An address held for a machine that is switched off must not go
    // to somebody else, and a client asking for it by name is the
    // path the store's own pool check does not cover.
    let lab = Lab::new("request-reserved-elsewhere").await;
    let reserved = Ipv4Addr::new(10, 0, 0, 15);
    lab.reserve(ALPHA, SelectorKind::Mac, "aa:bb:cc:dd:ee:ff", reserved)
        .await;

    let accepting = selecting(SERVER_ADDRESS, reserved);

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &accepting)
        .await
        .expect("alpha answers");
    assert_eq!(decided.outcome, Outcome::Sent(AnswerKind::Refusal));
    assert_eq!(lab.leased(ALPHA).await, nothing());
    lab.sent().clear();

    let decided = lab
        .decider
        .answer(&Lab::arrival(BETA_IF), &accepting)
        .await
        .expect("beta answers");
    assert_eq!(
        decided.outcome,
        Outcome::Sent(AnswerKind::Acknowledgement),
        "the reservation is alpha's, and beta has no opinion about that address"
    );
    assert_eq!(lab.leased(BETA).await, vec![reserved]);
}

#[tokio::test]
async fn a_renewal_with_ciaddr_set_is_acknowledged_where_the_lease_is_and_refused_where_it_is_not()
{
    // The renewing row, and the cross-network case for it: the same
    // client, the same address, and a lease in only one of the two
    // networks.
    let lab = Lab::new("renewal").await;
    lab.decider
        .answer(&Lab::arrival(BETA_IF), &discover())
        .await
        .expect("an offer in beta");
    let held = lab.sent().only().offered_address();
    lab.sent().clear();

    let decided = lab
        .decider
        .answer(&Lab::arrival(BETA_IF), &renewing(held))
        .await
        .expect("beta answers");
    assert_eq!(decided.outcome, Outcome::Sent(AnswerKind::Acknowledgement));
    assert_eq!(lab.sent().only().offered_address(), held);
    lab.sent().clear();

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &renewing(held))
        .await
        .expect("alpha answers");
    assert_eq!(
        decided.outcome,
        Outcome::Sent(AnswerKind::Refusal),
        "alpha has no lease on it, whatever beta holds"
    );
    assert_eq!(lab.sent().only().message_type(), NAK_TYPE);
    assert_eq!(lab.leased(ALPHA).await, nothing(), "and still none");
}

#[tokio::test]
async fn a_renewal_of_a_lapsed_lease_is_refused_and_a_live_one_elsewhere_is_not() {
    // Two networks, the same address, two lease times, one clock. The
    // store evaluates expiry against that clock on every read, so no
    // sweep has to have run for this to be the answer.
    let lab = Lab::new("renewal-lapsed").await;
    lab.set_lease_seconds(ALPHA, 60).await;

    for ifname in [ALPHA_IF, BETA_IF] {
        lab.sent().clear();
        lab.decider
            .answer(&Lab::arrival(ifname), &discover())
            .await
            .expect("an offer");
    }
    let held = lab.sent().only().offered_address();
    lab.sent().clear();

    lab.clock.advance(120);

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &renewing(held))
        .await
        .expect("alpha answers");
    assert_eq!(
        decided.outcome,
        Outcome::Sent(AnswerKind::Refusal),
        "alpha's minute has passed, and a lapsed lease is not renewed"
    );
    lab.sent().clear();

    let decided = lab
        .decider
        .answer(&Lab::arrival(BETA_IF), &renewing(held))
        .await
        .expect("beta answers");
    assert_eq!(
        decided.outcome,
        Outcome::Sent(AnswerKind::Acknowledgement),
        "beta's twelve hours have not"
    );
}

#[tokio::test]
async fn a_renewal_of_an_address_another_client_holds_is_refused() {
    let lab = Lab::new("renewal-another-holder").await;
    for network in [ALPHA, BETA] {
        lab.held_by_somebody_else(network, POOL_FIRST).await;
    }

    for ifname in [ALPHA_IF, BETA_IF] {
        lab.sent().clear();
        let decided = lab
            .decider
            .answer(&Lab::arrival(ifname), &renewing(POOL_FIRST))
            .await
            .expect("a decision");

        assert_eq!(decided.outcome, Outcome::Sent(AnswerKind::Refusal));
        assert_eq!(lab.sent().only().message_type(), NAK_TYPE);
    }
}

#[tokio::test]
async fn a_rebooting_client_no_network_has_a_record_of_is_not_answered() {
    // RFC 2131 section 4.3.2's MUST, and the reason it is a MUST is
    // about what else is on the wire: two servers on one bridge is
    // what a migration looks like, and a server that refused every
    // other server's clients as they rebooted would break the network
    // it had just been added to.
    //
    // Neither network has ever heard of this client, so neither may
    // say anything.
    let lab = Lab::new("reboot-no-record").await;

    for ifname in [ALPHA_IF, BETA_IF] {
        let decided = lab
            .decider
            .answer(&Lab::arrival(ifname), &rebooting(POOL_FIRST))
            .await
            .expect("a decision");

        assert_eq!(
            decided.outcome,
            Outcome::Silent(Silence::NoRecordOfThisClient)
        );
        assert_eq!(
            decided.notes,
            no_notes(),
            "and not a line either: the section permits a warning and a line per \
             reboot of every foreign client on the link is what buries a fault"
        );
    }

    assert_eq!(
        lab.sent().count(),
        0,
        "nothing was sent on either interface"
    );
}

#[tokio::test]
async fn a_rebooting_client_with_a_lapsed_lease_is_refused_there_and_not_answered_elsewhere() {
    // The other half of the division. A client whose lease here ran
    // out is a client this network knows, and what it believes about
    // its address is wrong -- so it is told. The network that never
    // saw it stays silent about the same address and the same client.
    let lab = Lab::new("reboot-lapsed").await;
    lab.set_lease_seconds(ALPHA, 60).await;
    lab.decider
        .answer(&Lab::arrival(ALPHA_IF), &discover())
        .await
        .expect("an offer in alpha");
    let remembered = lab.sent().only().offered_address();
    lab.sent().clear();

    lab.clock.advance(120);

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &rebooting(remembered))
        .await
        .expect("alpha answers");
    assert_eq!(
        decided.outcome,
        Outcome::Sent(AnswerKind::Refusal),
        "alpha has a record of this client, and it has lapsed"
    );
    assert_eq!(lab.sent().only().message_type(), NAK_TYPE);
    lab.sent().clear();

    let decided = lab
        .decider
        .answer(&Lab::arrival(BETA_IF), &rebooting(remembered))
        .await
        .expect("beta answers");
    assert_eq!(
        decided.outcome,
        Outcome::Silent(Silence::NoRecordOfThisClient),
        "beta has never seen this client, whatever alpha remembers"
    );
    assert_eq!(lab.sent().count(), 0);
}

#[tokio::test]
async fn a_rebooting_client_with_a_live_lease_elsewhere_is_not_answered_here() {
    // The cross-network form of the same MUST. A front-end that
    // looked for a record of the client without naming a network
    // would find beta's lease and refuse on alpha's interface, which
    // is the silently wrong answer in its noisiest form: a NAK sent
    // to somebody else's client.
    let lab = Lab::new("reboot-live-elsewhere").await;
    lab.decider
        .answer(&Lab::arrival(BETA_IF), &discover())
        .await
        .expect("an offer in beta");
    let remembered = lab.sent().only().offered_address();
    lab.sent().clear();

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &rebooting(remembered))
        .await
        .expect("alpha answers");
    assert_eq!(
        decided.outcome,
        Outcome::Silent(Silence::NoRecordOfThisClient)
    );
    assert_eq!(lab.sent().count(), 0);

    let decided = lab
        .decider
        .answer(&Lab::arrival(BETA_IF), &rebooting(remembered))
        .await
        .expect("beta answers");
    assert_eq!(
        decided.outcome,
        Outcome::Sent(AnswerKind::Acknowledgement),
        "and the network that gave it the address confirms it"
    );
    assert_eq!(lab.sent().only().offered_address(), remembered);
}

#[tokio::test]
async fn a_binding_is_a_record_of_a_client_that_has_never_held_an_address() {
    // The second half of what counts as a record. An operator who
    // configured a reservation has told this network about the
    // client, so a reboot naming the wrong address is answered --
    // which is the case that gets the client back to the address it
    // was configured for instead of leaving it to time out.
    let lab = Lab::new("reboot-binding").await;
    lab.reserve(
        ALPHA,
        SelectorKind::Mac,
        "52:54:00:11:22:33",
        Ipv4Addr::new(10, 0, 0, 77),
    )
    .await;

    let wrong = Ipv4Addr::new(10, 0, 0, 12);

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &rebooting(wrong))
        .await
        .expect("alpha answers");
    assert_eq!(
        decided.outcome,
        Outcome::Sent(AnswerKind::Refusal),
        "alpha has the client on record and does not agree about the address"
    );
    assert_eq!(lab.sent().only().message_type(), NAK_TYPE);
    lab.sent().clear();

    let decided = lab
        .decider
        .answer(&Lab::arrival(BETA_IF), &rebooting(wrong))
        .await
        .expect("beta answers");
    assert_eq!(
        decided.outcome,
        Outcome::Silent(Silence::NoRecordOfThisClient),
        "the reservation is alpha's, and beta knows nothing of this client"
    );
    assert_eq!(lab.sent().count(), 0);
}

#[tokio::test]
async fn a_release_in_one_network_leaves_the_other_networks_lease_alone() {
    // The property the brief names in as many words, and the reason a
    // lease is keyed by `(network, address)` rather than by an
    // address: the same client holds the same address in both
    // networks, legitimately, and gives up one of them.
    let lab = Lab::new("release").await;
    for ifname in [ALPHA_IF, BETA_IF] {
        lab.sent().clear();
        lab.decider
            .answer(&Lab::arrival(ifname), &discover())
            .await
            .expect("an offer");
    }
    let held = lab.sent().only().offered_address();
    lab.sent().clear();
    assert_eq!(lab.leased(ALPHA).await, vec![held]);
    assert_eq!(lab.leased(BETA).await, vec![held]);

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &releasing(held))
        .await
        .expect("alpha answers");

    assert_eq!(decided.outcome, Outcome::Revoked { address: held });
    assert_eq!(
        lab.sent().count(),
        0,
        "and nothing goes back: the protocol specifies no reply"
    );
    assert_eq!(lab.leased(ALPHA).await, nothing(), "alpha's lease is gone");
    assert_eq!(lab.leased(BETA).await, vec![held], "and beta's is not");
}

#[tokio::test]
async fn a_release_of_an_address_this_client_holds_only_elsewhere_revokes_nothing() {
    let lab = Lab::new("release-elsewhere").await;
    lab.decider
        .answer(&Lab::arrival(BETA_IF), &discover())
        .await
        .expect("an offer in beta");
    let held = lab.sent().only().offered_address();
    lab.sent().clear();

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &releasing(held))
        .await
        .expect("alpha answers");

    assert_eq!(
        decided.outcome,
        Outcome::Silent(Silence::NotThisClientsAddress { address: held })
    );
    assert!(
        decided.notes.iter().any(
            |note| matches!(note, Note::AddressNotHeldHere { network, address }
                if network == ALPHA && *address == held)
        ),
        "and the line names alpha: {:?}",
        decided.notes
    );
    assert_eq!(lab.leased(BETA).await, vec![held], "beta keeps its lease");
}

#[tokio::test]
async fn a_decline_revokes_the_lease_in_its_own_network_and_is_always_recorded() {
    // A decline says the address this server just handed out is in
    // use by something the store knows nothing about. This phase's
    // answer is to revoke and log, which returns the address to the
    // pool and may offer it again -- so the line is the only way an
    // operator finds out, and it is asserted here rather than left to
    // a reading of the code.
    let lab = Lab::new("decline").await;
    for ifname in [ALPHA_IF, BETA_IF] {
        lab.sent().clear();
        lab.decider
            .answer(&Lab::arrival(ifname), &discover())
            .await
            .expect("an offer");
    }
    let held = lab.sent().only().offered_address();
    lab.sent().clear();

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &declining(held))
        .await
        .expect("alpha answers");

    assert_eq!(decided.outcome, Outcome::Revoked { address: held });
    assert_eq!(lab.sent().count(), 0, "and no reply");
    assert_eq!(
        decided.notes,
        vec![Note::AddressDeclined {
            network: ALPHA.to_owned(),
            address: held,
        }]
    );
    assert_eq!(lab.leased(ALPHA).await, nothing());
    assert_eq!(lab.leased(BETA).await, vec![held], "beta is untouched");
}

#[tokio::test]
async fn an_inform_is_answered_with_configuration_and_writes_no_lease() {
    let lab = Lab::new("inform").await;
    let already = Ipv4Addr::new(10, 0, 0, 200);

    for ifname in [ALPHA_IF, BETA_IF] {
        lab.sent().clear();
        let decided = lab
            .decider
            .answer(&Lab::arrival(ifname), &informing(already))
            .await
            .expect("a decision");

        assert_eq!(decided.outcome, Outcome::Sent(AnswerKind::Acknowledgement));
        let ack = lab.sent().only();
        assert_eq!(ack.message_type(), ACK_TYPE);
        assert_eq!(
            ack.offered_address(),
            Ipv4Addr::UNSPECIFIED,
            "there is no address to give: the client has one"
        );
        assert_eq!(ack.option(51), None, "and so no lease time");
        assert_eq!(ack.option(58), None);
        assert_eq!(ack.option(59), None);
        assert_eq!(
            ack.option_codes(),
            vec![53, 54, 1, 28, 3, 6],
            "the derived configuration, and nothing about a lease"
        );
    }

    assert_eq!(
        lab.leased(ALPHA).await,
        nothing(),
        "and no lease is written"
    );
    assert_eq!(lab.leased(BETA).await, nothing());
}

#[tokio::test]
async fn a_relayed_request_is_dropped_with_one_line_for_each_relay() {
    // Answering a relay correctly means taking the subnet from
    // `giaddr` rather than from the arrival interface, which is a
    // model this front-end does not have; answering it incorrectly
    // means a lease in the wrong subnet. So it is dropped -- and the
    // line is what distinguishes that from a silence nobody chose.
    //
    // Once per relay rather than once per packet: a relay pointed at
    // this server sends every request its clients make.
    let lab = Lab::new("relayed").await;
    let relay = Ipv4Addr::new(10, 0, 0, 254);
    let other_relay = Ipv4Addr::new(10, 0, 0, 253);

    let mut datagram = message(DISCOVER, &[]);
    datagram.relay_address = relay;
    let relayed = parsed(&datagram);

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &relayed)
        .await
        .expect("a decision");
    assert_eq!(decided.outcome, Outcome::Silent(Silence::Relayed { relay }));
    assert_eq!(
        decided.notes,
        vec![Note::RelayedRequestDropped { relay }],
        "exactly one line, naming the relay"
    );

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &relayed)
        .await
        .expect("a decision");
    assert_eq!(decided.outcome, Outcome::Silent(Silence::Relayed { relay }));
    assert_eq!(
        decided.notes,
        no_notes(),
        "and not a second one for the same relay"
    );

    let decided = lab
        .decider
        .answer(&Lab::arrival(BETA_IF), &relayed)
        .await
        .expect("a decision");
    assert_eq!(
        decided.notes,
        vec![Note::RelayedRequestDropped { relay }],
        "the same relay on the other network's interface is its own fault to report"
    );

    datagram.relay_address = other_relay;
    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &parsed(&datagram))
        .await
        .expect("a decision");
    assert_eq!(
        decided.notes,
        vec![Note::RelayedRequestDropped { relay: other_relay }]
    );

    assert_eq!(lab.sent().count(), 0, "and nothing was ever answered");
    assert_eq!(lab.leased(ALPHA).await, nothing());
    assert_eq!(lab.leased(BETA).await, nothing());
}

#[tokio::test]
async fn an_interface_no_network_has_claimed_is_not_answered_and_is_named() {
    // One network's worth of property rather than two: the fault is
    // that *no* network claimed the interface, so there is no second
    // network for the answer to leak from. Both of the fixture's
    // networks exist and neither has this interface.
    let lab = Lab::new("unclaimed").await;

    let decided = lab
        .decider
        .answer(&Lab::arrival("br-nobodys"), &discover())
        .await
        .expect("a decision");

    assert_eq!(
        decided.outcome,
        Outcome::Silent(Silence::InterfaceNotClaimed)
    );
    assert_eq!(
        decided.notes,
        vec![Note::InterfaceNotClaimed {
            namespace: UNNAMESPACED.to_owned(),
            ifname: "br-nobodys".to_owned(),
        }],
        "and the line names the interface, which is what an operator attaches"
    );
    assert_eq!(lab.sent().count(), 0);
}

#[tokio::test]
async fn a_client_whose_hardware_address_is_not_six_octets_is_not_answered() {
    // One network's worth of property: the request never reaches the
    // store, so there is no network for it to reach the wrong one of.
    //
    // `hlen` may lawfully be anything from one to sixteen and the
    // store will hold only a six-octet hardware address, so such a
    // client presents nothing a selector can be -- and a lease is
    // recorded against a selector. It is a refusal with a line on it
    // rather than a guess, and it is a limit this front-end has.
    let lab = Lab::new("short-hardware-address").await;
    let mut datagram = message(DISCOVER, &[]);
    datagram.hlen = 4;
    datagram.chaddr = vec![0x52, 0x54, 0x00, 0x11];

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &parsed(&datagram))
        .await
        .expect("a decision");

    assert_eq!(
        decided.outcome,
        Outcome::Silent(Silence::ClientNotIdentifiable)
    );
    assert!(
        decided.notes.iter().any(|note| matches!(
            note,
            Note::AttributeNotPresented {
                kind: SelectorKind::Mac,
                ..
            }
        )),
        "and the line says which attribute was lost: {:?}",
        decided.notes
    );
    assert_eq!(lab.sent().count(), 0);
    assert_eq!(lab.leased(ALPHA).await, nothing());
}

#[tokio::test]
async fn a_vendor_class_that_is_not_text_costs_that_attribute_and_not_the_lease() {
    // The same ruling the parser makes about an option whose value
    // will not decode. What the client loses is a binding configured
    // against that class, which is why the fact is logged.
    let lab = Lab::new("vendor-class").await;
    let unprintable = parsed(&message(DISCOVER, &[option(60, &[0xff, 0xfe])]));

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &unprintable)
        .await
        .expect("a decision");

    assert_eq!(decided.outcome, Outcome::Sent(AnswerKind::Offer));
    assert!(
        decided.notes.iter().any(|note| matches!(
            note,
            Note::AttributeNotPresented {
                kind: SelectorKind::VendorClass,
                ..
            }
        )),
        "{:?}",
        decided.notes
    );
    assert_eq!(lab.leased(ALPHA).await, vec![POOL_FIRST]);
}

#[tokio::test]
async fn a_vendor_class_the_operator_typed_matches_the_packet_that_carried_it() {
    // The third of decision 15's forms, round tripped the same way:
    // the option's bytes as text, configured as text, matched against
    // the packet. Two networks, each with its own value for the same
    // class.
    let lab = Lab::new("vendor-class-match").await;
    lab.reserve(
        ALPHA,
        SelectorKind::VendorClass,
        "till-2000",
        Ipv4Addr::new(10, 0, 0, 44),
    )
    .await;
    lab.reserve(
        BETA,
        SelectorKind::VendorClass,
        "till-2000",
        Ipv4Addr::new(10, 0, 0, 45),
    )
    .await;

    let classed = parsed(&message(DISCOVER, &[option(60, b"till-2000")]));

    lab.decider
        .answer(&Lab::arrival(ALPHA_IF), &classed)
        .await
        .expect("alpha answers");
    assert_eq!(
        lab.sent().only().offered_address(),
        Ipv4Addr::new(10, 0, 0, 44)
    );
    lab.sent().clear();

    lab.decider
        .answer(&Lab::arrival(BETA_IF), &classed)
        .await
        .expect("beta answers");
    assert_eq!(
        lab.sent().only().offered_address(),
        Ipv4Addr::new(10, 0, 0, 45)
    );
}

#[tokio::test]
async fn an_option_the_parser_could_not_read_is_carried_into_the_log() {
    // One network's worth of property: the note comes from the
    // parser's record and not from anything the store was asked.
    //
    // The point of carrying it up is that "the client did not send
    // this" and "the client sent this and we could not read it" are
    // different diagnoses, and only one of them is a reason a binding
    // does not match.
    let lab = Lab::new("option-not-understood").await;
    let unreadable = parsed(&message(DISCOVER, &[option(12, &[0xff, 0xfe, 0xfd])]));

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &unreadable)
        .await
        .expect("a decision");

    assert_eq!(decided.outcome, Outcome::Sent(AnswerKind::Offer));
    assert_eq!(decided.notes, vec![Note::OptionNotUnderstood { code: 12 }]);
}

#[tokio::test]
async fn a_network_with_nothing_left_to_offer_says_nothing_and_says_why() {
    // One address in each network's pool, and somebody else has
    // alpha's. The two reasons a network can have nothing -- an empty
    // pool and no pool at all -- are told apart by the store, and the
    // line carries what it said.
    let lab = Lab::new("nothing-to-offer").await;
    for network in [ALPHA, BETA] {
        lab.set_range(network, POOL_FIRST, POOL_FIRST).await;
    }
    lab.held_by_somebody_else(ALPHA, POOL_FIRST).await;

    let decided = lab
        .decider
        .answer(&Lab::arrival(ALPHA_IF), &discover())
        .await
        .expect("alpha answers");
    assert_eq!(decided.outcome, Outcome::Silent(Silence::NothingToOffer));
    assert!(
        decided
            .notes
            .iter()
            .any(|note| matches!(note, Note::NothingToOffer { network, .. } if network == ALPHA)),
        "{:?}",
        decided.notes
    );
    assert_eq!(lab.sent().count(), 0);

    let decided = lab
        .decider
        .answer(&Lab::arrival(BETA_IF), &discover())
        .await
        .expect("beta answers");
    assert_eq!(
        decided.outcome,
        Outcome::Sent(AnswerKind::Offer),
        "beta's one address is free, whatever alpha's is"
    );
}

#[tokio::test]
async fn the_lease_a_discover_wrote_carries_the_front_ends_own_attribution() {
    // Every mutating store method takes the actor that made the
    // write, and this is the one place in the tree where that actor is
    // a lease the datapath recorded rather than an operator or a
    // token. One network's worth of property: the attribution is the
    // same in both, and asserting it twice would assert nothing twice.
    let lab = Lab::new("attribution").await;
    lab.decider
        .answer(&Lab::arrival(ALPHA_IF), &discover())
        .await
        .expect("an offer");

    let view = lab.store.read().await.expect("a view");
    let leases = view.leases(ALPHA).await.expect("alpha's leases");

    assert_eq!(leases.len(), 1);
    assert_eq!(leases[0].written_by, "daemon:dhcpv4");
    assert_eq!(
        leases[0].expires_at - leases[0].acquired_at,
        DEFAULT_LEASE,
        "for the network's configured lease time"
    );
}
