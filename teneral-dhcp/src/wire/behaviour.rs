//! What this layer does with a datagram it accepts, and with the
//! parameters of an answer.
//!
//! The malformed inputs are next door, in `malformed`. These are the
//! other half: that a request is read as the client sent it, and that
//! an answer carries the nine options that were measured, in the
//! order they were measured in, with the two derived times at the
//! measured fractions.

use std::net::Ipv4Addr;

use dhcproto::v4::{DhcpOption, Message, OptionCode};
use dhcproto::{Decodable, Decoder};

use crate::wire::test_support::{
    CLIENT_MAC, DISCOVER, Datagram, INFORM, LEASE_SECONDS, OFFERED_ADDRESS, PREFIX_LENGTH, REQUEST,
    SERVER_ADDRESS, TRANSACTION, option, option_codes, option_value, options,
};
use crate::wire::{
    Answer, AnswerKind, Error, LeaseSeconds, Request, RequestKind, ServerFacts, parse,
};

/// The nine options a client that asked for nothing was observed to
/// receive, in the order it received them.
const MEASURED_ORDER: [u8; 9] = [53, 54, 51, 58, 59, 1, 28, 3, 6];

/// The interface a request arrived on, as every test here sees it.
fn server() -> ServerFacts {
    ServerFacts::new(SERVER_ADDRESS, PREFIX_LENGTH).expect("a /24 is a prefix")
}

/// The lease time every test here hands out.
fn lease() -> LeaseSeconds {
    LeaseSeconds::new(LEASE_SECONDS).expect("twelve hours is a lease")
}

/// A DISCOVER, parsed.
fn discover() -> Request {
    parse(&Datagram::discover().bytes()).expect("a DISCOVER parses")
}

#[test]
fn a_discover_parses_into_the_fields_the_client_presented() {
    let request = discover();

    assert_eq!(request.kind, RequestKind::Discover);
    assert_eq!(request.transaction, TRANSACTION);
    assert!(!request.broadcast);
    assert_eq!(request.hardware_type, 1);
    assert_eq!(request.hardware_address, CLIENT_MAC.to_vec());
    assert_eq!(request.client_address, Ipv4Addr::UNSPECIFIED);
    assert_eq!(request.relay_address, Ipv4Addr::UNSPECIFIED);
    assert_eq!(request.requested_address, None);
    assert_eq!(request.server_identifier, None);
    assert_eq!(
        request.client_identifier,
        Some(vec![0x01, 0x52, 0x54, 0x00, 0xaa, 0xbb, 0xcc])
    );
    assert_eq!(request.hostname, None);
    assert_eq!(
        request.not_understood,
        vec![],
        "a well-formed request drops nothing"
    );
}

#[test]
fn a_request_that_names_a_server_and_an_address_carries_both() {
    let mut datagram = Datagram::discover();
    datagram.options = options(&[
        option(53, &[REQUEST]),
        option(50, &OFFERED_ADDRESS.octets()),
        option(54, &SERVER_ADDRESS.octets()),
        option(12, b"till-3"),
    ]);

    let request = parse(&datagram.bytes()).expect("a REQUEST parses");

    assert_eq!(request.kind, RequestKind::Request);
    assert_eq!(request.requested_address, Some(OFFERED_ADDRESS));
    assert_eq!(request.server_identifier, Some(SERVER_ADDRESS));
    assert_eq!(request.hostname, Some("till-3".to_string()));
}

#[test]
fn a_relay_address_is_carried_rather_than_refused() {
    // The layer above drops a relayed request and logs the address it
    // came through, which it cannot do if the parse threw it away.
    let mut datagram = Datagram::discover();
    datagram.relay_address = Ipv4Addr::new(10, 0, 0, 254);

    let request = parse(&datagram.bytes()).expect("a relayed DISCOVER still parses");

    assert_eq!(request.relay_address, Ipv4Addr::new(10, 0, 0, 254));
}

#[test]
fn a_hardware_address_is_read_to_the_length_the_client_declared() {
    // `chaddr` is sixteen octets and this client's address is six, so
    // the ten that follow it are padding and not part of the address.
    let request = discover();

    assert_eq!(request.hardware_address.len(), 6);
}

#[test]
fn an_offer_carries_the_nine_options_in_the_order_they_were_measured() {
    let frame = Answer::offer(&discover(), server(), OFFERED_ADDRESS, lease()).encode();

    assert_eq!(option_codes(&frame), MEASURED_ORDER.to_vec());
}

#[test]
fn an_acknowledgement_carries_the_same_nine_as_an_offer() {
    let frame = Answer::acknowledge(&discover(), server(), OFFERED_ADDRESS, lease()).encode();

    assert_eq!(option_codes(&frame), MEASURED_ORDER.to_vec());
    assert_eq!(
        option_value(&frame, 53),
        Some(vec![5]),
        "an acknowledgement is message type 5"
    );
}

#[test]
fn a_refusal_carries_the_message_type_and_the_server_and_nothing_else() {
    // A refusal that carried a mask and a router would be describing
    // the network the client is being told it is not on.
    let frame = Answer::refuse(&discover(), server()).encode();

    assert_eq!(option_codes(&frame), vec![53, 54]);
    assert_eq!(option_value(&frame, 53), Some(vec![6]));
    assert_eq!(
        option_value(&frame, 54),
        Some(SERVER_ADDRESS.octets().to_vec())
    );
}

#[test]
fn an_answer_to_an_inform_carries_the_configuration_and_no_lease() {
    let mut datagram = Datagram::discover();
    datagram.client_address = Ipv4Addr::new(10, 0, 0, 77);
    datagram.options = options(&[option(53, &[INFORM])]);
    let request = parse(&datagram.bytes()).expect("an INFORM parses");

    let frame = Answer::inform(&request, server()).encode();

    assert_eq!(option_codes(&frame), vec![53, 54, 1, 28, 3, 6]);
    assert_eq!(
        yiaddr(&frame),
        Ipv4Addr::UNSPECIFIED,
        "an INFORM is answered with no address, the client having one"
    );
}

#[test]
fn the_renewal_and_rebinding_times_are_half_and_seven_eighths_of_the_lease() {
    let frame = Answer::offer(&discover(), server(), OFFERED_ADDRESS, lease()).encode();

    assert_eq!(
        option_value(&frame, 51),
        Some(43200_u32.to_be_bytes().to_vec())
    );
    assert_eq!(
        option_value(&frame, 58),
        Some(21600_u32.to_be_bytes().to_vec())
    );
    assert_eq!(
        option_value(&frame, 59),
        Some(37800_u32.to_be_bytes().to_vec())
    );
}

#[test]
fn the_derived_times_are_computed_rather_than_truncated_to_a_divisible_lease() {
    // A lease that is not a multiple of eight still gets
    // seven-eighths of itself rather than seven-eighths of the
    // nearest multiple below, and a lease at the top of the field
    // does not wrap on the way.
    let one_hour_and_one = LeaseSeconds::new(3601).expect("a lease");
    assert_eq!(one_hour_and_one.renewal(), 1800);
    assert_eq!(one_hour_and_one.rebinding(), 3150);

    let largest = LeaseSeconds::new(i64::from(u32::MAX)).expect("a lease");
    assert_eq!(largest.renewal(), u32::MAX / 2);
    assert_eq!(largest.rebinding(), 3_758_096_383);
}

#[test]
fn the_server_identifier_the_router_and_the_nameserver_are_all_the_arrival_interface() {
    // Measured: a client that asked for nothing received the server's
    // own address as its router and its nameserver.
    let frame = Answer::offer(&discover(), server(), OFFERED_ADDRESS, lease()).encode();
    let own = SERVER_ADDRESS.octets().to_vec();

    assert_eq!(option_value(&frame, 54), Some(own.clone()));
    assert_eq!(option_value(&frame, 3), Some(own.clone()));
    assert_eq!(option_value(&frame, 6), Some(own));
}

#[test]
fn the_mask_and_the_broadcast_address_come_from_the_prefix() {
    let cases = [
        (0_u8, [0, 0, 0, 0], [255, 255, 255, 255]),
        (16, [255, 255, 0, 0], [10, 0, 255, 255]),
        (24, [255, 255, 255, 0], [10, 0, 0, 255]),
        (25, [255, 255, 255, 128], [10, 0, 0, 127]),
        (31, [255, 255, 255, 254], [10, 0, 0, 1]),
        (32, [255, 255, 255, 255], [10, 0, 0, 1]),
    ];

    for (prefix_length, netmask, broadcast) in cases {
        let facts = ServerFacts::new(SERVER_ADDRESS, prefix_length).expect("a prefix");

        assert_eq!(
            facts.netmask(),
            Ipv4Addr::from(netmask),
            "the mask of a /{prefix_length}"
        );
        assert_eq!(
            facts.broadcast(),
            Ipv4Addr::from(broadcast),
            "the broadcast address of a /{prefix_length}"
        );

        let frame = Answer::offer(&discover(), facts, OFFERED_ADDRESS, lease()).encode();
        assert_eq!(option_value(&frame, 1), Some(netmask.to_vec()));
        assert_eq!(option_value(&frame, 28), Some(broadcast.to_vec()));
    }
}

#[test]
fn an_answer_echoes_the_transaction_and_the_client_it_answers() {
    let mut datagram = Datagram::discover();
    datagram.broadcast = true;
    datagram.relay_address = Ipv4Addr::new(10, 0, 0, 254);
    let request = parse(&datagram.bytes()).expect("a DISCOVER parses");

    let answer = Answer::offer(&request, server(), OFFERED_ADDRESS, lease());
    let frame = answer.encode();
    let decoded = Message::decode(&mut Decoder::new(&frame)).expect("our own bytes decode");

    assert_eq!(decoded.xid(), TRANSACTION);
    assert_eq!(decoded.chaddr(), CLIENT_MAC.as_slice());
    assert_eq!(decoded.htype(), dhcproto::v4::HType::Eth);
    assert_eq!(decoded.giaddr(), Ipv4Addr::new(10, 0, 0, 254));
    assert!(
        decoded.flags().broadcast(),
        "the flag is carried back as the client set it"
    );
    assert_eq!(answer.kind(), AnswerKind::Offer);
    assert_eq!(answer.client_address(), OFFERED_ADDRESS);
    assert_eq!(answer.hardware_address(), CLIENT_MAC.as_slice());
    assert!(answer.broadcast());
}

#[test]
fn a_cleared_broadcast_flag_is_carried_back_cleared() {
    let frame = Answer::offer(&discover(), server(), OFFERED_ADDRESS, lease()).encode();
    let decoded = Message::decode(&mut Decoder::new(&frame)).expect("our own bytes decode");

    assert!(!decoded.flags().broadcast());
}

#[test]
fn an_offer_this_crate_built_decodes_as_the_message_it_describes() {
    // The encoder here is this project's own, so a decoder written
    // elsewhere reading it back is the only cross-check available
    // short of the lab.
    let frame = Answer::offer(&discover(), server(), OFFERED_ADDRESS, lease()).encode();
    let decoded = Message::decode(&mut Decoder::new(&frame)).expect("our own bytes decode");

    assert_eq!(decoded.opcode(), dhcproto::v4::Opcode::BootReply);
    assert_eq!(decoded.hlen(), 6);
    assert_eq!(decoded.yiaddr(), OFFERED_ADDRESS);
    assert_eq!(decoded.ciaddr(), Ipv4Addr::UNSPECIFIED);
    assert_eq!(decoded.siaddr(), Ipv4Addr::UNSPECIFIED);
    assert_eq!(
        decoded.opts().msg_type(),
        Some(dhcproto::v4::MessageType::Offer)
    );
    assert_eq!(
        decoded.opts().get(OptionCode::Router),
        Some(&DhcpOption::Router(vec![SERVER_ADDRESS]))
    );
    assert_eq!(
        decoded.opts().get(OptionCode::DomainNameServer),
        Some(&DhcpOption::DomainNameServer(vec![SERVER_ADDRESS]))
    );
    assert_eq!(
        decoded.opts().get(OptionCode::AddressLeaseTime),
        Some(&DhcpOption::AddressLeaseTime(43200))
    );
}

#[test]
fn an_answer_keeps_chaddr_inside_its_field_whatever_the_length_was() {
    // The other side of the guard: a reply that wrote only `hlen`
    // octets where the field is sixteen would misalign `sname`,
    // `file`, the cookie and every option after them.
    for hlen in [1_u8, 6, 16] {
        let mut datagram = Datagram::discover();
        datagram.hlen = hlen;
        datagram.chaddr = (1..=hlen).collect();
        let request = parse(&datagram.bytes()).expect("a DISCOVER parses");

        let frame = Answer::offer(&request, server(), OFFERED_ADDRESS, lease()).encode();

        assert_eq!(
            option_codes(&frame),
            MEASURED_ORDER.to_vec(),
            "the options are still where they belong for hlen {hlen}"
        );
        let decoded = Message::decode(&mut Decoder::new(&frame)).expect("our own bytes decode");
        assert_eq!(decoded.hlen(), hlen);
        assert_eq!(decoded.chaddr().len(), usize::from(hlen));
    }
}

#[test]
fn a_prefix_longer_than_an_address_is_refused() {
    assert_eq!(
        ServerFacts::new(SERVER_ADDRESS, 33),
        Err(Error::PrefixTooLong { prefix_length: 33 })
    );
    assert_eq!(
        ServerFacts::new(SERVER_ADDRESS, 255),
        Err(Error::PrefixTooLong { prefix_length: 255 })
    );
}

#[test]
fn a_lease_time_no_option_can_carry_is_refused() {
    for seconds in [0_i64, -1, -43200, i64::from(u32::MAX) + 1, i64::MAX] {
        assert_eq!(
            LeaseSeconds::new(seconds),
            Err(Error::LeaseTimeNotRepresentable { seconds }),
            "a lease of {seconds} seconds"
        );
    }
}

#[test]
fn a_message_type_is_the_one_the_client_sent() {
    let kinds = [
        (DISCOVER, RequestKind::Discover),
        (REQUEST, RequestKind::Request),
        (4, RequestKind::Decline),
        (7, RequestKind::Release),
        (INFORM, RequestKind::Inform),
    ];

    for (code, kind) in kinds {
        let mut datagram = Datagram::discover();
        datagram.options = options(&[option(53, &[code])]);

        let request = parse(&datagram.bytes()).expect("a client message parses");

        assert_eq!(request.kind, kind, "message type {code}");
    }
}

/// `yiaddr` out of an encoded reply.
fn yiaddr(frame: &[u8]) -> Ipv4Addr {
    Message::decode(&mut Decoder::new(frame))
        .expect("our own bytes decode")
        .yiaddr()
}
