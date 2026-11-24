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
use crate::selector::Selector;

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

/// What an operator configured for one client identity in one
/// network.
///
/// The selector is the binding's identity within its network, and it
/// is therefore also the answer to "which selector matched" on every
/// lookup that returned this binding. That is not a coincidence to be
/// relied on quietly: the precedence between selector kinds is this
/// project's own invention, so a consumer can only check that its
/// understanding of the order matches the daemon's by being told
/// which rule was applied. Because the matched selector *is* the
/// binding's identity, a lookup cannot answer without saying.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// The network the binding was configured in.
    pub network: String,
    /// The selector it is configured against, which is its identity
    /// inside that network and the selector a lookup matched on.
    pub selector: Selector,
    /// The address this client is to be answered with.
    pub address: Ipv4Addr,
    /// The name the binding contributes to its network's zones, if
    /// the operator gave it one.
    pub hostname: Option<String>,
    /// Where the binding's last write sits in the store's order.
    pub cursor: Cursor,
}

/// An address a client is using in a network, as against one an
/// operator configured for it.
///
/// Keyed by `(network, address)` rather than by the client, because
/// within one network an address has at most one holder at a time
/// while a client may hold two addresses at once as it moves between
/// them. So this type names a client in a field rather than in its
/// identity, and "what does this client hold" has a list for an
/// answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    /// The network the address was leased in.
    pub network: String,
    /// The address, which with the network is the lease's identity.
    pub address: Ipv4Addr,
    /// The attribute the holder was identified by when it acquired
    /// the address.
    ///
    /// One selector rather than everything the client presented,
    /// because a holder has to be a single comparable value. It is
    /// the highest-precedence attribute the client presented at the
    /// time, by the same published order a binding lookup walks -- so
    /// a client is identified the same way whether it is being
    /// matched against configuration or given an address.
    pub holder: Selector,
    /// Where in the store's order the current holder acquired this
    /// address.
    ///
    /// This is the opaque lease id the API promises changes whenever
    /// the address changes holder, and it is a position rather than a
    /// random value: derived from the store's own order it is
    /// deterministic in a test, needs no generator, and is already
    /// something a consumer can compare against its stream. A
    /// renewal keeps it, because the holder has not changed; an
    /// acquisition issues a new one, because acquiring is what issues
    /// it.
    ///
    /// It is a cursor rather than a bare number for the reason any
    /// position here is: a sequence number from a restored backup
    /// means nothing without the epoch that says whose it is. It
    /// equals [`Lease::cursor`] for a lease that has not been renewed
    /// since it was acquired, and the two part company at the first
    /// renewal.
    pub lease_id: Cursor,
    /// When the current holder acquired the address, as seconds since
    /// the Unix epoch.
    pub acquired_at: i64,
    /// The instant the lease runs out, as seconds since the Unix
    /// epoch.
    ///
    /// Absolute rather than a duration and a start, so that expiry is
    /// a comparison against the clock rather than an arithmetic every
    /// reader repeats. This is the fact; the field below is the fact
    /// evaluated.
    pub expires_at: i64,
    /// Whether the instant above had passed when this answer was
    /// produced.
    ///
    /// The store evaluates expiry rather than leaving each caller to,
    /// because the rule -- held before the instant, expired from it
    /// -- is one every caller would otherwise write again, and a
    /// caller that wrote the comparison the other way round would
    /// hand out an address somebody is using.
    ///
    /// The instant it was evaluated at is the instant the answer was
    /// produced: for a write, the one the write itself used, and for
    /// a read through a view, the one the view opened at. A view
    /// whose answers moved with the clock would not be a view --
    /// assembling a list of leases from it could report two whose
    /// states were never true together -- and a view is a thing to
    /// open, use and drop rather than to keep by. A caller holding
    /// one for longer has [`Lease::expires_at`], which is the fact
    /// rather than a reading of it.
    pub expired: bool,
    /// Where the lease's last write sits in the store's order.
    pub cursor: Cursor,
}

/// What the store would answer a client, and how it got there, with
/// no packet involved.
///
/// This is the diagnostic the whole project is shaped around. The
/// failure being guarded against is a silently wrong answer, observed
/// hours later inside somebody's guest; an operator or a test that
/// can ask "what *would* you say to this client on this interface"
/// converts that into a question with an immediate answer. It is
/// cheap because it is not a second implementation of anything: the
/// front-ends resolve an interface to a network and match an identity
/// against the bindings in it, and this is that same path with the
/// packet replaced by its parameters.
///
/// Every field is here because its absence is a distinct diagnosis. A
/// network of `None` means no network has claimed the interface at
/// all, which is a different fault from a network that has claimed it
/// and configured nothing. A `matched` of `None` under a network that
/// is present means the client would fall through to whatever the
/// front-end does with an unknown client. And `considered` is what
/// the lookup would have looked for, in the order it would have
/// looked: it turns "this selector matched" into "these lost, in this
/// order", which is the only complete check of an invented
/// precedence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Explanation {
    /// The namespace of the interface the client was presented as
    /// arriving on.
    pub namespace: String,
    /// The interface itself.
    pub ifname: String,
    /// The network that interface resolved to, or `None` if no
    /// network has claimed it.
    pub network: Option<String>,
    /// The binding that would answer, if any. Its selector is the one
    /// that matched.
    pub matched: Option<Binding>,
    /// The selectors the lookup would consult, highest precedence
    /// first, whether or not each had a binding.
    pub considered: Vec<Selector>,
    /// The position in the store's order this explanation was
    /// computed at, so that it can be ordered against a watch stream
    /// like any other answer.
    pub cursor: Cursor,
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
