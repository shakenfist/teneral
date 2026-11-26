//! The seam between deciding and sending.
//!
//! One trait with one method, and the narrowness is the point. The
//! repository's test target cannot bind a privileged port or open an
//! `AF_PACKET` socket, so whatever is decided below this line is
//! decided where no test here can see it. Everything the socket layer
//! needs in order to send is therefore gathered into [`Reply`] by the
//! layer above, and what is left for the implementation is the one
//! thing the lab measures directly: that the bytes went out of the
//! right file descriptor.
//!
//! The fields a reply carries are the ones the measured behaviours
//! need, and no more. A client with no address is replied to at the
//! address it is being offered, which ordinary IP routing cannot
//! deliver and which works in a frame addressed to the hardware
//! address the client does hold -- so the hardware address and the
//! offered address are both here, and so is the address the client
//! already holds, which is what separates a client that can be reached
//! by routing from one that cannot. A broadcast reply from an
//! interface with no default route needs its egress named per message,
//! so the interface is here. The broadcast flag is a request for
//! broadcast and never permission to unicast, so it is carried as the
//! client set it and read that way below. Which answer this is, is
//! here because a refusal is broadcast however the client could
//! otherwise have been reached. And the server's own address is here
//! so that a frame built below the IP layer carries the same source
//! address as the server identifier inside it -- read once, by the
//! layer that derived the answer, rather than read again by the layer
//! that sends it and possibly differing.

use std::net::Ipv4Addr;

use crate::decide::Arrival;
use crate::wire::{Answer, AnswerKind, Request};

/// Bytes to put on an interface, and who to address them to.
///
/// Built by the decision layer and read by the socket layer. The
/// fields are private with accessors rather than public, because a
/// reply assembled anywhere other than from an answer and the
/// interface that answer was derived from would be a reply nobody
/// could reconstruct afterwards.
#[derive(Clone, Copy, Debug)]
pub struct Reply<'a> {
    /// The namespace the interface below is named within.
    namespace: &'a str,
    /// The interface the request arrived on, which is also the
    /// interface the reply leaves by.
    ifname: &'a str,
    /// The client's hardware address, truncated to the length it
    /// declared.
    hardware_address: &'a [u8],
    /// Which answer this is.
    kind: AnswerKind,
    /// The address of the interface the answer was derived from.
    server_address: Ipv4Addr,
    /// The address being offered or acknowledged, or the unspecified
    /// address where there is none.
    client_address: Ipv4Addr,
    /// The address the client already holds, or the unspecified
    /// address if it holds none.
    held_address: Ipv4Addr,
    /// Whether the client asked for a broadcast reply.
    broadcast: bool,
    /// The datagram itself.
    datagram: &'a [u8],
}

impl<'a> Reply<'a> {
    /// Gather what the socket layer needs to send one answer.
    ///
    /// Taken from the arrival, the request and the answer together
    /// rather than field by field, so that a reply cannot be assembled
    /// from an answer and an interface that answer was not derived
    /// from -- which would be one network's answer leaving by
    /// another's interface, the failure this project exists to
    /// prevent.
    pub(crate) fn new(
        arrival: &Arrival<'a>,
        request: &Request,
        answer: &'a Answer,
        datagram: &'a [u8],
    ) -> Self {
        Reply {
            namespace: arrival.namespace,
            ifname: arrival.ifname,
            hardware_address: answer.hardware_address(),
            kind: answer.kind(),
            server_address: arrival.server.address(),
            client_address: answer.client_address(),
            held_address: request.client_address,
            broadcast: answer.broadcast(),
            datagram,
        }
    }

    /// The namespace the interface is named within.
    pub fn namespace(&self) -> &str {
        self.namespace
    }

    /// The interface to send on.
    pub fn ifname(&self) -> &str {
        self.ifname
    }

    /// The client's hardware address, which is the only address a
    /// client with no IP address can be reached at.
    pub fn hardware_address(&self) -> &[u8] {
        self.hardware_address
    }

    /// Which answer this is.
    ///
    /// The send path needs it for one case: a refusal is broadcast
    /// whether or not the client could have been reached any other
    /// way, a client being told it may not have an address being one
    /// that may not be able to receive a unicast at all.
    pub fn kind(&self) -> AnswerKind {
        self.kind
    }

    /// The address of the interface this answer was derived from.
    ///
    /// Which is the source address of anything sent, and the same
    /// address the reply's own server identifier carries. It is read
    /// here rather than read again below so that a frame built by hand
    /// cannot disagree with the payload inside it.
    pub fn server_address(&self) -> Ipv4Addr {
        self.server_address
    }

    /// The address being offered or acknowledged.
    ///
    /// The unspecified address means there is none -- a refusal, or an
    /// answer to a client that already has one -- which is also the
    /// case in which there is no address to unicast to.
    pub fn client_address(&self) -> Ipv4Addr {
        self.client_address
    }

    /// The address the client already holds, or the unspecified
    /// address if it holds none.
    ///
    /// This is what says whether ordinary routing can deliver the
    /// reply: a client that holds an address answers address
    /// resolution for it, and a client that does not cannot be reached
    /// above the link layer at all.
    pub fn held_address(&self) -> Ipv4Addr {
        self.held_address
    }

    /// Whether the client asked for a broadcast reply.
    ///
    /// A request for broadcast, never permission to unicast: a client
    /// that cleared the flag was observed accepting a broadcast reply
    /// in every transaction measured, so a clear flag says nothing
    /// about what the client will accept.
    pub fn broadcast(&self) -> bool {
        self.broadcast
    }

    /// The bytes to put on the wire.
    pub fn datagram(&self) -> &[u8] {
        self.datagram
    }
}

/// Where an answer goes.
///
/// One method, so that the untestable part of this crate is one
/// implementation of it. `std::io::Error` rather than an error of this
/// crate's own because sending bytes on an interface is an operating
/// system call and every implementation of this will have one to
/// report; wrapping it would add a type without adding a fact.
pub trait Egress: Send + Sync {
    /// Put `reply`'s bytes on `reply`'s interface, addressed to
    /// `reply`'s client.
    fn send(&self, reply: &Reply<'_>) -> std::io::Result<()>;
}
