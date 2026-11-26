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
//! The fields a reply carries are the ones three measured behaviours
//! need. A client with no address is replied to at the address it is
//! being offered, which ordinary IP routing cannot deliver and which
//! works in a frame addressed to the hardware address the client does
//! hold -- so the hardware address and the offered address are both
//! here. A broadcast reply from an interface with no default route
//! needs its egress named per message, so the interface is here. And
//! the broadcast flag is a request for broadcast and never permission
//! to unicast, so it is carried as the client set it and read that way
//! below.

use std::net::Ipv4Addr;

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
    /// The address being offered or acknowledged, or the unspecified
    /// address where there is none.
    client_address: Ipv4Addr,
    /// Whether the client asked for a broadcast reply.
    broadcast: bool,
    /// The datagram itself.
    datagram: &'a [u8],
}

impl<'a> Reply<'a> {
    /// Gather what the socket layer needs to send one answer.
    pub(crate) fn new(
        namespace: &'a str,
        ifname: &'a str,
        hardware_address: &'a [u8],
        client_address: Ipv4Addr,
        broadcast: bool,
        datagram: &'a [u8],
    ) -> Self {
        Reply {
            namespace,
            ifname,
            hardware_address,
            client_address,
            broadcast,
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

    /// The address being offered or acknowledged.
    ///
    /// The unspecified address means there is none -- a refusal, or an
    /// answer to a client that already has one -- which is also the
    /// case in which there is no address to unicast to.
    pub fn client_address(&self) -> Ipv4Addr {
        self.client_address
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
