//! Datagrams that must be refused, and must be refused without
//! panicking.
//!
//! One process serves every network, so a malformed packet that
//! panics is a remote denial of service against all of them at once.
//! That makes this corpus the point of the layer rather than a
//! side-effect of it, and it is why every test here asserts a *named*
//! refusal: a guard that was removed and happened to be replaced by a
//! failure further along would otherwise look like a pass.
//!
//! "And does not panic" needs no separate assertion. A panic inside
//! `parse` unwinds through the test, so a test that reaches its
//! assertion at all has already proved it.
//!
//! Every input is a DISCOVER that parses with one thing wrong with
//! it, built from the same fixture the accepting tests use. A corpus
//! of hand-assembled packets would be a corpus in which an input that
//! stopped reaching the guard it was written for still passed.

use std::net::Ipv4Addr;

use crate::wire::test_support::{Datagram, OFFER, option, options};
use crate::wire::{Error, MAX_DATAGRAM_BYTES, OptionNotUnderstood, RequestKind, parse};

#[test]
fn a_datagram_of_no_bytes_is_refused() {
    assert_eq!(parse(&[]), Err(Error::TooShort { length: 0 }));
}

#[test]
fn a_datagram_truncated_at_any_field_boundary_is_refused_as_too_short() {
    // Every boundary in the fixed header, and the two either side of
    // the magic cookie. A truncation before the third byte cannot even
    // be asked about `hlen`, which is why the shortest of these are
    // here rather than somewhere else.
    let boundaries = [
        0_usize, // nothing at all
        1,       // op
        2,       // htype: one byte short of hlen
        3,       // hlen
        4,       // hops
        8,       // xid
        10,      // secs
        12,      // flags
        16,      // ciaddr
        20,      // yiaddr
        24,      // siaddr
        28,      // giaddr
        44,      // chaddr
        108,     // sname
        236,     // file: everything but the cookie
        239,     // one byte of the cookie missing
    ];

    for boundary in boundaries {
        let datagram = Datagram::discover().truncated_to(boundary);

        assert_eq!(
            parse(&datagram),
            Err(Error::TooShort { length: boundary }),
            "a datagram truncated to {boundary} bytes"
        );
    }
}

#[test]
fn a_hardware_length_past_the_end_of_the_field_is_refused() {
    // The guard this layer exists around. The decoder's `chaddr()`
    // sizes a slice by this byte, so above sixteen one accessor
    // panics and the other returns bytes the client chose as the
    // client's hardware address. Each of these inputs is an otherwise
    // complete DISCOVER, so a refusal that disappeared would be
    // answered rather than refused somewhere else.
    for hlen in [17_u8, 18, 32, 64, 128, 254, 255] {
        let mut datagram = Datagram::discover();
        datagram.hlen = hlen;

        assert_eq!(
            parse(&datagram.bytes()),
            Err(Error::HardwareLengthOverflowsField { hlen }),
            "a DISCOVER claiming an hlen of {hlen}"
        );
    }
}

#[test]
fn a_hardware_length_past_the_end_of_the_field_is_refused_whatever_follows_it() {
    // The borrowed accessor's failure depends on the length of the
    // buffer rather than on anything about the client: on a datagram
    // long enough to absorb the over-long slice it returns the wrong
    // bytes instead of panicking. So the guard is exercised at the
    // minimum length and at an ordinary one.
    for length in [240_usize, 300, 576, MAX_DATAGRAM_BYTES] {
        let mut datagram = Datagram::discover();
        datagram.hlen = 255;
        let mut bytes = datagram.bytes();
        bytes.resize(length, 0);

        assert_eq!(
            parse(&bytes),
            Err(Error::HardwareLengthOverflowsField { hlen: 255 }),
            "an hlen of 255 in a datagram of {length} bytes"
        );
    }
}

#[test]
fn a_hardware_length_of_zero_is_refused_rather_than_read_as_an_empty_address() {
    let mut datagram = Datagram::discover();
    datagram.hlen = 0;

    assert_eq!(parse(&datagram.bytes()), Err(Error::NoHardwareLength));
}

#[test]
fn a_hardware_length_of_exactly_the_field_is_the_boundary_and_is_accepted() {
    // Sixteen is the length the field holds, so it is the one value in
    // the corpus that must *not* be refused: a client with a sixteen
    // octet hardware address has sent nothing wrong. Asserting the
    // refusal here instead would have shipped an off-by-one as a
    // guard.
    let mut datagram = Datagram::discover();
    datagram.hlen = 16;
    datagram.chaddr = (1..=16).collect();

    let request = parse(&datagram.bytes()).expect("a sixteen-octet address is an address");

    assert_eq!(request.hardware_address, (1..=16).collect::<Vec<u8>>());
}

#[test]
fn a_hardware_address_of_all_zeroes_is_refused_rather_than_identified() {
    // A client that filled the field in with nothing. The length says
    // six, so nothing above would notice; what it would notice is
    // every such client in a network sharing one identity.
    let mut datagram = Datagram::discover();
    datagram.chaddr = vec![0; 6];

    assert_eq!(parse(&datagram.bytes()), Err(Error::HardwareAddressUnset));
}

#[test]
fn a_datagram_larger_than_the_link_will_carry_is_refused_before_it_is_parsed() {
    // The largest a UDP datagram can be, and one byte over the bound.
    // Refused on the length alone, which is why the first of these
    // need not be a DHCP message at all.
    let largest_datagram = vec![0_u8; 65507];
    assert_eq!(
        parse(&largest_datagram),
        Err(Error::TooLarge { length: 65507 })
    );

    let mut bytes = Datagram::discover().bytes();
    bytes.resize(MAX_DATAGRAM_BYTES + 1, 0);
    assert_eq!(
        parse(&bytes),
        Err(Error::TooLarge {
            length: MAX_DATAGRAM_BYTES + 1
        })
    );
}

#[test]
fn a_datagram_the_size_of_the_bound_is_the_boundary_and_is_accepted() {
    let mut bytes = Datagram::discover().bytes();
    bytes.resize(MAX_DATAGRAM_BYTES, 0);

    parse(&bytes).expect("a datagram at the bound is within it");
}

#[test]
fn a_datagram_without_the_magic_cookie_is_refused() {
    // The decoder does not check it, so a vendor-specific BOOTP
    // message would otherwise have its options area read as DHCP's.
    for cookie in [[0_u8, 0, 0, 0], [99, 130, 83, 98], [0xff; 4]] {
        let mut datagram = Datagram::discover();
        datagram.cookie = cookie;

        assert_eq!(
            parse(&datagram.bytes()),
            Err(Error::NotTheMagicCookie { found: cookie }),
            "a cookie of {cookie:?}"
        );
    }
}

#[test]
fn an_option_whose_length_runs_past_the_end_of_the_datagram_is_refused() {
    // The decoder stops at the first option it cannot read and
    // reports success, so without this the message would parse with
    // the overrunning option and everything after it silently absent.
    let mut datagram = Datagram::discover();
    datagram.options = vec![53, 1, 1, 61, 20, 1, 2, 3, 4, 255];

    assert_eq!(
        parse(&datagram.bytes()),
        Err(Error::OptionRunsPastTheEnd {
            code: 61,
            declared: 20,
            remaining: 5,
        })
    );
}

#[test]
fn an_option_with_no_length_byte_is_refused() {
    let mut datagram = Datagram::discover();
    datagram.options = vec![53, 1, 1, 61];

    assert_eq!(
        parse(&datagram.bytes()),
        Err(Error::OptionLengthMissing { code: 61 })
    );
}

#[test]
fn an_option_the_decoder_cannot_read_is_dropped_and_recorded_rather_than_refused() {
    // A host name that is not UTF-8 and a requested address two octets
    // wide are each framed correctly and each declined on their
    // contents. The record's own boundaries are known, so the option
    // can go without the datagram going with it: what an unreadable
    // option costs is that option.
    let cases = [(12_u8, vec![0xff_u8, 0xfe, 0xfd]), (50, vec![10, 0])];

    for (code, value) in cases {
        let mut datagram = Datagram::discover();
        datagram.options = options(&[option(53, &[1]), option(code, &value)]);

        let request = parse(&datagram.bytes()).expect("an unreadable option is not a refusal");

        assert_eq!(
            request.not_understood,
            vec![OptionNotUnderstood { code }],
            "option {code} carrying {value:?}"
        );
        assert_eq!(request.hostname, None);
        assert_eq!(request.requested_address, None);
    }
}

#[test]
fn a_host_name_is_kept_when_it_decodes_and_only_the_name_is_lost_when_it_does_not() {
    // The case this division exists for. A machine whose name is in a
    // local codepage sends a host name that is not UTF-8; what that
    // costs it is the name, and not its address.
    let mut readable = Datagram::discover();
    readable.options = options(&[option(53, &[1]), option(12, b"till-3")]);
    let request = parse(&readable.bytes()).expect("a readable host name parses");
    assert_eq!(request.hostname, Some("till-3".to_string()));
    assert_eq!(request.not_understood, vec![]);

    let mut unreadable = Datagram::discover();
    unreadable.options = options(&[option(53, &[1]), option(12, &[0x74, 0x69, 0xf6, 0x6c])]);
    let request = parse(&unreadable.bytes()).expect("an unreadable host name still parses");
    assert_eq!(
        request.kind,
        RequestKind::Discover,
        "and is still answerable"
    );
    assert_eq!(request.hostname, None);
    assert_eq!(
        request.not_understood,
        vec![OptionNotUnderstood { code: 12 }]
    );
}

#[test]
fn an_option_the_decoder_cannot_read_does_not_take_the_options_after_it() {
    // The decoder's options loop ends at the first value it declines
    // and reports success with what it had, so without the re-decode
    // the client identifier below would be absent and nothing would
    // say so.
    let mut datagram = Datagram::discover();
    datagram.options = options(&[
        option(53, &[1]),
        option(12, &[0xff, 0xfe]),
        option(61, &[0x01, 0x52, 0x54, 0x00, 0xaa, 0xbb, 0xcc]),
        option(50, &[10, 0, 0, 128]),
    ]);

    let request = parse(&datagram.bytes()).expect("the rest of the options are readable");

    assert_eq!(request.kind, RequestKind::Discover);
    assert_eq!(
        request.client_identifier,
        Some(vec![0x01, 0x52, 0x54, 0x00, 0xaa, 0xbb, 0xcc])
    );
    assert_eq!(
        request.requested_address,
        Some(Ipv4Addr::new(10, 0, 0, 128))
    );
    assert_eq!(
        request.not_understood,
        vec![OptionNotUnderstood { code: 12 }]
    );
}

#[test]
fn several_options_the_decoder_cannot_read_are_each_recorded() {
    let mut datagram = Datagram::discover();
    datagram.options = options(&[
        option(12, &[0xff]),
        option(53, &[1]),
        option(50, &[10, 0]),
        option(1, &[255, 255, 255]),
    ]);

    let request = parse(&datagram.bytes()).expect("three unreadable options are not a refusal");

    assert_eq!(
        request.not_understood,
        vec![
            OptionNotUnderstood { code: 12 },
            OptionNotUnderstood { code: 50 },
            OptionNotUnderstood { code: 1 },
        ]
    );
}

#[test]
fn more_unreadable_options_than_the_layer_will_drop_refuse_the_datagram() {
    // Four is the cap, and this test is what documents the number.
    // Each drop costs a decode of the whole datagram, so a bound is
    // what keeps the work one packet buys from being chosen by its
    // sender -- and it lands on the one thread serving every network
    // in this process. A client offering one or two options this
    // decoder declines is the ordinary case the drop exists for; one
    // offering five is not a client whose lease is being protected.
    let unreadable = [
        option(1, &[255, 255, 255]),
        option(3, &[10, 0, 0]),
        option(6, &[10, 0]),
        option(12, &[0xff, 0xfe]),
        option(50, &[10, 0]),
    ];
    let area = |count: usize| {
        let mut each = vec![option(53, &[1])];
        each.extend_from_slice(&unreadable[..count]);
        options(&each)
    };

    let mut at_the_cap = Datagram::discover();
    at_the_cap.options = area(4);
    let request = parse(&at_the_cap.bytes()).expect("four drops are within the cap");
    assert_eq!(request.kind, RequestKind::Discover);
    assert_eq!(
        request.not_understood,
        vec![
            OptionNotUnderstood { code: 1 },
            OptionNotUnderstood { code: 3 },
            OptionNotUnderstood { code: 6 },
            OptionNotUnderstood { code: 12 },
        ]
    );

    let mut past_the_cap = Datagram::discover();
    past_the_cap.options = area(5);
    assert_eq!(
        parse(&past_the_cap.bytes()),
        Err(Error::TooManyUnreadableOptions { limit: 4 })
    );
}

#[test]
fn an_option_code_the_decoder_has_no_type_for_is_not_treated_as_unreadable() {
    // Checked against the decoder rather than assumed: a code it has
    // no type for comes back as an unknown variant carrying the code
    // and the bytes, so a vendor or private-use option is never
    // mistaken for a value that would not decode.
    let mut datagram = Datagram::discover();
    datagram.options = options(&[
        option(53, &[1]),
        option(224, &[1, 2, 3]),
        option(43, &[0xde, 0xad]),
    ]);

    let request = parse(&datagram.bytes()).expect("an unknown option is not a refusal");

    assert_eq!(request.not_understood, vec![]);
}

#[test]
fn the_option_overload_code_is_reported_by_the_decoder_rather_than_consumed() {
    // The walk identifies a dropped option as the first framed code
    // missing from the decoded options, which is sound for a code the
    // decoder refuses and unsound for one it consumes without
    // reporting. Option 52 is the candidate a reader would worry
    // about: it says the options continue in `sname` and `file`, so a
    // decoder might read it and surface nothing. dhcproto 0.15 gives
    // it a type and keys it like any other, which this pins -- because
    // if a later version consumed it instead, this walk would read it
    // as undecodable, strip it, and silently lose every option after
    // it in the datagram, the requested address and the client
    // identifier among them. That is the precise failure the walk
    // exists to prevent, so it is checked rather than reasoned about.
    let mut datagram = Datagram::discover();
    datagram.options = options(&[
        option(53, &[1]),
        option(52, &[3]),
        option(50, &[10, 0, 0, 128]),
        option(61, &[0x01, 0x52, 0x54, 0x00, 0xaa, 0xbb, 0xcc]),
    ]);

    let request = parse(&datagram.bytes()).expect("option overload is not a refusal");

    assert_eq!(request.not_understood, vec![]);
    assert_eq!(
        request.requested_address,
        Some(Ipv4Addr::new(10, 0, 0, 128))
    );
    assert_eq!(
        request.client_identifier,
        Some(vec![0x01, 0x52, 0x54, 0x00, 0xaa, 0xbb, 0xcc])
    );
}

#[test]
fn padding_between_options_is_not_read_as_an_option_that_would_not_decode() {
    // The other half of the same soundness argument. The decoder does
    // consume two codes without reporting them -- the pad and the end
    // -- and the walk is sound because it records neither. A pad
    // recorded as a framed option would come back as code 0 "not
    // understood", which is a false note about the datagram and spends
    // one of the four drops the cap allows on a byte that means
    // nothing.
    let mut datagram = Datagram::discover();
    let mut area = option(53, &[1]);
    area.push(0);
    area.extend_from_slice(&option(61, &[0x01, 0x52, 0x54, 0x00, 0xaa, 0xbb, 0xcc]));
    area.push(0);
    area.push(0);
    area.push(255);
    datagram.options = area;

    let request = parse(&datagram.bytes()).expect("padding is not a refusal");

    assert_eq!(request.not_understood, vec![]);
    assert_eq!(request.kind, RequestKind::Discover);
    assert_eq!(
        request.client_identifier,
        Some(vec![0x01, 0x52, 0x54, 0x00, 0xaa, 0xbb, 0xcc])
    );
}

#[test]
fn a_message_type_whose_value_will_not_decode_is_still_a_refusal() {
    // The other side of the division. An option teneral acts on is
    // asked for by name, so one that was dropped is one that is
    // missing -- and a message with no readable message type cannot be
    // answered at all.
    let mut datagram = Datagram::discover();
    datagram.options = options(&[option(53, &[]), option(61, &[1, 2, 3])]);

    assert_eq!(parse(&datagram.bytes()), Err(Error::NoMessageType));
}

#[test]
fn a_datagram_with_no_message_type_option_is_refused() {
    // A BOOTP request. Answering one as though it were a DISCOVER
    // would hand a lease to a client with no way to renew it.
    let mut datagram = Datagram::discover();
    datagram.options = options(&[option(55, &[1, 3, 6])]);
    assert_eq!(parse(&datagram.bytes()), Err(Error::NoMessageType));

    // And the same with no options area at all, which is the shortest
    // datagram this layer accepts the shape of.
    let bytes = Datagram::discover().truncated_to(240);
    assert_eq!(parse(&bytes), Err(Error::NoMessageType));
}

#[test]
fn a_message_type_only_a_server_sends_is_refused() {
    // An OFFER, an ACK or a NAK arriving on the server's port is
    // another server on the link or a reflection of our own reply.
    for message_type in [OFFER, 5, 6] {
        let mut datagram = Datagram::discover();
        datagram.options = options(&[option(53, &[message_type])]);

        assert_eq!(
            parse(&datagram.bytes()),
            Err(Error::NotAClientMessage { message_type }),
            "message type {message_type}"
        );
    }
}

#[test]
fn a_message_type_no_client_sends_is_refused() {
    for message_type in [0_u8, 9, 17, 100, 255] {
        let mut datagram = Datagram::discover();
        datagram.options = options(&[option(53, &[message_type])]);

        assert_eq!(
            parse(&datagram.bytes()),
            Err(Error::NotAClientMessage { message_type }),
            "message type {message_type}"
        );
    }
}

#[test]
fn a_reply_from_a_server_is_refused() {
    let mut datagram = Datagram::discover();
    datagram.opcode = 2;

    assert_eq!(
        parse(&datagram.bytes()),
        Err(Error::NotABootRequest { opcode: 2 })
    );
}

#[test]
fn an_unknown_opcode_is_refused() {
    for opcode in [0_u8, 3, 255] {
        let mut datagram = Datagram::discover();
        datagram.opcode = opcode;

        assert_eq!(
            parse(&datagram.bytes()),
            Err(Error::NotABootRequest { opcode }),
            "an op of {opcode}"
        );
    }
}

#[test]
fn an_options_area_that_simply_stops_is_read_as_far_as_it_goes() {
    // RFC 2131 asks for an end option and every client measured sends
    // one, but a datagram that stops is unambiguous. Refusing one
    // would refuse a client over a byte nothing reads.
    let mut datagram = Datagram::discover();
    datagram.options = vec![53, 1, 1];

    let request = parse(&datagram.bytes()).expect("an unterminated options area is still readable");

    assert_eq!(request.client_address, Ipv4Addr::UNSPECIFIED);
}

#[test]
fn padding_between_options_is_skipped_rather_than_read_as_a_code() {
    let mut datagram = Datagram::discover();
    datagram.options = vec![0, 0, 53, 1, 1, 0, 61, 3, 1, 2, 3, 0, 255];

    let request = parse(&datagram.bytes()).expect("padded options are readable");

    assert_eq!(request.client_identifier, Some(vec![1, 2, 3]));
}
