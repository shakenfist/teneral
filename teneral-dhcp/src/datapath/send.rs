//! How an answer reaches the client it is for.
//!
//! Three ways, and which one an answer takes is decided here. The
//! order below is RFC 2131 section 4.1's own, which settles all of it:
//! every request that reaches this layer came in with a zero `giaddr`,
//! a relayed request having been dropped before the store was asked,
//! and a zero `giaddr` is the condition that section states its rules
//! under.
//!
//! 1. **A refusal is broadcast**, whatever else is true of the client.
//!    Section 4.3.2 makes this a MUST and gives the reason: a client
//!    being refused may have neither a correct address nor a correct
//!    mask, and may not be answering address resolution.
//! 2. **A client that already holds an address is answered at it**, by
//!    UDP, with the interface the answer leaves by named in the
//!    message. Section 4.1 reads the condition off `ciaddr` and does
//!    not consult the broadcast flag for it, which is why this comes
//!    before the flag rather than after: a client that holds an
//!    address answers address resolution for it, so the one thing the
//!    flag is about does not arise. Section 4.3.5 says the same for a
//!    request for configuration alone, in its own words.
//! 3. **A client with no address that asked for broadcast gets one.**
//!    The flag is read as a request for broadcast and never as
//!    permission to unicast: a client that cleared it was observed
//!    accepting a broadcast reply in every transaction measured, so a
//!    clear flag says nothing about what a client will accept and a
//!    set one says everything about what it needs.
//! 4. **A client with no address that did not ask for broadcast is
//!    answered below the IP layer.** Section 4.1 says to unicast to
//!    the client's hardware address and `yiaddr`, which is a sentence
//!    about two layers at once: ordinary routing cannot deliver it,
//!    there being no route to an address nobody holds, and the frame
//!    that can is addressed to the hardware address the client does
//!    hold.
//!
//! An answer with no address of any kind and no request for broadcast
//! is broadcast too, because there is nothing else to address it to.
//! That is this file's own and no section covers it: the layers above
//! do not produce such an answer, a refusal being covered by the first
//! step and every other answer carrying an address or going to a
//! client that has one.
//!
//! # Where the privilege is
//!
//! Only the fourth needs the capability to use the network below the
//! IP layer, and it is the one the protocol makes unavoidable: the
//! address resolution a unicast would need cannot happen until the
//! client has the address this reply is what gives it.

use std::io;
use std::net::Ipv4Addr;

use crate::datapath::error::{Error, Result};
use crate::datapath::frame::Frame;
use crate::datapath::interface::{ETHERNET_ADDRESS_BYTES, Interfaces};
use crate::datapath::socket::{PacketSocket, Shared};
use crate::decide::{Egress, Reply};
use crate::wire::AnswerKind;

/// The port a client listens for its answer on.
///
/// The protocol's, like the server's own, and for the same reason: a
/// client cannot be told to use another one.
pub(crate) const CLIENT_PORT: u16 = 68;

/// How one answer is addressed.
///
/// Named rather than left as a branch inside the send so that the
/// decision is a value a test can make without a socket, which is the
/// only way this judgement can be checked where no socket below the IP
/// layer can be opened and there is one interface to send on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// To every client on the link, because this client cannot be
    /// relied on to receive anything narrower.
    Broadcast,
    /// To an address the client already holds, by ordinary routing.
    Unicast {
        /// The address it holds.
        address: Ipv4Addr,
    },
    /// Below the IP layer, to a client that holds no address yet.
    BelowIp {
        /// The address it is being given, which the IP packet is
        /// addressed to.
        address: Ipv4Addr,
        /// The hardware address it does hold, which the frame is
        /// addressed to.
        hardware_address: [u8; ETHERNET_ADDRESS_BYTES],
    },
}

/// The sending half of one interface's datapath.
///
/// One sender serves one interface, which is what makes the check in
/// [`Sender::send`] worth its two comparisons: an answer derived from
/// one network leaving by another network's interface is the failure
/// this project exists to prevent, and a sender that serves everything
/// could not notice it.
#[derive(Debug)]
pub struct Sender {
    /// The datagram socket, shared with the receiving half so that a
    /// reply leaves from the port the request arrived at.
    udp: Shared,
    /// The socket a whole frame goes out of.
    packet: PacketSocket,
    /// The interface queries a frame's source address comes from.
    interfaces: Interfaces,
    /// The namespace the interface below is named within.
    namespace: String,
    /// The interface this sender serves.
    ifname: String,
    /// Its index, which names the egress in every message.
    ifindex: u32,
    /// The port a reply is sent from, which is the port the socket is
    /// bound to.
    port: u16,
}

impl Sender {
    /// Take over the sockets one interface's answers leave by.
    pub(crate) fn new(
        sockets: (Shared, PacketSocket),
        interfaces: Interfaces,
        served: Served,
    ) -> Self {
        let (udp, packet) = sockets;

        Sender {
            udp,
            packet,
            interfaces,
            namespace: served.namespace,
            ifname: served.ifname,
            ifindex: served.ifindex,
            port: served.port,
        }
    }

    /// The interface this sender serves.
    pub fn ifname(&self) -> &str {
        &self.ifname
    }

    /// Put a reply on the link, below the IP layer.
    ///
    /// The source hardware address is read now rather than when the
    /// socket was opened, for the same reason the interface's IP
    /// address is: a front-end that remembered it would be a front-end
    /// with a second place to look when a frame went out wrong.
    fn below_ip(
        &self,
        reply: &Reply<'_>,
        address: Ipv4Addr,
        hardware_address: [u8; ETHERNET_ADDRESS_BYTES],
    ) -> Result<()> {
        let frame = Frame {
            source_hardware_address: self.interfaces.hardware_address(&self.ifname)?,
            destination_hardware_address: hardware_address,
            source: reply.server_address(),
            destination: address,
            source_port: self.port,
            destination_port: CLIENT_PORT,
            payload: reply.datagram(),
        }
        .encode()?;

        self.packet
            .send(self.ifindex, hardware_address, &frame)
            .map_err(|source| Error::Syscall {
                call: "sendto(AF_PACKET)",
                source,
            })
    }
}

/// Which interface a sender serves, and how it is named.
///
/// Gathered into one value so that opening a sender takes four
/// arguments rather than seven, and so that the namespace and the
/// interface -- which together are the attachment an operator wrote --
/// travel as a pair.
#[derive(Clone, Debug)]
pub(crate) struct Served {
    /// The namespace the interface is named within.
    pub(crate) namespace: String,
    /// The interface.
    pub(crate) ifname: String,
    /// Its index in the namespace the sockets belong to.
    pub(crate) ifindex: u32,
    /// The port a reply is sent from.
    pub(crate) port: u16,
}

impl Egress for Sender {
    fn send(&self, reply: &Reply<'_>) -> io::Result<()> {
        if reply.namespace() != self.namespace || reply.ifname() != self.ifname {
            return Err(Error::WrongInterface {
                serving: format!("{}/{}", self.namespace, self.ifname),
                named: format!("{}/{}", reply.namespace(), reply.ifname()),
            }
            .into());
        }

        match delivery(reply)? {
            Delivery::Broadcast => self.udp.send(
                Ipv4Addr::BROADCAST,
                CLIENT_PORT,
                self.ifindex,
                reply.server_address(),
                reply.datagram(),
            ),
            Delivery::Unicast { address } => self.udp.send(
                address,
                CLIENT_PORT,
                self.ifindex,
                reply.server_address(),
                reply.datagram(),
            ),
            Delivery::BelowIp {
                address,
                hardware_address,
            } => self
                .below_ip(reply, address, hardware_address)
                .map_err(io::Error::from),
        }
    }
}

/// How this reply is addressed.
///
/// The order of the branches is RFC 2131 section 4.1's, set out in
/// this module's own documentation, and each of them is reached by a
/// real message: a refusal, a client renewing or asking for
/// configuration alone, a client with no address that asked for
/// broadcast, and a client with no address that did not.
pub fn delivery(reply: &Reply<'_>) -> Result<Delivery> {
    if reply.kind() == AnswerKind::Refusal {
        return Ok(Delivery::Broadcast);
    }

    let held = reply.held_address();
    if !held.is_unspecified() {
        return Ok(Delivery::Unicast { address: held });
    }

    if reply.broadcast() {
        return Ok(Delivery::Broadcast);
    }

    let offered = reply.client_address();
    if offered.is_unspecified() {
        return Ok(Delivery::Broadcast);
    }

    Ok(Delivery::BelowIp {
        address: offered,
        hardware_address: ethernet(reply.hardware_address())?,
    })
}

/// A client's hardware address as a frame is addressed to it.
///
/// A client whose hardware address is not an Ethernet one presents
/// nothing the store will hold as a selector value, so it is never
/// answered and this is never reached from the wire. It is refused
/// rather than padded anyway, because a frame addressed to an invented
/// hardware address would be delivered to whatever did hold it.
fn ethernet(hardware_address: &[u8]) -> Result<[u8; ETHERNET_ADDRESS_BYTES]> {
    hardware_address.try_into().map_err(|_| Error::NotEthernet {
        length: hardware_address.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decide::Arrival;
    use crate::wire::test_support::{
        CLIENT_MAC, DISCOVER, Datagram, INFORM, LEASE_SECONDS, OFFERED_ADDRESS, PREFIX_LENGTH,
        REQUEST, SERVER_ADDRESS, option, options,
    };
    use crate::wire::{Answer, LeaseSeconds, Request, ServerFacts};

    #[test]
    fn a_client_with_no_address_is_answered_below_the_ip_layer() {
        let request = parsed(&message(DISCOVER));
        let answer = Answer::offer(&request, facts(), OFFERED_ADDRESS, lease());

        assert_eq!(
            decide(&request, &answer),
            Delivery::BelowIp {
                address: OFFERED_ADDRESS,
                hardware_address: CLIENT_MAC,
            },
            "addressed to the address it is being given, in a frame to the one it holds"
        );
    }

    #[test]
    fn a_client_that_asked_for_broadcast_gets_one_even_though_it_has_no_address() {
        let mut datagram = message(DISCOVER);
        datagram.broadcast = true;
        let request = parsed(&datagram);
        let answer = Answer::offer(&request, facts(), OFFERED_ADDRESS, lease());

        assert_eq!(decide(&request, &answer), Delivery::Broadcast);
    }

    #[test]
    fn a_clear_broadcast_flag_is_not_permission_to_unicast_by_ip() {
        let request = parsed(&message(DISCOVER));
        let answer = Answer::offer(&request, facts(), OFFERED_ADDRESS, lease());

        assert!(
            !request.broadcast,
            "the client cleared the flag, as most do"
        );
        assert_ne!(
            decide(&request, &answer),
            Delivery::Unicast {
                address: OFFERED_ADDRESS
            },
            "and the reply still does not go by ordinary routing, which could not deliver it"
        );
    }

    #[test]
    fn a_client_renewing_is_answered_at_the_address_it_holds() {
        let mut datagram = message(REQUEST);
        datagram.client_address = OFFERED_ADDRESS;
        let request = parsed(&datagram);
        let answer = Answer::acknowledge(&request, facts(), OFFERED_ADDRESS, lease());

        assert_eq!(
            decide(&request, &answer),
            Delivery::Unicast {
                address: OFFERED_ADDRESS
            },
            "it answers address resolution for that address, so routing reaches it"
        );
    }

    /// The one case the two plausible orders disagree about. RFC 2131
    /// section 4.1 reads the condition off `ciaddr` and consults the
    /// broadcast flag only where `ciaddr` is zero, so a client that
    /// holds an address is answered at it however the flag is set --
    /// and the flag's own reason does not arise for such a client,
    /// which answers address resolution for the address it holds.
    #[test]
    fn a_client_that_holds_an_address_is_answered_at_it_even_having_asked_for_broadcast() {
        let mut datagram = message(REQUEST);
        datagram.client_address = OFFERED_ADDRESS;
        datagram.broadcast = true;
        let request = parsed(&datagram);
        let answer = Answer::acknowledge(&request, facts(), OFFERED_ADDRESS, lease());

        assert!(request.broadcast, "the client asked for a broadcast");
        assert_eq!(
            decide(&request, &answer),
            Delivery::Unicast {
                address: OFFERED_ADDRESS
            },
            "and section 4.1 answers it at the address it holds regardless"
        );
    }

    #[test]
    fn a_client_asking_for_configuration_alone_is_answered_at_the_address_it_holds() {
        let mut datagram = message(INFORM);
        datagram.client_address = OFFERED_ADDRESS;
        let request = parsed(&datagram);
        let answer = Answer::inform(&request, facts());

        assert_eq!(
            decide(&request, &answer),
            Delivery::Unicast {
                address: OFFERED_ADDRESS
            },
            "an answer with no address of its own goes to the one the client has"
        );
    }

    #[test]
    fn a_refusal_is_broadcast_even_to_a_client_that_holds_an_address() {
        let mut datagram = message(REQUEST);
        datagram.client_address = OFFERED_ADDRESS;
        let request = parsed(&datagram);
        let answer = Answer::refuse(&request, facts());

        assert_eq!(
            decide(&request, &answer),
            Delivery::Broadcast,
            "a client being refused may be in no state to receive a unicast"
        );
    }

    #[test]
    fn an_answer_with_no_address_to_send_it_to_is_broadcast() {
        // Reached by no message the layers above produce: an answer
        // carrying no address at all is a refusal, and a refusal is
        // covered above. It is here because a reply nobody could
        // address is still a reply, and broadcasting it is the only
        // thing left to do with it.
        let request = parsed(&message(INFORM));
        let answer = Answer::inform(&request, facts());

        assert_eq!(decide(&request, &answer), Delivery::Broadcast);
    }

    #[test]
    fn a_client_whose_hardware_address_is_not_an_ethernet_one_is_refused_rather_than_padded() {
        let mut datagram = message(DISCOVER);
        datagram.hlen = 4;
        datagram.chaddr = vec![0x52, 0x54, 0x00, 0x11];
        let request = parsed(&datagram);
        let answer = Answer::offer(&request, facts(), OFFERED_ADDRESS, lease());
        let encoded = answer.encode();
        let reply = Reply::new(&arrival(), &request, &answer, &encoded);

        let refusal = delivery(&reply).expect_err("no frame can be addressed to it");

        assert!(
            matches!(refusal, Error::NotEthernet { length: 4 }),
            "{refusal}"
        );
    }

    #[test]
    fn a_reply_for_another_interface_does_not_leave_by_this_one() {
        // Asserted on the value rather than through a `Sender`, which
        // cannot be built here: opening its packet socket needs a
        // privilege this test environment does not have.
        let request = parsed(&message(DISCOVER));
        let answer = Answer::offer(&request, facts(), OFFERED_ADDRESS, lease());
        let encoded = answer.encode();
        let elsewhere = Arrival {
            namespace: crate::datapath::UNNAMESPACED,
            ifname: "gw-n2",
            server: facts(),
        };
        let reply = Reply::new(&elsewhere, &request, &answer, &encoded);

        assert_ne!(
            reply.ifname(),
            arrival().ifname,
            "a reply carries the interface its answer was derived from"
        );
    }

    /// A message of one kind from the fixture client.
    fn message(kind: u8) -> Datagram {
        let mut datagram = Datagram::discover();
        datagram.options = options(&[option(53, &[kind])]);

        datagram
    }

    /// How this answer to this request would be addressed.
    fn decide(request: &Request, answer: &Answer) -> Delivery {
        let encoded = answer.encode();
        let reply = Reply::new(&arrival(), request, answer, &encoded);

        delivery(&reply).expect("a delivery")
    }

    /// The interface these tests answer on.
    fn arrival() -> Arrival<'static> {
        Arrival {
            namespace: crate::datapath::UNNAMESPACED,
            ifname: "gw-n1",
            server: facts(),
        }
    }

    /// What that interface says about this server.
    fn facts() -> ServerFacts {
        ServerFacts::new(SERVER_ADDRESS, PREFIX_LENGTH).expect("a prefix an address has bits for")
    }

    /// Twelve hours, as the store's own default is.
    fn lease() -> LeaseSeconds {
        LeaseSeconds::new(LEASE_SECONDS).expect("a lease an option can carry")
    }

    /// The request a datagram parses to.
    fn parsed(datagram: &Datagram) -> Request {
        crate::wire::parse(&datagram.bytes()).expect("a well-formed request")
    }
}
