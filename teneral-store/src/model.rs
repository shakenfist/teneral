//! The resources the store holds, as a caller sees them.
//!
//! Each of these carries the network it belongs to, including when it
//! was fetched through a method that already named one. Repeating it
//! costs a string and removes a class of bug: a caller that learned
//! to infer the network from the question it asked is wrong the day
//! somebody widens the question, and a layer that wants to check a
//! returned row against the network it asked for cannot do so if the
//! row does not say.
//!
//! Each also carries the cursor at which it was last written, which
//! is what makes "change this only if it is still what I read" a
//! single operation available to any caller that has read the row.

use std::net::Ipv4Addr;

use crate::cursor::Cursor;

/// A network: the scope everything else in the store lives inside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    /// The caller's name for it, unique in the store and opaque here.
    pub name: String,
    /// Where this network's creation sits in the store's order.
    pub cursor: Cursor,
}

/// An interface, in a namespace, belonging to exactly one network.
///
/// The namespace and the interface name together are this resource's
/// identity across the whole store rather than within its network,
/// which is the structural half of the rule that an interface belongs
/// to one network only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    /// The network that claimed the interface.
    pub network: String,
    /// The network namespace the interface name is unique within.
    pub namespace: String,
    /// The interface's name inside that namespace.
    pub ifname: String,
    /// Where the claim sits in the store's order.
    pub cursor: Cursor,
}

/// A contiguous span of addresses a network may allocate from.
///
/// The ends are inclusive, which is how an operator writes a pool and
/// how the protocol's own documentation of one reads. Nothing in the
/// store assumes a range is a subnet: a network may own several, or
/// none at all, and a network with none allocates nothing and only
/// ever answers from what was configured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressRange {
    /// The network that owns the range.
    pub network: String,
    /// The range's name, unique within that network.
    pub name: String,
    /// The lowest address in the span.
    pub first: Ipv4Addr,
    /// The highest address in the span, included in it.
    pub last: Ipv4Addr,
    /// Where the range's last write sits in the store's order.
    pub cursor: Cursor,
}

impl AddressRange {
    /// Whether `address` falls inside this range.
    ///
    /// On the type rather than at each call site because the ends are
    /// inclusive and an off-by-one here is an address handed out that
    /// no range contains, or one inside a pool that can never be
    /// allocated.
    pub fn contains(&self, address: Ipv4Addr) -> bool {
        let address = u32::from(address);

        u32::from(self.first) <= address && address <= u32::from(self.last)
    }
}

#[cfg(test)]
mod tests {
    use super::AddressRange;
    use crate::cursor::Cursor;
    use crate::identity::Epoch;
    use std::net::Ipv4Addr;

    fn range(first: &str, last: &str) -> AddressRange {
        AddressRange {
            network: "alpha".to_owned(),
            name: "pool".to_owned(),
            first: first.parse().unwrap(),
            last: last.parse().unwrap(),
            cursor: Cursor::new(Epoch::for_test("00"), 1),
        }
    }

    #[test]
    fn a_range_includes_both_of_its_ends() {
        let pool = range("10.0.0.10", "10.0.0.20");

        assert!(pool.contains(Ipv4Addr::new(10, 0, 0, 10)));
        assert!(pool.contains(Ipv4Addr::new(10, 0, 0, 20)));
        assert!(!pool.contains(Ipv4Addr::new(10, 0, 0, 9)));
        assert!(!pool.contains(Ipv4Addr::new(10, 0, 0, 21)));
    }

    #[test]
    fn containment_compares_addresses_and_not_their_text() {
        // The case that a dotted-quad comparison gets wrong: "10.0.0.9"
        // sorts after "10.0.0.100" as text, so a store that compared
        // the written form would place this address outside a range
        // that plainly contains it.
        let pool = range("10.0.0.9", "10.0.0.100");

        assert!(pool.contains(Ipv4Addr::new(10, 0, 0, 100)));
        assert!(pool.contains(Ipv4Addr::new(10, 0, 0, 99)));
    }
}
