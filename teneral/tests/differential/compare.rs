//! The comparison: teneral's reply beside the captured one, field by
//! field.
//!
//! Field-wise and not byte-wise, and that is a correction rather than
//! a concession. Three things in a reply cannot be compared byte for
//! byte without asserting something false. Which address comes out of
//! a pool is teneral's own choice and dnsmasq's is by client identity,
//! so `yiaddr` is asserted to agree with the address the reply was
//! sent to rather than to be the address dnsmasq picked. The IP and
//! UDP checksums are recomputed by whoever puts the frame on the wire
//! -- for two of teneral's three delivery paths that is the kernel --
//! so they are not teneral's bytes to compare. And the identification
//! and differentiated-services fields of the IPv4 header are in the
//! same position.
//!
//! So a case names the fields it asserts, and this reports **every**
//! difference rather than the first. One byte offset tells a reader
//! that two frames differ; a list of fields tells them what teneral
//! got wrong, which is usually more than one thing and never usefully
//! described by whichever happens to come first in the frame.

use crate::corpus::{Comparison, Expect};
use crate::frame::{self, Frame};
use crate::subject::{Answered, Sent};

/// Every field name a case may assert on, for the message a mistyped
/// one gets.
const HEADER_FIELDS: &[&str] = &[
    "op", "htype", "hlen", "hops", "xid", "secs", "flags", "ciaddr", "yiaddr", "siaddr", "giaddr",
    "chaddr", "sname", "file",
];

/// The names that are not header fields.
const OTHER_FIELDS: &[&str] = &[
    "option-codes",
    "option-<code>",
    "ethernet-destination",
    "ip-destination",
    "payload-length",
];

/// The agreements a case may assert.
const AGREEMENTS: &[&str] = &[
    "yiaddr-matches-ip-destination",
    "chaddr-matches-ethernet-destination",
];

/// Compare one case's captured reply with teneral's answer.
///
/// Returns every problem it found. An empty list is a pass.
pub fn compare(
    comparison: &Comparison,
    request: &[u8],
    response: Option<&[u8]>,
    answered: &Answered,
) -> Vec<String> {
    let mut problems = Vec::new();

    if comparison.expect == Expect::Silence {
        return silence(answered, response);
    }

    let Some(response) = response else {
        return vec![
            "the case records no reply and expects teneral to send one, which cannot be \
             compared against anything. A case whose capture holds no reply has \
             `expect = \"silence\"`."
                .to_owned(),
        ];
    };

    let sent = match answered {
        Answered::Sent(sent) => sent,
        Answered::Silent(why) => {
            return vec![format!(
                "the capture holds a reply and teneral sent nothing: {why}. Either teneral has \
                 stopped answering a request it used to answer, or this is a divergence, in \
                 which case the case says `expect = \"silence\"` and says why in `asserts`."
            )];
        }
    };

    let theirs = match frame::read(response) {
        Ok(theirs) => theirs,
        Err(why) => return vec![format!("the captured reply {why}")],
    };
    let ours = match ours(sent) {
        Ok(ours) => ours,
        Err(why) => return vec![format!("teneral's reply {why}")],
    };
    // Read so that a case's `[network]` table can be checked against
    // the request it was written for: a case whose configuration does
    // not match the capture would compare two different experiments.
    if let Err(why) = frame::read(request) {
        problems.push(format!("the captured request {why}"));
    }

    for name in &comparison.fields {
        match difference(name, &theirs, &ours, sent) {
            Err(why) => problems.push(why),
            Ok(Some(difference)) => problems.push(format!(
                "{name} differs and the case asserts that it matches: {difference}"
            )),
            Ok(None) => {}
        }
    }

    for name in &comparison.diverges {
        match difference(name, &theirs, &ours, sent) {
            Err(why) => problems.push(why),
            Ok(None) => problems.push(format!(
                "{name} now agrees with the capture, and the case records it as a deliberate \
                 divergence. Either teneral has changed or the reference has; move {name} from \
                 `diverges` to `fields` and say so in `asserts`."
            )),
            Ok(Some(_)) => {}
        }
    }

    for name in &comparison.agree {
        for (whose, reply, sent) in [
            ("the capture", &theirs, None),
            ("teneral", &ours, Some(sent)),
        ] {
            if let Err(why) = agreement(name, reply, sent) {
                problems.push(format!("{whose}: {why}"));
            }
        }
    }

    problems
}

/// A case whose whole assertion is that teneral says nothing.
fn silence(answered: &Answered, response: Option<&[u8]>) -> Vec<String> {
    match answered {
        Answered::Silent(_) => Vec::new(),
        Answered::Sent(sent) => {
            let also = match response {
                Some(_) => {
                    "the reference implementation answered it too, and the case records \
                            teneral's silence as the difference"
                }
                None => "nothing answered it in the capture",
            };
            vec![format!(
                "the case asserts that teneral sends nothing and it sent {} octets, addressed \
                 {:?}. {also}, so this is teneral's behaviour changing rather than the case \
                 being wrong.",
                sent.datagram.len(),
                sent.delivery
            )]
        }
    }
}

/// teneral's reply read the same way the captured one is.
///
/// The datagram is teneral's own bytes; the addressing comes from the
/// delivery the send path chose, because the headers around the
/// datagram are written by the kernel on two of the three paths and so
/// are not teneral's to compare.
fn ours(sent: &Sent) -> Result<Frame, String> {
    // A datagram rather than a frame, so it is read by wrapping it in
    // the one shape this reader understands: the fields it compares
    // all live in the BOOTP message, and the addressing is read off
    // the delivery instead.
    frame::read(&synthetic(sent))
}

/// teneral's datagram inside the smallest frame that carries it.
///
/// Built here and nowhere near the front-end, and never compared as
/// bytes: only the BOOTP fields inside it are read, and the addressing
/// a case asserts on comes from the delivery. The addresses and ports
/// below are the protocol's own constants, so that the reader above
/// accepts the frame at all.
fn synthetic(sent: &Sent) -> Vec<u8> {
    let destination = sent.delivery.ip_destination();
    let hardware = sent
        .delivery
        .ethernet_destination()
        .unwrap_or_else(|| vec![0; 6]);

    let mut frame = Vec::new();
    frame.extend_from_slice(&hardware);
    frame.extend_from_slice(&[0; 6]);
    frame.extend_from_slice(&[0x08, 0x00]);
    frame.extend_from_slice(&[0x45, 0x00, 0, 0, 0, 0, 0, 0, 64, 17, 0, 0]);
    frame.extend_from_slice(&sent.source.octets());
    frame.extend_from_slice(&destination.octets());
    frame.extend_from_slice(&67_u16.to_be_bytes());
    frame.extend_from_slice(&68_u16.to_be_bytes());
    frame.extend_from_slice(&[0, 0, 0, 0]);
    frame.extend_from_slice(&sent.datagram);
    frame
}

/// How one named field differs, or `None` where it does not.
fn difference(
    name: &str,
    theirs: &Frame,
    ours: &Frame,
    sent: &Sent,
) -> Result<Option<String>, String> {
    if let Some(code) = name.strip_prefix("option-")
        && code != "codes"
    {
        let code: u8 = code.parse().map_err(|_| {
            format!("`{name}` names no field: after `option-` comes an option code, 0 to 255")
        })?;
        return Ok(option(code, theirs, ours));
    }

    let found = match name {
        "op" => octets(theirs.op, ours.op),
        "htype" => octets(theirs.htype, ours.htype),
        "hlen" => octets(theirs.hlen, ours.hlen),
        "hops" => octets(theirs.hops, ours.hops),
        "xid" => bytes(&theirs.xid, &ours.xid),
        "secs" => numbers(theirs.secs, ours.secs),
        "flags" => bytes(&theirs.flags, &ours.flags),
        "ciaddr" => addresses(theirs.ciaddr, ours.ciaddr),
        "yiaddr" => addresses(theirs.yiaddr, ours.yiaddr),
        "siaddr" => addresses(theirs.siaddr, ours.siaddr),
        "giaddr" => addresses(theirs.giaddr, ours.giaddr),
        "chaddr" => bytes(&theirs.chaddr, &ours.chaddr),
        "sname" => bytes(&theirs.sname, &ours.sname),
        "file" => bytes(&theirs.file, &ours.file),
        "option-codes" => codes(theirs, ours),
        "ethernet-destination" => match sent.delivery.ethernet_destination() {
            Some(chosen) => bytes(&theirs.ethernet_destination, &chosen),
            None => {
                return Err(format!(
                    "`{name}` cannot be asserted on this reply: teneral is sending it by \
                     ordinary routing, so the kernel resolves the hardware address and teneral \
                     never chooses one. Assert `ip-destination` instead."
                ));
            }
        },
        "ip-source" => addresses(theirs.ip_source, sent.source),
        "ip-destination" => addresses(theirs.ip_destination, sent.delivery.ip_destination()),
        "payload-length" => lengths(theirs.datagram.len(), ours.datagram.len()),
        unknown => {
            return Err(format!(
                "`{unknown}` names no field. A case asserts on one of {}, one of {}, or \
                 `option-<code>` for the value of one option.",
                HEADER_FIELDS.join(", "),
                OTHER_FIELDS.join(", "),
            ));
        }
    };

    Ok(found)
}

/// An agreement within one reply.
fn agreement(name: &str, reply: &Frame, sent: Option<&Sent>) -> Result<(), String> {
    match name {
        "yiaddr-matches-ip-destination" => {
            let destination = match sent {
                Some(sent) => sent.delivery.ip_destination(),
                None => reply.ip_destination,
            };
            if reply.yiaddr != destination {
                return Err(format!(
                    "the reply offers {} and was addressed to {destination}, and the case \
                     asserts that the two agree -- which is how a case can hold a reply whose \
                     address teneral chose differently",
                    reply.yiaddr
                ));
            }
            Ok(())
        }
        "chaddr-matches-ethernet-destination" => {
            let destination = match sent {
                Some(sent) => match sent.delivery.ethernet_destination() {
                    Some(chosen) => chosen,
                    None => {
                        return Err(
                            "teneral is sending this reply by ordinary routing, so it chooses no \
                             hardware address and this agreement cannot hold of it"
                                .to_owned(),
                        );
                    }
                },
                None => reply.ethernet_destination.clone(),
            };
            let hardware = reply
                .chaddr
                .get(..usize::from(reply.hlen))
                .ok_or_else(|| "the reply's hlen is longer than chaddr".to_owned())?;
            if hardware != destination.as_slice() {
                return Err(format!(
                    "the reply names the hardware address {} and the frame went to {}, and the \
                     case asserts that the two agree",
                    hex(hardware),
                    hex(&destination)
                ));
            }
            Ok(())
        }
        unknown => Err(format!(
            "`{unknown}` names no agreement. A case asserts one of {}.",
            AGREEMENTS.join(", ")
        )),
    }
}

fn octets(theirs: u8, ours: u8) -> Option<String> {
    (theirs != ours).then(|| format!("the capture has {theirs} and teneral produced {ours}"))
}

fn numbers(theirs: u16, ours: u16) -> Option<String> {
    (theirs != ours).then(|| format!("the capture has {theirs} and teneral produced {ours}"))
}

fn lengths(theirs: usize, ours: usize) -> Option<String> {
    (theirs != ours).then(|| {
        format!("the capture has {theirs} octets of BOOTP message and teneral produced {ours}")
    })
}

fn addresses(theirs: std::net::Ipv4Addr, ours: std::net::Ipv4Addr) -> Option<String> {
    (theirs != ours).then(|| format!("the capture has {theirs} and teneral produced {ours}"))
}

fn bytes(theirs: &[u8], ours: &[u8]) -> Option<String> {
    (theirs != ours).then(|| {
        format!(
            "the capture has {} and teneral produced {}",
            hex(theirs),
            hex(ours)
        )
    })
}

fn codes(theirs: &Frame, ours: &Frame) -> Option<String> {
    let mine = ours.option_codes();
    let yours = theirs.option_codes();

    (mine != yours).then(|| {
        format!("the capture carries {yours:?} in that order and teneral produced {mine:?}",)
    })
}

/// One option's value, which both replies must carry.
fn option(code: u8, theirs: &Frame, ours: &Frame) -> Option<String> {
    match (theirs.option(code), ours.option(code)) {
        (Some(yours), Some(mine)) if yours == mine => None,
        (Some(yours), Some(mine)) => Some(format!(
            "the capture carries {} and teneral produced {}",
            hex(yours),
            hex(mine)
        )),
        (Some(yours), None) => Some(format!(
            "the capture carries {} and teneral's reply does not carry option {code} at all",
            hex(yours)
        )),
        (None, Some(mine)) => Some(format!(
            "teneral carries {} and the captured reply does not carry option {code} at all",
            hex(mine)
        )),
        (None, None) => Some(format!("neither reply carries option {code}")),
    }
}

fn hex(octets: &[u8]) -> String {
    octets
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<String>>()
        .join(":")
}
