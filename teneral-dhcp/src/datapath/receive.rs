//! Where a request arrived, as the kernel reports it.
//!
//! One socket serves one interface, and the socket is bound to the
//! wildcard address because a client with no address can only send to
//! `255.255.255.255`. That leaves two things about a datagram that the
//! datagram itself does not say, and the kernel is asked for both: the
//! interface it arrived on, and the address it was sent to.
//!
//! The arrival interface is what decides which network a request
//! belongs to, and so which network's answer a client gets. Nothing
//! else can decide it: two networks may hold the same addresses, and
//! the same client may be on both. The index the kernel reports is
//! resolved to a name through a socket rather than through
//! `/sys/class/net`, and the name is checked against the interface
//! this socket was opened to serve -- which costs two system calls a
//! datagram and makes the interface the kernel's answer rather than a
//! string somebody typed.
//!
//! The destination address is carried because it distinguishes a
//! broadcast from a unicast without parsing anything, which is how a
//! client that already has an address is told from one that does not
//! before any decision is made about either.

use std::io;
use std::net::Ipv4Addr;

use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

use crate::datapath::RECEIVE_BUFFER_BYTES;
use crate::datapath::error::{Error, Result};
use crate::datapath::interface::Interfaces;
use crate::datapath::socket::Shared;
use crate::wire::ServerFacts;

/// What the kernel said about a datagram, beside the datagram itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrived {
    /// The index of the interface it arrived on, which is only
    /// meaningful in this socket's own namespace.
    pub ifindex: u32,
    /// The address it was sent to: the limited broadcast from a client
    /// with no address, and the interface's own address from a client
    /// that has one.
    pub destination: Ipv4Addr,
}

/// The receiving half of one interface's datapath.
///
/// Reads one datagram at a time, which is a bound on the work a client
/// can cause rather than a limitation: the number of requests being
/// decided at once is the number of interfaces an operator configured,
/// and no client can raise it.
#[derive(Debug)]
pub struct Receiver {
    /// The socket, registered with the runtime so that waiting for a
    /// datagram parks no thread.
    socket: AsyncFd<Shared>,
    /// The interface queries this path makes, in the namespace the
    /// socket belongs to.
    interfaces: Interfaces,
    /// The interface this socket was opened to serve.
    ifname: String,
    /// Its index, as resolved when the socket was opened.
    ifindex: u32,
}

impl Receiver {
    /// Take over a socket that is already bound and told which
    /// interface it serves.
    ///
    /// Registers it for readable readiness only, because nothing waits
    /// to write here: a reply is sent as the answer is decided and a
    /// transmit queue with no room is reported rather than waited for.
    /// Must be called with a runtime running, which is where the
    /// daemon opens its sockets.
    pub(crate) fn new(
        socket: Shared,
        interfaces: Interfaces,
        ifname: String,
        ifindex: u32,
    ) -> Result<Self> {
        // SAFETY: the socket owns its descriptor and keeps it open
        // and unchanged for as long as any share of it lives, and
        // this registration holds one of those shares. Its
        // `as_raw_fd` returns that one descriptor and nothing else.
        let registered = unsafe { AsyncFd::register_with_interest(socket, Interest::READABLE) };

        Ok(Receiver {
            socket: registered.map_err(|error| Error::Syscall {
                call: "registering a socket with the runtime",
                source: error.into_parts().1,
            })?,
            interfaces,
            ifname,
            ifindex,
        })
    }

    /// The interface this receiver serves.
    pub fn ifname(&self) -> &str {
        &self.ifname
    }

    /// The index of the interface this receiver serves.
    pub fn ifindex(&self) -> u32 {
        self.ifindex
    }

    /// The name the kernel has for the interface at `index`.
    ///
    /// Through an `ioctl` on a socket, so that the answer is about the
    /// namespace the socket belongs to.
    pub fn name_of(&self, index: u32) -> Result<String> {
        self.interfaces.name_of(index)
    }

    /// The address and prefix the served interface holds now.
    ///
    /// Read for each request rather than kept, so that this front-end
    /// holds no state an operator could change without it taking
    /// effect.
    pub fn facts(&self) -> Result<ServerFacts> {
        self.interfaces.facts(&self.ifname)
    }

    /// Wait for one datagram, and leave `buffer` holding exactly the
    /// octets that arrived.
    ///
    /// The buffer is grown to one octet more than the largest datagram
    /// this front-end will parse. That extra octet is what makes an
    /// over-long datagram a datagram that is too long: with a buffer
    /// of exactly the bound, the kernel would hand over a truncated
    /// datagram that parses as a short well-formed one, and whatever
    /// was cut off -- which can be the address the client asked for --
    /// would be lost silently.
    pub async fn receive(&self, buffer: &mut Vec<u8>) -> io::Result<Arrived> {
        buffer.clear();
        buffer.resize(RECEIVE_BUFFER_BYTES, 0);

        let receipt = self
            .socket
            .async_io(Interest::READABLE, |socket| socket.receive(buffer))
            .await?;

        buffer.truncate(receipt.length);

        Ok(Arrived {
            ifindex: receipt.ifindex,
            destination: receipt.destination,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::net::UdpSocket as StdSocket;

    use super::*;
    use crate::datapath::socket::UdpSocket;
    use crate::wire::MAX_DATAGRAM_BYTES;

    /// A receiver on loopback, set up exactly as the daemon sets one
    /// up, at a port the kernel chose rather than the protocol's.
    ///
    /// The port is the one difference, and it is the reason the daemon
    /// takes a port at all: a test that bound the protocol's port
    /// would conflict with the next test, and with anything else on
    /// the machine. Everything else -- the options, naming the
    /// interface, the wildcard bind -- is what runs in a deployment.
    ///
    /// What cannot be reached from here is the reply to a client with
    /// no address, which needs a socket below the IP layer, and two
    /// sockets on one port, which needs a second interface. Loopback
    /// is the only interface this environment is promised.
    fn on_loopback() -> (Receiver, u16) {
        let socket = UdpSocket::open().expect("a datagram socket");
        socket.serve_only("lo").expect(
            "naming the interface a socket serves needs no privilege on this kernel; if this \
             fails, the capability set the daemon documents has grown",
        );
        socket.bind(0).expect("the wildcard at a port we may have");
        let port = socket.port().expect("the port the kernel chose");
        let interfaces = Interfaces::open().expect("a socket to ask through");
        let index = interfaces.index_of("lo").expect("loopback has an index");
        let receiver = Receiver::new(Shared::new(socket), interfaces, "lo".to_owned(), index)
            .expect("a receiver");

        (receiver, port)
    }

    /// Two sockets serving the same interface at the same port are a
    /// conflict, and that is the arrangement rather than an
    /// inconvenience: the separation between two overlapping networks
    /// is the kernel's to enforce, which it can only do while each
    /// socket receives one interface's traffic. An option that made
    /// this bind succeed -- either of the two that share a port --
    /// would have the kernel spread datagrams across the sockets
    /// instead, and one network would answer another's client.
    ///
    /// The case this cannot reach is the useful one: two sockets on
    /// one port naming *different* interfaces, which coexist. That
    /// needs a second interface, and there is none here.
    #[test]
    fn two_sockets_serving_one_interface_may_not_share_a_port() {
        let first = UdpSocket::open().expect("a datagram socket");
        first.serve_only("lo").expect("naming the interface");
        first.bind(0).expect("a port we may have");
        let port = first.port().expect("the port the kernel chose");

        let second = UdpSocket::open().expect("a second datagram socket");
        second.serve_only("lo").expect("naming the interface");
        let refusal = second.bind(port).expect_err("the port is taken");

        assert!(
            matches!(
                &refusal,
                Error::Syscall { call, source }
                    if *call == "bind" && source.kind() == io::ErrorKind::AddrInUse
            ),
            "the kernel refuses the second bind: {refusal}"
        );
    }

    #[tokio::test]
    async fn a_datagram_reports_the_interface_it_arrived_on_and_the_address_it_was_sent_to() {
        let (receiver, port) = on_loopback();
        let client = StdSocket::bind("127.0.0.1:0").expect("a client socket");
        client
            .send_to(b"a datagram", ("127.0.0.1", port))
            .expect("a datagram sent to the receiver");

        let mut buffer = Vec::new();
        let arrived = receiver.receive(&mut buffer).await.expect("a datagram");

        assert_eq!(buffer.as_slice(), b"a datagram");
        assert_eq!(
            arrived.ifindex,
            receiver.ifindex(),
            "the kernel names the interface it arrived on"
        );
        assert_eq!(
            receiver
                .name_of(arrived.ifindex)
                .expect("an index has a name"),
            "lo",
            "and that index resolves to a name through a socket"
        );
        assert_eq!(
            arrived.destination,
            Ipv4Addr::LOCALHOST,
            "and the address it was sent to, which a broadcast would differ in"
        );
    }

    #[tokio::test]
    async fn a_datagram_over_the_size_bound_arrives_long_enough_to_be_refused() {
        let (receiver, port) = on_loopback();
        let client = StdSocket::bind("127.0.0.1:0").expect("a client socket");
        let oversize = vec![0xaa_u8; MAX_DATAGRAM_BYTES * 2];
        client
            .send_to(&oversize, ("127.0.0.1", port))
            .expect("a datagram larger than anything this front-end parses");

        let mut buffer = Vec::new();
        receiver.receive(&mut buffer).await.expect("a datagram");

        assert_eq!(
            buffer.len(),
            MAX_DATAGRAM_BYTES + 1,
            "it arrives one octet past the bound rather than cut to it"
        );
        let refusal = crate::wire::parse(&buffer).expect_err("and so is refused unparsed");
        assert!(
            matches!(refusal, crate::wire::Error::TooLarge { .. }),
            "as too large rather than as malformed: {refusal}"
        );
    }

    #[tokio::test]
    async fn the_egress_is_named_in_the_message_rather_than_on_the_socket() {
        let (receiver, port) = on_loopback();
        let sender = UdpSocket::open().expect("a datagram socket");
        sender.bind(0).expect("a port we may have");
        let egress = receiver.ifindex();

        sender
            .send(
                Ipv4Addr::LOCALHOST,
                port,
                egress,
                Ipv4Addr::LOCALHOST,
                b"by lo",
            )
            .expect("a datagram sent out of a named interface");

        let mut buffer = Vec::new();
        let arrived = receiver.receive(&mut buffer).await.expect("a datagram");

        assert_eq!(buffer.as_slice(), b"by lo");
        assert_eq!(arrived.ifindex, egress);
    }
}
