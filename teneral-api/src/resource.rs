//! One converter per resource kind, and the state each is built from.
//!
//! A resource reaches this crate twice over. A read hands back the
//! store's own model; a stream event hands back the same resource as
//! JSON text the database engine built, because the store composes
//! that state without a serialiser of its own. Both have to arrive on
//! the wire as the same message, and the failure when they do not is
//! invisible until a consumer applies a streamed event over a read it
//! already held and the two disagree about a field neither side
//! mentions.
//!
//! So the message is built in exactly one place per kind. The state
//! structs below are what a read and an event have in common -- the
//! resource's own fields, without the position it was written at --
//! and they are what the converters take. A read fills one from the
//! store's model through `of`, and a stream fills one by
//! deserialising the event's state. Neither builds a message itself.
//!
//! Two things are deliberately not in a state struct.
//!
//! The position is a parameter rather than a field, because the two
//! callers learn it differently: a read reads it off the model and an
//! event carries it in the event rather than in its state. A field
//! would be a field one caller had to remember to fill.
//!
//! And whether a lease has run out is a parameter too, for the same
//! reason it is a field on the wire at all: it is the expiry instant
//! evaluated at the moment the answer was produced, so it is the
//! answering layer's to decide and not a property of the state. A
//! default here would be a wrong answer about an address somebody is
//! using.

use serde::Deserialize;
use teneral_store::{AddressRange, Attachment, Binding, EventKind, Lease, Network};

use crate::proto::{self, Cursor, Resource, resource};

/// A network, as a read or an event supplies it.
///
/// Every one of these mirrors the keys the store's event state holds,
/// name for name, and refuses one it does not know: a key the store
/// began writing and this struct has not been told about would
/// otherwise be dropped from a stream's events and kept in a read's,
/// which is the exact divergence this module exists to make
/// impossible.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkState {
    /// The caller's name for it.
    pub name: String,
    /// Who made the write that produced this state.
    pub written_by: String,
}

impl NetworkState {
    /// The state a read found.
    pub fn of(network: &Network) -> Self {
        Self {
            name: network.name.clone(),
            written_by: network.written_by.clone(),
        }
    }
}

/// An interface a network has claimed, as a read or an event supplies
/// it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentState {
    /// The network that claimed it.
    pub network: String,
    /// The namespace the interface name is unique within.
    pub namespace: String,
    /// The interface itself.
    pub ifname: String,
    /// Who made the write that produced this state.
    pub written_by: String,
}

impl AttachmentState {
    /// The state a read found.
    pub fn of(attachment: &Attachment) -> Self {
        Self {
            network: attachment.network.clone(),
            namespace: attachment.namespace.clone(),
            ifname: attachment.ifname.clone(),
            written_by: attachment.written_by.clone(),
        }
    }
}

/// A span of addresses, as a read or an event supplies it.
///
/// The ends are the numbers the row holds rather than dotted quads,
/// which is both what the event's state carries and what the wire
/// carries, so nothing here renders an address to text and parses it
/// back.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddressRangeState {
    /// The network that owns it.
    pub network: String,
    /// Its name within that network.
    pub name: String,
    /// The lowest address in the span.
    pub first: u32,
    /// The highest address in the span, included in it.
    pub last: u32,
    /// Who made the write that produced this state.
    pub written_by: String,
}

impl AddressRangeState {
    /// The state a read found.
    pub fn of(range: &AddressRange) -> Self {
        Self {
            network: range.network.clone(),
            name: range.name.clone(),
            first: proto::address(range.first),
            last: proto::address(range.last),
            written_by: range.written_by.clone(),
        }
    }
}

/// What an operator configured for one client, as a read or an event
/// supplies it.
///
/// The selector arrives as its two stored halves rather than as a
/// parsed selector, because that is the shape the event's state has
/// and a read can render into it without losing anything: the kind's
/// stored spelling is the store's own, from one list.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingState {
    /// The network it was configured in.
    pub network: String,
    /// Which attribute it is configured against.
    pub selector_kind: String,
    /// That attribute's value.
    pub selector_value: String,
    /// The address this client is answered with.
    pub address: u32,
    /// The name it contributes to its network's zones, if it was given
    /// one. Absent and empty are different states and only one of them
    /// is legal, which is why this is an option rather than a string.
    pub hostname: Option<String>,
    /// Who made the write that produced this state.
    pub written_by: String,
}

impl BindingState {
    /// The state a read found.
    pub fn of(binding: &Binding) -> Self {
        Self {
            network: binding.network.clone(),
            selector_kind: binding.selector.kind().as_str().to_owned(),
            selector_value: binding.selector.value().to_owned(),
            address: proto::address(binding.address),
            hostname: binding.hostname.clone(),
            written_by: binding.written_by.clone(),
        }
    }
}

/// An address a client is using, as a read or an event supplies it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseState {
    /// The network the address was leased in.
    pub network: String,
    /// The address, which with the network is the lease's identity.
    pub address: u32,
    /// Which attribute the holder was identified by.
    pub holder_kind: String,
    /// That attribute's value.
    pub holder_value: String,
    /// Where in the store's order the current holder acquired the
    /// address: the sequence alone, the epoch being the one the
    /// position this state is converted at belongs to.
    pub lease_id: u64,
    /// When the current holder acquired it, in seconds since the Unix
    /// epoch.
    pub acquired_at: i64,
    /// When the lease runs out, in the same units.
    pub expires_at: i64,
    /// Who made the write that produced this state.
    pub written_by: String,
}

impl LeaseState {
    /// The state a read found.
    pub fn of(lease: &Lease) -> Self {
        Self {
            network: lease.network.clone(),
            address: proto::address(lease.address),
            holder_kind: lease.holder.kind().as_str().to_owned(),
            holder_value: lease.holder.value().to_owned(),
            lease_id: lease.lease_id.sequence(),
            acquired_at: lease.acquired_at,
            expires_at: lease.expires_at,
            written_by: lease.written_by.clone(),
        }
    }
}

/// A network, as the wire carries it.
pub fn network(state: &NetworkState, written_at: &Cursor) -> proto::Network {
    proto::Network {
        name: state.name.clone(),
        cursor: Some(written_at.clone()),
        written_by: state.written_by.clone(),
    }
}

/// An attachment, as the wire carries it.
pub fn attachment(state: &AttachmentState, written_at: &Cursor) -> proto::Attachment {
    proto::Attachment {
        network: state.network.clone(),
        namespace: state.namespace.clone(),
        ifname: state.ifname.clone(),
        cursor: Some(written_at.clone()),
        written_by: state.written_by.clone(),
    }
}

/// An address range, as the wire carries it.
pub fn address_range(state: &AddressRangeState, written_at: &Cursor) -> proto::AddressRange {
    proto::AddressRange {
        network: state.network.clone(),
        name: state.name.clone(),
        first: state.first,
        last: state.last,
        cursor: Some(written_at.clone()),
        written_by: state.written_by.clone(),
    }
}

/// A binding, as the wire carries it.
pub fn binding(state: &BindingState, written_at: &Cursor) -> proto::Binding {
    proto::Binding {
        network: state.network.clone(),
        selector: Some(selector(&state.selector_kind, &state.selector_value)),
        address: state.address,
        hostname: state.hostname.clone(),
        cursor: Some(written_at.clone()),
        written_by: state.written_by.clone(),
    }
}

/// A lease, as the wire carries it.
///
/// `expired` is the expiry instant evaluated at the moment this answer
/// was produced, which is the caller's to decide: a read has the
/// store's own evaluation at the position it read, and a stream has
/// the state as it was written. Nothing here derives it, because a
/// derivation would be this layer's clock disagreeing with the one the
/// answer was read at.
pub fn lease(state: &LeaseState, written_at: &Cursor, expired: bool) -> proto::Lease {
    proto::Lease {
        network: state.network.clone(),
        address: state.address,
        holder: Some(selector(&state.holder_kind, &state.holder_value)),
        // The lease id is a position in the same order as the one this
        // state was written at, and the store issues every position it
        // reads in its own current epoch -- so the epoch of the one is
        // the epoch of the other, and carrying it from here is what
        // keeps the id a position a consumer can compare rather than a
        // bare number.
        lease_id: Some(Cursor {
            epoch: written_at.epoch.clone(),
            sequence: state.lease_id,
        }),
        acquired_at: state.acquired_at,
        expires_at: state.expires_at,
        expired,
        cursor: Some(written_at.clone()),
        written_by: state.written_by.clone(),
    }
}

/// A network a read found, as a resource.
pub fn of_network(found: &Network) -> Resource {
    wrap(resource::State::Network(network(
        &NetworkState::of(found),
        &proto::cursor(&found.cursor),
    )))
}

/// An attachment a read found, as a resource.
pub fn of_attachment(found: &Attachment) -> Resource {
    wrap(resource::State::Attachment(attachment(
        &AttachmentState::of(found),
        &proto::cursor(&found.cursor),
    )))
}

/// An address range a read found, as a resource.
pub fn of_address_range(found: &AddressRange) -> Resource {
    wrap(resource::State::AddressRange(address_range(
        &AddressRangeState::of(found),
        &proto::cursor(&found.cursor),
    )))
}

/// A binding a read found, as a resource.
pub fn of_binding(found: &Binding) -> Resource {
    wrap(resource::State::Binding(of_binding_message(found)))
}

/// A binding a read found, as the message itself.
///
/// The explain answer carries a binding without wrapping it in a
/// resource, and it is the same binding: a diagnostic that spelled a
/// binding differently from a read of one would be a diagnostic that
/// could agree with itself and disagree with the daemon.
pub fn of_binding_message(found: &Binding) -> proto::Binding {
    binding(&BindingState::of(found), &proto::cursor(&found.cursor))
}

/// A lease a read found, as a resource.
pub fn of_lease(found: &Lease) -> Resource {
    wrap(resource::State::Lease(lease(
        &LeaseState::of(found),
        &proto::cursor(&found.cursor),
        found.expired,
    )))
}

/// The resource an event's state describes.
///
/// The other caller of the converters above, and the reason they are
/// one set rather than two: the store hands an event's state over as
/// the JSON text the database engine built, so a message assembled
/// from it by any other route would be a second spelling of a
/// resource, and the first time anybody noticed would be a consumer
/// applying a streamed event over a read it was already holding.
///
/// `written_at` is the event's own position, which is where the write
/// that produced this state sits. `now` is the instant the answer is
/// being produced, and it is used for exactly one thing: whether a
/// lease's expiry has passed. The store evaluates that for a read and
/// cannot for an event, an event's state being text it wrote when
/// nothing had expired.
///
/// A state that does not parse is an error rather than a resource with
/// fields missing. It means this build and the store disagree about
/// what an event carries, which is a thing to report and not to paper
/// over.
pub fn of_event(
    kind: EventKind,
    state: &str,
    written_at: &Cursor,
    now: i64,
) -> Result<Resource, serde_json::Error> {
    let resource = match kind {
        EventKind::Network => wrap(resource::State::Network(network(
            &serde_json::from_str(state)?,
            written_at,
        ))),
        EventKind::Attachment => wrap(resource::State::Attachment(attachment(
            &serde_json::from_str(state)?,
            written_at,
        ))),
        EventKind::AddressRange => wrap(resource::State::AddressRange(address_range(
            &serde_json::from_str(state)?,
            written_at,
        ))),
        EventKind::Binding => wrap(resource::State::Binding(binding(
            &serde_json::from_str(state)?,
            written_at,
        ))),
        EventKind::Lease => {
            let state: LeaseState = serde_json::from_str(state)?;
            let expired = !held(state.expires_at, now);

            wrap(resource::State::Lease(lease(&state, written_at, expired)))
        }
    };

    Ok(resource)
}

/// Whether a lease expiring at `expires_at` is still held at `now`.
///
/// The store's own boundary -- held while the clock is before the
/// instant, expired from the instant itself -- restated here for the
/// one answer the store cannot evaluate, because an event's state is
/// text and nothing had expired when it was written. The two
/// disagreeing by one would report an address as free in a stream and
/// held in a read of the same position.
fn held(expires_at: i64, now: i64) -> bool {
    now < expires_at
}

/// A selector from the two halves the store stores it as.
///
/// A kind this build cannot name becomes the unspecified value with
/// its value kept, which is the wire's own way of saying so. It cannot
/// arise from a read -- the string came from the store's own list --
/// and from an event it means the store has begun writing a kind this
/// build does not serve, which a consumer should refuse rather than
/// read as some other kind.
fn selector(kind: &str, value: &str) -> proto::Selector {
    let kind = match proto::selector_kind_named(kind) {
        Some(kind) => proto::selector_kind(kind),
        None => proto::SelectorKind::Unspecified,
    };

    proto::Selector {
        kind: kind as i32,
        value: value.to_owned(),
    }
}

/// A state in the one message that says what a resource can be.
fn wrap(state: resource::State) -> Resource {
    Resource { state: Some(state) }
}

#[cfg(test)]
mod tests {
    use super::{BindingState, LeaseState, binding, lease};
    use crate::proto::{self, SelectorKind};
    use teneral_store::SelectorKind as StoreSelectorKind;

    /// A position, as the wire carries one.
    fn at(sequence: u64) -> proto::Cursor {
        proto::Cursor {
            epoch: "abc".to_owned(),
            sequence,
        }
    }

    #[test]
    fn every_selector_kind_the_store_stores_is_named_on_the_wire() {
        // The converters take the kind as the string the store stores,
        // so the mapping back is by name and not by a number the
        // compiler checks. A kind whose spelling did not match would
        // reach a consumer as the unspecified value, which is a
        // binding that matches nothing rather than an error -- so the
        // whole list is walked here.
        for kind in StoreSelectorKind::ALL {
            let state = BindingState {
                network: "alpha".to_owned(),
                selector_kind: kind.as_str().to_owned(),
                selector_value: "value".to_owned(),
                address: proto::address("10.0.0.5".parse().expect("a literal address")),
                hostname: None,
                written_by: "peer:0".to_owned(),
            };

            let message = binding(&state, &at(7));
            let selector = message.selector.expect("a binding names its selector");

            assert_eq!(selector.kind, proto::selector_kind(kind) as i32);
            assert_ne!(selector.kind, SelectorKind::Unspecified as i32);
            assert_eq!(selector.value, "value");
        }
    }

    #[test]
    fn a_lease_id_carries_the_epoch_of_the_position_it_was_written_at() {
        // The state the store builds for an event holds the lease id's
        // sequence and no epoch, so a converter that left the epoch
        // empty would hand a consumer a sequence number it could
        // compare against another store's.
        let state = LeaseState {
            network: "alpha".to_owned(),
            address: proto::address("10.0.0.5".parse().expect("a literal address")),
            holder_kind: "mac".to_owned(),
            holder_value: "00:11:22:33:44:55".to_owned(),
            lease_id: 3,
            acquired_at: 100,
            expires_at: 200,
            written_by: "peer:0".to_owned(),
        };

        let message = lease(&state, &at(9), false);
        let id = message.lease_id.expect("a lease carries its id");

        assert_eq!(id.epoch, "abc");
        assert_eq!(id.sequence, 3);
        assert_eq!(
            message
                .cursor
                .expect("a lease carries its position")
                .sequence,
            9,
            "the id and the position are different numbers and must not be confused"
        );
    }
}
