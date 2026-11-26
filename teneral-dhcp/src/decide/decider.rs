//! Which answer a request gets, and from which network.
//!
//! The dispatch below is the whole of the front-end's behaviour, so
//! the order of its steps is written out rather than left to be
//! reconstructed:
//!
//! 1. a relayed request is dropped, before the store is asked
//!    anything, because answering one needs a subnet model this
//!    front-end does not have;
//! 2. the arrival interface is resolved to a network, and an interface
//!    no network has claimed is a refusal with a name on it;
//! 3. what the client presents becomes an identity the store will
//!    match, with any attribute the store would not hold left out and
//!    recorded;
//! 4. the message decides the rest.
//!
//! Every question after step 2 names that network. There is no path
//! through this file that asks the store about an address or a client
//! without one.
//!
//! # What each message gets
//!
//! | Message | Answer |
//! |---------|--------|
//! | DISCOVER | an OFFER, of the address a binding reserves for this client if one does, and otherwise of the address the store chooses |
//! | REQUEST naming this server | an ACK if the address it names is one this network may give this client, and a NAK if it is not |
//! | REQUEST naming another server | nothing: it is not this server's to answer |
//! | REQUEST naming no server, with `ciaddr` set | an ACK from a renewal, and a NAK when the store refuses because the lease has lapsed or another client holds it |
//! | REQUEST naming no server, with the address in option 50 | the same, except that a client this network has no record of at all gets silence, which RFC 2131 section 4.3.2 makes a MUST |
//! | RELEASE | the lease revoked, and no reply, the protocol specifying none |
//! | DECLINE | the lease revoked, a line in the log, and no reply |
//! | INFORM | an ACK carrying the derived configuration, with no address, no lease time and no lease written |
//!
//! Two of those are deliberately the cheap answer, and both are
//! recorded as such where the daemon's behaviour is documented rather
//! than only here. A DECLINE revokes the lease and logs, so the
//! address returns to the pool and may be offered again, which loops
//! if the conflict is permanent; stopping it needs an exclusion the
//! store does not hold. And a relayed request is dropped rather than
//! served, which is a refusal and not an omission.
//!
//! # The one question the store has no method for
//!
//! A REQUEST names an address, and whether the client may have it is
//! not the question the store's allocation answers. `allocate_lease`
//! *chooses* an address for a client that has none and refuses one a
//! binding reserves or that lies outside every range;
//! `acquire_lease` asserts an address somebody else chose and
//! deliberately consults neither, because its other caller is an
//! operator configuring a reservation outside every pool. Neither is
//! "may this client have the address it just asked for".
//!
//! So that question is answered here, from facts read out of the
//! store: the binding that reserves an address for this client, the
//! network's ranges compared using the store's own containment, and
//! the addresses this network's other bindings reserve. It is not a
//! second implementation of selection -- nothing here chooses an
//! address -- and the reason it cannot be skipped is that
//! `acquire_lease` would otherwise hand a client an address outside
//! every pool, or one held for a machine that is switched off.

use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, PoisonError};

use teneral_store::{
    AddressRange, Binding, BindingStore, ClientIdentity, Error as StoreError, Network,
};

use crate::decide::ACTOR;
use crate::decide::egress::{Egress, Reply};
use crate::decide::error::{Error, Result};
use crate::decide::identity;
use crate::decide::outcome::{Decided, Note, Outcome, Silence};
use crate::wire::{Answer, LeaseSeconds, Request, RequestKind, ServerFacts};

/// How many relays the log suppression remembers.
///
/// Bounded, and the bound is the point rather than tidiness: the key
/// comes out of a packet, so a record with no limit is memory whose
/// size a sender chooses. At the limit the oldest entry is forgotten,
/// which may cost a second line about a relay last seen long ago --
/// the cheaper of the two failures by a wide margin.
const RELAYS_REMEMBERED: usize = 64;

/// Where a request arrived, and what that interface says about this
/// server.
///
/// The namespace and the interface name are the attachment's identity
/// in the store, and the interface with no namespace of its own is
/// attached under `root`. [`ServerFacts`] is the address and prefix the
/// interface was observed to hold, which is where four of the nine
/// options an answer carries come from. It is a parameter rather than
/// something this layer holds because reading an interface is
/// observation and not state: nothing is remembered between packets,
/// which is what keeps a front-end free of per-network state of its
/// own.
#[derive(Clone, Copy, Debug)]
pub struct Arrival<'a> {
    /// The namespace the interface is named within.
    pub namespace: &'a str,
    /// The interface the datagram arrived on.
    pub ifname: &'a str,
    /// What that interface says about this server.
    pub server: ServerFacts,
}

/// The front-end's decisions, over one store.
///
/// Holds no per-network state. The one thing it remembers between
/// packets is which relays have already been named in the log, which
/// is not an answer to anything and is keyed by the interface as well
/// as by the relay.
pub struct Decider<E: Egress> {
    /// The source of truth for every answer.
    store: Arc<dyn BindingStore>,
    /// Where an answer goes.
    egress: E,
    /// The relays already reported.
    relays: Mutex<Reported>,
}

impl<E: Egress> Decider<E> {
    /// Decide for this store, and send through this egress.
    pub fn new(store: Arc<dyn BindingStore>, egress: E) -> Self {
        Decider {
            store,
            egress,
            relays: Mutex::new(Reported::default()),
        }
    }

    /// The egress this decider sends through.
    ///
    /// For the datapath, which owns the socket it passed in, and for a
    /// test, which reads what the recorder it passed in was given.
    pub fn egress(&self) -> &E {
        &self.egress
    }

    /// Answer `request`, which arrived at `arrival`.
    ///
    /// The answer, if there is one, has already been handed to the
    /// egress by the time this returns: there is no state in which
    /// this layer has decided to acknowledge an address and the client
    /// has not been told, because the store commits durably before it
    /// returns a lease and the send follows the commit.
    pub async fn answer(&self, arrival: &Arrival<'_>, request: &Request) -> Result<Decided> {
        // Carried up from the parser, which drops an option whose
        // value will not decode and answers anyway. The code is worth
        // a line because a binding configured against that option will
        // not match, and "the client did not send it" and "we could
        // not read it" are different diagnoses.
        let mut notes: Vec<Note> = request
            .not_understood
            .iter()
            .map(|dropped| Note::OptionNotUnderstood { code: dropped.code })
            .collect();

        if !request.relay_address.is_unspecified() {
            let relay = request.relay_address;
            if self.first_sighting_of(arrival, relay) {
                notes.push(Note::RelayedRequestDropped { relay });
            }

            return Ok(Decided {
                outcome: Outcome::Silent(Silence::Relayed { relay }),
                notes,
            });
        }

        // The interface is the whole of what says which network a
        // packet belongs to, and two networks may legitimately hold
        // the same addresses, so nothing below this line is asked
        // without the network this returned.
        let claimed = self
            .store
            .interface_owner(arrival.namespace, arrival.ifname)
            .await?;
        let Some(attachment) = claimed else {
            notes.push(Note::InterfaceNotClaimed {
                namespace: arrival.namespace.to_owned(),
                ifname: arrival.ifname.to_owned(),
            });

            return Ok(Decided {
                outcome: Outcome::Silent(Silence::InterfaceNotClaimed),
                notes,
            });
        };
        let network = attachment.network.as_str();

        let presented = identity::presented(request);
        let unidentifiable = presented.is_empty();
        notes.extend(presented.notes);

        // An INFORM is answered without asking the store about the
        // client at all -- every option it carries comes from the
        // interface -- so it is the one message an unidentifiable
        // client can still be answered.
        if unidentifiable && request.kind != RequestKind::Inform {
            notes.push(Note::ClientNotIdentifiable);

            return Ok(Decided {
                outcome: Outcome::Silent(Silence::ClientNotIdentifiable),
                notes,
            });
        }
        let client = &presented.identity;

        let outcome = match request.kind {
            RequestKind::Discover => {
                self.discover(arrival, request, network, client, &mut notes)
                    .await?
            }
            RequestKind::Request => match request.server_identifier {
                Some(server) => {
                    self.selecting(arrival, request, network, client, server, &mut notes)
                        .await?
                }
                None => {
                    self.renewing(arrival, request, network, client, &mut notes)
                        .await?
                }
            },
            RequestKind::Release => {
                self.giving_up(request, network, client, GivingUp::Released, &mut notes)
                    .await?
            }
            RequestKind::Decline => {
                self.giving_up(request, network, client, GivingUp::Declined, &mut notes)
                    .await?
            }
            // No lease, no lease time and no address: the client has
            // one already and asked for the rest of the configuration.
            RequestKind::Inform => {
                self.send(arrival, request, &Answer::inform(request, arrival.server))?
            }
        };

        Ok(Decided { outcome, notes })
    }

    /// A DISCOVER: offer this client an address in this network.
    async fn discover(
        &self,
        arrival: &Arrival<'_>,
        request: &Request,
        network: &str,
        client: &ClientIdentity,
        notes: &mut Vec<Note>,
    ) -> Result<Outcome> {
        let view = self.store.read().await?;
        let lease = lease_time(network, view.network(network).await?)?;
        let reserved = view
            .lookup_binding_on_interface(network, arrival.namespace, arrival.ifname, client)
            .await?;
        drop(view);

        let taken = match &reserved {
            // A reservation is an address an operator chose, so it is
            // asserted rather than allocated: it need not lie inside
            // any of the network's ranges, and the pool would not
            // offer it in any case.
            Some(binding) => {
                self.store
                    .acquire_lease(
                        &ACTOR,
                        network,
                        client,
                        binding.address,
                        i64::from(lease.seconds()),
                    )
                    .await
            }
            // Nothing configured, so the store chooses -- in the
            // specified order, honouring what the client asked for
            // only at its place in it.
            None => {
                self.store
                    .allocate_lease(
                        &ACTOR,
                        network,
                        client,
                        request.requested_address,
                        i64::from(lease.seconds()),
                    )
                    .await
            }
        };

        let offered = match taken {
            Ok((_, lease)) => lease.address,
            // The two refusals a network can honestly make: its pool
            // is empty or it has none, and the address reserved for
            // this client is in use by somebody else. Neither is a
            // reason to answer, and a server with nothing to offer
            // says nothing, which is what the protocol expects.
            Err(refused)
                if matches!(
                    refused,
                    StoreError::NoAddress { .. } | StoreError::Conflict { .. }
                ) =>
            {
                notes.push(Note::NothingToOffer {
                    network: network.to_owned(),
                    detail: refused.to_string(),
                });

                return Ok(Outcome::Silent(Silence::NothingToOffer));
            }
            Err(other) => return Err(other.into()),
        };

        self.send(
            arrival,
            request,
            &Answer::offer(request, arrival.server, offered, lease),
        )
    }

    /// A REQUEST that names a server: the client is accepting one
    /// particular offer.
    async fn selecting(
        &self,
        arrival: &Arrival<'_>,
        request: &Request,
        network: &str,
        client: &ClientIdentity,
        server: Ipv4Addr,
        notes: &mut Vec<Note>,
    ) -> Result<Outcome> {
        // Another server on the same link was selected. Not a fault
        // and not worth a line: this is the protocol working, and a
        // log line per client of every other server on the bridge
        // would bury everything that is a fault.
        if server != arrival.server.address() {
            return Ok(Outcome::Silent(Silence::AnotherServerSelected { server }));
        }

        let Some(wanted) = request.requested_address else {
            notes.push(Note::AddressRefused {
                network: network.to_owned(),
                address: Ipv4Addr::UNSPECIFIED,
                detail: "the request selected this server and named no address".to_owned(),
            });

            return self.refuse(arrival, request);
        };

        let view = self.store.read().await?;
        let lease = lease_time(network, view.network(network).await?)?;
        let reserved = view
            .lookup_binding_on_interface(network, arrival.namespace, arrival.ifname, client)
            .await?;
        let refusal = match &reserved {
            // An operator who reserved an address for this client said
            // which address it may have. Asking for another one in a
            // network where it has a reservation is refused rather
            // than granted alongside it.
            Some(binding) if binding.address == wanted => None,
            Some(binding) => Some(format!(
                "this network reserves {} for this client",
                binding.address
            )),
            None => {
                let ranges = view.address_ranges(network).await?;
                let bindings = view.bindings(network).await?;

                why_not_allocatable(wanted, &ranges, &bindings)
            }
        };
        drop(view);

        if let Some(detail) = refusal {
            notes.push(Note::AddressRefused {
                network: network.to_owned(),
                address: wanted,
                detail,
            });

            return self.refuse(arrival, request);
        }

        match self
            .store
            .acquire_lease(&ACTOR, network, client, wanted, i64::from(lease.seconds()))
            .await
        {
            Ok(_) => self.send(
                arrival,
                request,
                &Answer::acknowledge(request, arrival.server, wanted, lease),
            ),
            // Another client holds it in this network. The client is
            // told so rather than left to time out, which is what
            // sends it back to a DISCOVER.
            Err(refused) if matches!(refused, StoreError::Conflict { .. }) => {
                notes.push(Note::AddressRefused {
                    network: network.to_owned(),
                    address: wanted,
                    detail: refused.to_string(),
                });

                self.refuse(arrival, request)
            }
            Err(other) => Err(other.into()),
        }
    }

    /// A REQUEST that names no server: the client is renewing,
    /// rebinding, or checking after a reboot that what it remembers is
    /// still its own.
    ///
    /// Three states and one path, because the store answers all three
    /// with the same question -- is there a live lease on this address
    /// in this network, held by this client. Where they part company
    /// is what a refusal means, and that division is RFC 2131 section
    /// 4.3.2's rather than this project's. A client with `ciaddr` set
    /// is telling this server it holds an address, so a network that
    /// does not agree says so. A client rebooting is *asking*, and
    /// where this network has no record of the client at all the
    /// section requires silence; see
    /// [`Silence::NoRecordOfThisClient`] for the reason, which is
    /// about what else is on the wire rather than about this client.
    async fn renewing(
        &self,
        arrival: &Arrival<'_>,
        request: &Request,
        network: &str,
        client: &ClientIdentity,
        notes: &mut Vec<Note>,
    ) -> Result<Outcome> {
        let Some(address) = renewed_address(request) else {
            notes.push(Note::AddressRefused {
                network: network.to_owned(),
                address: Ipv4Addr::UNSPECIFIED,
                detail: "the request named neither a server nor an address".to_owned(),
            });

            return self.refuse(arrival, request);
        };

        let view = self.store.read().await?;
        let lease = lease_time(network, view.network(network).await?)?;
        drop(view);

        // No check against the ranges or the bindings here, and none
        // is needed: a renewal requires a live lease in *this* network
        // recorded against an attribute this client still presents,
        // which is a stronger statement than any of them. An address
        // this client holds in a different network is not found here,
        // which is the whole point.
        match self
            .store
            .renew_lease(&ACTOR, network, address, client, i64::from(lease.seconds()))
            .await
        {
            Ok(_) => self.send(
                arrival,
                request,
                &Answer::acknowledge(request, arrival.server, address, lease),
            ),
            // The two refusals an identity can decide: no live lease,
            // because it lapsed or there never was one here, and a
            // lease another client holds.
            Err(refused)
                if matches!(
                    refused,
                    StoreError::NotFound { .. } | StoreError::Conflict { .. }
                ) =>
            {
                // A rebooting client that this network has never
                // heard of is the one case the section makes a MUST,
                // and the record is asked for only here, on the path
                // that was about to refuse.
                if rebooting(request) && !self.has_a_record_of(arrival, network, client).await? {
                    return Ok(Outcome::Silent(Silence::NoRecordOfThisClient));
                }

                notes.push(Note::AddressRefused {
                    network: network.to_owned(),
                    address,
                    detail: refused.to_string(),
                });

                self.refuse(arrival, request)
            }
            Err(other) => Err(other.into()),
        }
    }

    /// Whether this network has anything on record for this client.
    ///
    /// A lease it holds, live *or* lapsed, or a binding configured for
    /// it. Both are read from the store rather than decided here: the
    /// lease question is the store's own matching of a client against
    /// every attribute in its identity, and the binding question is
    /// the lookup that refuses rather than answers when the interface
    /// and the network disagree.
    ///
    /// A lapsed lease counts, and that is the whole point of asking
    /// this way. A client whose lease here ran out is a client this
    /// network knows, and what it believes about its address is wrong
    /// -- so it is told, where a client this network has never seen is
    /// not.
    ///
    /// Asked only on the path that is about to refuse, so an ordinary
    /// renewal costs nothing for it.
    async fn has_a_record_of(
        &self,
        arrival: &Arrival<'_>,
        network: &str,
        client: &ClientIdentity,
    ) -> Result<bool> {
        let view = self.store.read().await?;
        if !view.leases_held_by(network, client).await?.is_empty() {
            return Ok(true);
        }
        let reserved = view
            .lookup_binding_on_interface(network, arrival.namespace, arrival.ifname, client)
            .await?;

        Ok(reserved.is_some())
    }

    /// A RELEASE or a DECLINE: the client is giving an address up.
    ///
    /// One method for the two messages because the write is the same
    /// one, and the difference is what it means. A release is a client
    /// finishing with an address. A decline is a client saying the
    /// address this server just handed it is already in use by
    /// something the store knows nothing about, which is the clearest
    /// evidence a server ever gets that its belief about a network is
    /// wrong -- so it is always logged, including when there is
    /// nothing to revoke.
    ///
    /// Neither is answered. The protocol specifies no reply to either.
    async fn giving_up(
        &self,
        request: &Request,
        network: &str,
        client: &ClientIdentity,
        giving: GivingUp,
        notes: &mut Vec<Note>,
    ) -> Result<Outcome> {
        let address = given_up_address(request, giving);

        if giving == GivingUp::Declined {
            notes.push(Note::AddressDeclined {
                network: network.to_owned(),
                address,
            });
        }

        // Which leases this client holds *here*, asked of the store
        // rather than decided from the lease's recorded holder: the
        // store matches a client against every attribute in its
        // identity, ordered by the precedence each lease was recorded
        // against, and a second rule beside it would be a second
        // answer to the same question.
        let view = self.store.read().await?;
        let held = view.leases_held_by(network, client).await?;
        drop(view);

        let Some(lease) = held.into_iter().find(|lease| lease.address == address) else {
            notes.push(Note::AddressNotHeldHere {
                network: network.to_owned(),
                address,
            });

            return Ok(Outcome::Silent(Silence::NotThisClientsAddress { address }));
        };

        self.store
            .revoke_lease(&ACTOR, network, address, &lease.cursor)
            .await?;

        Ok(Outcome::Revoked { address })
    }

    /// A NAK: whatever the client asked for, it may not have it here.
    fn refuse(&self, arrival: &Arrival<'_>, request: &Request) -> Result<Outcome> {
        self.send(arrival, request, &Answer::refuse(request, arrival.server))
    }

    /// Encode an answer and hand it to the egress.
    ///
    /// The request goes with it because how a reply is addressed
    /// depends on something only the request says: whether the client
    /// already holds an address, and so whether ordinary routing can
    /// reach it at all. That decision belongs below this layer, and
    /// this is the layer that has the fact it turns on.
    fn send(&self, arrival: &Arrival<'_>, request: &Request, answer: &Answer) -> Result<Outcome> {
        let datagram = answer.encode();
        let reply = Reply::new(arrival, request, answer, &datagram);

        self.egress.send(&reply)?;

        Ok(Outcome::Sent(answer.kind()))
    }

    /// Whether this relay on this interface has not been reported yet.
    ///
    /// The lock is taken and released inside this call and never held
    /// across an await. A poisoned lock is read through rather than
    /// panicked on: the only thing it guards is which lines have been
    /// logged, and a panic here would take down a process serving
    /// every network in order to protect a log.
    fn first_sighting_of(&self, arrival: &Arrival<'_>, relay: Ipv4Addr) -> bool {
        let mut reported = self.relays.lock().unwrap_or_else(PoisonError::into_inner);

        reported.first_sighting(arrival.namespace, arrival.ifname, relay)
    }
}

/// Which of the two messages that give an address up this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GivingUp {
    /// A RELEASE.
    Released,
    /// A DECLINE.
    Declined,
}

/// The relays already named in the log.
///
/// A record of what has been said, not of what is true, which is why
/// it is allowed to forget. See [`RELAYS_REMEMBERED`] for the bound
/// and why there is one.
#[derive(Debug, Default)]
struct Reported {
    /// Oldest first, so that the one to forget is at the front.
    seen: VecDeque<(String, String, Ipv4Addr)>,
}

impl Reported {
    /// Whether this is the first sighting, recording it if it is.
    fn first_sighting(&mut self, namespace: &str, ifname: &str, relay: Ipv4Addr) -> bool {
        let already = self.seen.iter().any(|(seen_namespace, seen_ifname, seen)| {
            seen_namespace == namespace && seen_ifname == ifname && *seen == relay
        });
        if already {
            return false;
        }

        if self.seen.len() >= RELAYS_REMEMBERED {
            self.seen.pop_front();
        }
        self.seen
            .push_back((namespace.to_owned(), ifname.to_owned(), relay));

        true
    }
}

/// How long this network holds the addresses it gives out.
///
/// The schema's default stands in for a network nobody has configured,
/// so there is no unset state here to handle. A network the interface
/// resolved to and the view cannot find is a different thing: it was
/// deleted between the two reads, and that is reported rather than
/// defaulted, because a default for a network that is not there would
/// be an answer invented about a network nobody has.
fn lease_time(network: &str, configured: Option<Network>) -> Result<LeaseSeconds> {
    let found = configured.ok_or_else(|| Error::NetworkGone {
        network: network.to_owned(),
    })?;

    Ok(LeaseSeconds::new(found.lease_seconds)?)
}

/// Why `wanted` is not an address this network may hand a client that
/// has no reservation, or `None` if it may.
///
/// Both facts come out of the store and neither is recomputed here:
/// containment is the range's own, and a reservation is a binding's
/// address. A client asking for an address outside every range is
/// asking for one the pool could never have offered it, and one a
/// binding reserves is being held for a machine that may simply be
/// switched off.
fn why_not_allocatable(
    wanted: Ipv4Addr,
    ranges: &[AddressRange],
    bindings: &[Binding],
) -> Option<String> {
    if !ranges.iter().any(|range| range.contains(wanted)) {
        return Some("it is outside every range this network allocates from".to_owned());
    }

    if bindings.iter().any(|binding| binding.address == wanted) {
        return Some("a binding in this network reserves it for another client".to_owned());
    }

    None
}

/// The address a REQUEST that names no server is about.
///
/// `ciaddr` first, which is where a client renewing or rebinding puts
/// the address it holds, and then the requested address option, which
/// is where a client checking after a reboot puts the address it
/// remembers with `ciaddr` still zero. Reading both is what makes one
/// path serve all three states.
fn renewed_address(request: &Request) -> Option<Ipv4Addr> {
    if !request.client_address.is_unspecified() {
        return Some(request.client_address);
    }

    request.requested_address
}

/// Whether a REQUEST that names no server comes from a client
/// checking after a reboot rather than one renewing or rebinding.
///
/// The fields are the ones RFC 2131 section 4.3.2 uses to tell the
/// states apart, and they are read here rather than inferred from
/// anything else: a rebooting client fills in the requested address
/// option with the address it remembers and leaves `ciaddr` zero,
/// while a client renewing or rebinding has `ciaddr` set. The server
/// identifier is already known to be absent on this path, which is
/// what rules out a client selecting an offer.
///
/// The distinction is worth a predicate of its own because what hangs
/// on it is a MUST: a rebooting client is asking a question, and one
/// renewing is making a statement.
fn rebooting(request: &Request) -> bool {
    request.client_address.is_unspecified() && request.requested_address.is_some()
}

/// The address a RELEASE or a DECLINE is about.
///
/// The two messages carry it in different fields, and the order below
/// is each message's own: a release puts the address in `ciaddr`, and
/// a decline puts the address it is refusing in the requested address
/// option with `ciaddr` left zero. The other field is read as a
/// fallback rather than refused, so a client that filled in the one
/// this message does not use is still understood.
///
/// The unspecified address is what a message naming neither comes to,
/// and nothing holds it, so such a message revokes nothing and says
/// so.
fn given_up_address(request: &Request, giving: GivingUp) -> Ipv4Addr {
    let (first, second) = match giving {
        GivingUp::Released => (
            Some(request.client_address).filter(|address| !address.is_unspecified()),
            request.requested_address,
        ),
        GivingUp::Declined => (
            request.requested_address,
            Some(request.client_address).filter(|address| !address.is_unspecified()),
        ),
    };

    first.or(second).unwrap_or(Ipv4Addr::UNSPECIFIED)
}
