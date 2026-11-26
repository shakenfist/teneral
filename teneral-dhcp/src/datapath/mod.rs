//! The sockets, and the frame that reaches a client with no address.
//!
//! This is the layer the repository's own tests cannot reach, and
//! everything above it is arranged so that what is lost is as small as
//! it can be made. `make test` runs as an unprivileged uid with no
//! added capabilities: it cannot open a socket below the IP layer, and
//! it has one interface, which is loopback. So nothing here decides
//! what a client is told. What it decides is where the bytes go, and
//! that is measured in a lab with a real client rather than asserted
//! here.
//!
//! The protocol's own port is not the barrier it looks like. The build
//! container starts each run in a network namespace of its own, where
//! `net.ipv4.ip_unprivileged_port_start` is zero, so an unprivileged
//! uid may bind port 67 there -- it is the host that refuses. A test
//! still names a port of the kernel's choosing, because a test that
//! took a well-known one would collide with the next test and with
//! whatever else is on the machine, and that is the whole of why
//! `--dhcp-port` exists.
//!
//! What *is* checked here, over loopback and on a port the kernel
//! chose: that the kernel reports the interface a datagram
//! arrived on and the address it was sent to; that an interface index
//! resolves to a name through a socket; that a datagram longer than
//! this front-end will parse arrives long enough to be refused rather
//! than cut down to a well-formed one; that naming the egress in a
//! message produces a message the kernel accepts; that the address and
//! prefix an answer is derived from come out of one address and are
//! refused when there are none or several; and the whole of how a
//! reply is addressed, which is a function of the reply and so needs
//! no socket at all.
//!
//! # One interface, two sockets
//!
//! A served interface gets a [`Receiver`] and a [`Sender`] from
//! [`open`], and they share the datagram socket between them: a reply
//! to a client that holds an address is sent from the port the request
//! arrived at, and a second socket could not bind that port beside the
//! first. The sender also holds a socket that puts a whole frame on
//! the link, which is the one thing in this crate that needs privilege
//! nothing else can supply.
//!
//! Each interface is read and answered in turn. That bounds what one
//! client can make this process do -- the number of requests being
//! decided at once is the number of interfaces an operator configured,
//! and no client can raise it -- and it is also what the reference
//! implementation does, being single-threaded. The load given up is
//! not load this protocol produces: a client speaks twice a lease
//! time, so four clients on a twelve-hour lease are eight datagrams a
//! day.

mod error;
mod frame;
mod interface;
mod receive;
mod send;
mod socket;

pub use error::{Error, Result};
pub use interface::{ETHERNET_ADDRESS_BYTES, Interfaces};
pub use receive::{Arrived, Receiver};
// `Delivery` and the function that chooses one are public so that the
// addressing of a reply can be read without a socket. The choice is
// RFC 2131 section 4.1's algorithm and it is the one part of the send
// path that is a judgement rather than a system call, so the
// differential rig asks for it directly: a case compares where a
// reply went as well as what it said, and nothing in this repository
// may open the socket that would otherwise answer that.
pub use send::{Delivery, Sender, delivery};

use crate::wire::MAX_DATAGRAM_BYTES;
use send::Served;
use socket::{PacketSocket, Shared, UdpSocket};

/// The namespace an interface that is in no namespace of its own is
/// attached under.
///
/// An attachment is identified by a namespace and an interface name,
/// the namespace is required, and a datapath with no namespaces has to
/// pass something -- so this is the word an operator types for an
/// ordinary interface on an ordinary machine, and the one a datapath
/// that does move between namespaces will have to agree with.
///
/// A word rather than a character that could not be a namespace, which
/// was the first choice and turned out not to be available: an
/// attachment's identity reaches an event as one string with its two
/// halves joined by a slash, and the store refuses that character in
/// either half rather than escaping it, so that `a/b` plus `c` and `a`
/// plus `b/c` cannot become one event. Not an empty string, which is
/// indistinguishable from a field somebody forgot to fill in, and not
/// a dot, which a consumer that ever put an attachment's identity in a
/// URL path would silently collapse.
///
/// This name can be a real namespace, and what that costs is a refusal
/// rather than a wrong answer. An attachment is keyed by its namespace
/// and its interface together, so an interface here and an identically
/// named interface inside a namespace called `root` claim one
/// identity, and the store refuses the second claim naming the holder.
/// Nothing leaks between networks and nothing is answered wrongly; an
/// operator is told they cannot attach that interface, and can rename
/// their namespace.
pub const UNNAMESPACED: &str = "root";

/// The protocol number an Ethernet frame carrying IPv4 declares.
const ETHERNET_PROTOCOL_IPV4: u16 = 0x0800;

/// The buffer one datagram is read into.
///
/// One octet more than the largest datagram this front-end will parse,
/// and the extra octet is load-bearing: with a buffer of exactly the
/// bound, the kernel would hand over a long datagram truncated to it,
/// which parses as a short well-formed one with whatever was cut off
/// -- the address the client asked for, say -- lost silently. With one
/// more, an over-long datagram is visibly over-long and is refused
/// unparsed.
pub const RECEIVE_BUFFER_BYTES: usize = MAX_DATAGRAM_BYTES + 1;

/// Open the sockets one interface is served by.
///
/// In order: the datagram socket, told which interface it serves and
/// then bound, because naming the interface after the bind does not
/// move a socket that is already in the port's table; and the socket a
/// frame leaves by. A failure carries the name of the call and the
/// kernel's own errno, which is what distinguishes a missing
/// capability from a port something else is already serving.
///
/// The interface's index is resolved here and kept, and that is not a
/// front-end remembering state: the socket is bound to the interface
/// by the same identity, so an index that went stale would be a socket
/// that had stopped receiving anything. Everything an answer is
/// *derived* from is read again for each request.
pub fn open(namespace: &str, ifname: &str, port: u16) -> Result<(Receiver, Sender)> {
    let interfaces = Interfaces::open()?;
    let ifindex = interfaces.index_of(ifname)?;

    let datagrams = UdpSocket::open()?;
    datagrams.serve_only(ifname)?;
    datagrams.bind(port)?;
    let datagrams = Shared::new(datagrams);
    let frames = PacketSocket::open()?;

    let receiver = Receiver::new(
        datagrams.clone(),
        Interfaces::open()?,
        ifname.to_owned(),
        ifindex,
    )?;
    let sender = Sender::new(
        (datagrams, frames),
        interfaces,
        Served {
            namespace: namespace.to_owned(),
            ifname: ifname.to_owned(),
            ifindex,
            port,
        },
    );

    Ok((receiver, sender))
}
