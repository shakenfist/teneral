//! The generated types, and the conversions between them and the
//! store's.
//!
//! Everything in [`self`] below the conversions is generated from
//! `proto/teneral.v1.proto` into `OUT_DIR` and included here. Nothing
//! generated is committed, so this module is the only place in the
//! crate that knows where the generated code came from.

// The generated code. `include!` rather than a path attribute because
// `OUT_DIR` is only knowable at compile time, and a module of its own
// so that an allow this generated code needs is scoped to it and
// reaches nothing hand-written.
mod generated {
    include!(concat!(env!("OUT_DIR"), "/teneral.v1.rs"));
}

pub use generated::*;

use std::net::Ipv4Addr;

use teneral_store::{Capability as StoreCapability, Cursor as StoreCursor, TokenScope};

/// The store's position, as the wire carries it.
///
/// A function in one place rather than a `From` written at each call
/// site, so that a position cannot reach the wire with its epoch left
/// off -- which would be a sequence number a consumer could compare
/// against another store's.
pub fn cursor(position: &StoreCursor) -> Cursor {
    Cursor {
        epoch: position.epoch().as_str().to_owned(),
        sequence: position.sequence(),
    }
}

/// A capability, as the wire carries it.
///
/// An exhaustive match and not a cast: the two enumerations are
/// independent by design -- one is stored in a database, the other is
/// a field number on a wire -- and a cast between them would make
/// their numbering a thing nobody may change.
pub fn capability(capability: StoreCapability) -> Capability {
    match capability {
        StoreCapability::Read => Capability::Read,
        StoreCapability::Write => Capability::Write,
        StoreCapability::Watch => Capability::Watch,
        StoreCapability::Admin => Capability::Admin,
    }
}

/// The capability a wire value names, or `None` for one this build
/// does not know.
///
/// `None` rather than a default, because a capability that arrived as
/// a number this build cannot name is a request to be refused and not
/// a permission to be guessed at. The unspecified value lands there
/// too: a field nobody set is not a capability.
pub fn capability_from_wire(value: Capability) -> Option<StoreCapability> {
    match value {
        Capability::Read => Some(StoreCapability::Read),
        Capability::Write => Some(StoreCapability::Write),
        Capability::Watch => Some(StoreCapability::Watch),
        Capability::Admin => Some(StoreCapability::Admin),
        Capability::Unspecified => None,
    }
}

/// A token's scope, as the wire carries it.
pub fn scope(scope: &TokenScope) -> Scope {
    let reach = match scope {
        TokenScope::EveryNetwork => scope::Reach::EveryNetwork(EveryNetwork {}),
        TokenScope::Networks(names) => scope::Reach::Networks(NetworkNames {
            names: names.clone(),
        }),
    };

    Scope { reach: Some(reach) }
}

/// The scope a wire value names, or `None` if it named nothing.
///
/// A scope that said nothing is refused rather than read as every
/// network. The commonest way to widen a scope by accident is to leave
/// the field empty, which is why the every-network form is a value a
/// caller had to write.
pub fn scope_from_wire(scope: &Scope) -> Option<TokenScope> {
    match scope.reach.as_ref()? {
        scope::Reach::EveryNetwork(_) => Some(TokenScope::EveryNetwork),
        scope::Reach::Networks(names) => Some(TokenScope::Networks(names.names.clone())),
    }
}

/// An address, as the wire carries it: a 32-bit number, most
/// significant octet first.
///
/// Total in both directions, which is the whole reason the wire
/// carries a number rather than dotted-quad text. No address on this
/// wire can fail to parse, so no operation needs a refusal for one.
pub fn address(address: Ipv4Addr) -> u32 {
    u32::from(address)
}

/// The address a wire value names.
pub fn address_from_wire(value: u32) -> Ipv4Addr {
    Ipv4Addr::from(value)
}

#[cfg(test)]
mod tests {
    use super::{Capability, address, address_from_wire, capability, capability_from_wire};
    use std::net::Ipv4Addr;
    use teneral_store::Capability as StoreCapability;

    #[test]
    fn every_capability_survives_the_round_trip_to_the_wire_and_back() {
        // The two enumerations are matched by hand, so this is the
        // check that the hand did not slip: a capability that mapped to
        // the wrong wire value would hand a caller a permission it was
        // never granted, and nothing else in the crate compares the
        // two lists.
        for held in StoreCapability::ALL {
            assert_eq!(capability_from_wire(capability(held)), Some(held));
        }
    }

    #[test]
    fn a_capability_this_build_cannot_name_is_not_read_as_a_permission() {
        assert_eq!(capability_from_wire(Capability::Unspecified), None);
    }

    #[test]
    fn an_address_survives_the_round_trip_with_its_octets_in_order() {
        // The case a byte-swapped conversion gets wrong: these two
        // differ only in the order of their octets, so a conversion
        // that reversed them would still round-trip and would still be
        // answering about the wrong address.
        let low = Ipv4Addr::new(10, 0, 0, 1);
        let reversed = Ipv4Addr::new(1, 0, 0, 10);

        assert_eq!(address_from_wire(address(low)), low);
        assert_eq!(address_from_wire(address(reversed)), reversed);
        assert_ne!(address(low), address(reversed));
        assert_eq!(address(Ipv4Addr::new(10, 0, 0, 1)), 0x0a00_0001);
    }
}
