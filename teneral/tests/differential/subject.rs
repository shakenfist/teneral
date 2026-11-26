//! The subjects: the things in this repository whose bytes a case can
//! be compared against.
//!
//! A subject is not a function from bytes to bytes, and the reason is
//! the whole of why a case carries a `[network]` table. A DHCP reply
//! is not a function of the request alone: the lease time comes from
//! the network, the mask and the broadcast address from the prefix the
//! arrival interface holds, and which address is offered depends on
//! what the store has already given away. A subject that took only a
//! datagram could therefore only ever answer against a configuration
//! nobody had written down, and a rig built on it would be comparing
//! teneral's reply to a question dnsmasq was never asked.
//!
//! So the signature is "this request, against this configuration", and
//! the answer is a value that says where the reply went as well as
//! what it said -- because half of what this corpus has to defend is
//! the addressing. A reply to a client with no address goes to an
//! address nobody holds yet, in a frame addressed to the hardware
//! address the client does hold, and a comparison that ignored that
//! would miss the measurement the lab was built to take.

use std::net::Ipv4Addr;

use crate::corpus::Configuration;

/// Something that answers a captured request the way teneral does.
pub struct Subject {
    /// The name a case's `comparison.subject` has to match.
    pub name: &'static str,
    /// What teneral does with the captured request, against the
    /// configuration the capture was taken under.
    pub answer: fn(&Configuration, &[u8]) -> Result<Answered, String>,
}

/// What teneral did.
#[derive(Debug)]
pub enum Answered {
    /// It put a reply on the interface the request arrived on.
    Sent(Sent),
    /// It put nothing on the wire, for the reason given.
    ///
    /// A silence is an answer and is compared as one: a refused
    /// datagram, a message this front-end does not serve, and a
    /// message the protocol specifies no reply to are all silences,
    /// and the reason is carried so that a case naming the wrong one
    /// can be told apart from a case that is right.
    Silent(String),
}

/// One reply, as the layer below the decision would have been given
/// it.
#[derive(Debug)]
pub struct Sent {
    /// The bytes of the reply itself.
    pub datagram: Vec<u8>,
    /// The address the reply is sent from, which is the address of the
    /// interface the answer was derived from.
    ///
    /// Read off the reply rather than off the case, because a frame
    /// built below the IP layer carries it and the answer's own server
    /// identifier carries it too: a case comparing it is checking that
    /// the two cannot disagree.
    pub source: Ipv4Addr,
    /// Where they were to go.
    pub delivery: Delivery,
}

/// How a reply was to be addressed.
///
/// A shape of the rig's own rather than the datapath's, so that this
/// file says what a case compares; it is built from the datapath's own
/// choice of delivery, which is the function RFC 2131 section 4.1
/// specifies and the one part of the send path that is a judgement
/// rather than a system call.
#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    /// To every client on the link.
    Broadcast,
    /// By ordinary routing, to an address the client holds.
    Unicast {
        /// The address it holds.
        address: Ipv4Addr,
    },
    /// Below the IP layer, to a client that holds no address.
    BelowIp {
        /// The address the IP packet is addressed to.
        address: Ipv4Addr,
        /// The hardware address the frame is addressed to.
        hardware_address: Vec<u8>,
    },
}

impl Delivery {
    /// The address the IP packet carrying this reply is addressed to.
    pub fn ip_destination(&self) -> Ipv4Addr {
        match self {
            Delivery::Broadcast => Ipv4Addr::BROADCAST,
            Delivery::Unicast { address } | Delivery::BelowIp { address, .. } => *address,
        }
    }

    /// The hardware address the frame is addressed to, where this
    /// layer decides it.
    ///
    /// `None` for a unicast by ordinary routing: the kernel resolves
    /// that one from the address, so teneral does not choose it and a
    /// case must not compare it.
    pub fn ethernet_destination(&self) -> Option<Vec<u8>> {
        match self {
            Delivery::Broadcast => Some(vec![0xff; 6]),
            Delivery::Unicast { .. } => None,
            Delivery::BelowIp {
                hardware_address, ..
            } => Some(hardware_address.clone()),
        }
    }
}

/// Every subject the rig can compare against.
pub const SUBJECTS: &[Subject] = &[Subject {
    name: "dhcpv4",
    answer: crate::dhcpv4::answer,
}];

/// The subject a case names, if it is registered.
pub fn find(name: &str) -> Option<&'static Subject> {
    SUBJECTS.iter().find(|subject| subject.name == name)
}
