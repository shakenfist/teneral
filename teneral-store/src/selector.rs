//! What a client presents, what an operator configures against it,
//! and which of several matches wins.
//!
//! A client arriving on a network presents several attributes at
//! once: a hardware address, perhaps an identifier it was configured
//! with, perhaps a class naming its vendor or its role. A selector is
//! one of those attributes and its value, and an operator configures
//! a binding against a selector. Several of a client's attributes may
//! have bindings configured for them in the same network, which is
//! why an order between them has to exist.
//!
//! # The order, and why it is this one
//!
//! Highest first: the client identifier, then the DUID and IAID, then
//! the hardware address, then the user class, then the vendor class.
//!
//! One principle generates it. An attribute that names a single
//! client outranks one that names a class of clients; among those
//! that name a single client, the identity the client asserts *as a
//! client* outranks the identity of the hardware it happens to be in
//! today; and among those that name a class, the class the operator's
//! own deployment sets outranks the class the vendor shipped.
//!
//! Taking the five in turn:
//!
//! - The **client identifier** exists only because somebody set it,
//!   and the reason the protocol has it at all is that a client's
//!   identity should survive the replacement of its network card. It
//!   is therefore both the most deliberate and the most stable thing
//!   a v4 client can present.
//! - The **DUID and IAID** are the same statement in the v6 protocol:
//!   one client, and one address association on it. It sits
//!   immediately below the client identifier rather than beside it
//!   because the two cannot collide in one request -- a v4 request
//!   carries no DUID and a v6 request no hardware address -- so the
//!   order between them is a formality. It is fixed anyway, because a
//!   published precedence with a gap in it is one every consumer has
//!   to guess at.
//! - The **hardware address** names an interface rather than a
//!   client. It changes when a card is replaced and it is duplicated
//!   the moment a virtual machine template is cloned, which makes it
//!   the weakest of the three individual identities -- and it is
//!   nonetheless the one nearly every reservation is written against,
//!   because it is the one every client presents without being asked.
//! - The **user class** is a label the client was configured with. It
//!   names a role -- the installers, the tills -- and many clients
//!   share one.
//! - The **vendor class** names a product or a firmware family, and
//!   every device of that model presents the same value. It is the
//!   broadest of the five and the one whose match is most often an
//!   accident rather than an intention.
//!
//! The one argument for a different order is worth recording, because
//! it is the one an operator will make: nearly all reservations are
//! written against hardware addresses, so putting the hardware
//! address first would match what people actually configure. It is
//! rejected because it inverts the reason client identifiers exist. A
//! deliberate per-client identity would lose to the card the client
//! happens to have in it this month, and an operator who set an
//! identifier precisely so that a replaced card would keep its
//! address would find that the replacement silently got a different
//! one.
//!
//! What keeps the order safe is how narrow a question it answers.
//! Precedence only ever chooses between bindings an operator
//! configured in one network: nothing a client presents can take it
//! out of a binding, and the most a client can do is be moved from
//! one configured binding to another configured binding in the same
//! network. So the question is only ever "of the bindings written
//! here, which did the operator most likely mean for *this* client",
//! and the answer is the narrowest, because the broad ones are there
//! to catch the clients the narrow ones do not name.
//!
//! No standard fixes this order; it is this project's own choice, and
//! the published documentation is its only source. That is exactly
//! why every lookup reports which selector matched. A consumer cannot
//! check its understanding of an invented precedence against a
//! daemon that will not say which rule it applied.

use std::fmt;

use crate::error::{Error, Result};

/// A kind of thing a client presents, and that a binding can be
/// configured against.
///
/// The variants are declared highest-precedence first, which is also
/// the order [`SelectorKind::ALL`] holds them in and the order
/// [`SelectorKind::rank`] returns. That is one order written once; the
/// reasoning for it is in this module's documentation, and a test
/// asserts the order by name so that a reordering fails something
/// that says what it expected rather than quietly changing which
/// binding a client gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SelectorKind {
    /// The client identifier a v4 client presents: the identity it
    /// asserts as a client, independent of its hardware.
    ClientId,
    /// A v6 client's DUID together with the IAID of the address
    /// association being asked about.
    DuidIaid,
    /// The client's hardware address.
    Mac,
    /// The user class the client was configured with: a role, shared
    /// by every client in that role.
    UserClass,
    /// The vendor class the client's software reports: a product or
    /// firmware family, shared by every device of that model.
    VendorClass,
}

impl SelectorKind {
    /// Every kind, in precedence order, highest first.
    ///
    /// The lookup walks this, so this array *is* the precedence. A
    /// kind missing from it would be a kind an operator could
    /// configure a binding against and that no lookup would ever
    /// consult; the test below is what notices.
    pub const ALL: [SelectorKind; 5] = [
        SelectorKind::ClientId,
        SelectorKind::DuidIaid,
        SelectorKind::Mac,
        SelectorKind::UserClass,
        SelectorKind::VendorClass,
    ];

    /// Where this kind sits in the precedence, lowest number first.
    ///
    /// An exhaustive match rather than a search of [`Self::ALL`], so
    /// that a kind added to the enum and forgotten here does not
    /// compile.
    pub fn rank(self) -> usize {
        match self {
            SelectorKind::ClientId => 0,
            SelectorKind::DuidIaid => 1,
            SelectorKind::Mac => 2,
            SelectorKind::UserClass => 3,
            SelectorKind::VendorClass => 4,
        }
    }

    /// The kind as it is stored and as it appears in an event.
    ///
    /// Hyphenated lower case rather than the Rust spelling, because
    /// this string is in a database column, in an event payload and
    /// in a URL path, and none of those is a place to read a Rust
    /// identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            SelectorKind::ClientId => "client-id",
            SelectorKind::DuidIaid => "duid-iaid",
            SelectorKind::Mac => "mac",
            SelectorKind::UserClass => "user-class",
            SelectorKind::VendorClass => "vendor-class",
        }
    }

    /// The kind a stored string names.
    ///
    /// A value the store wrote always parses, the column being
    /// constrained to exactly these five. One that does not means
    /// something other than this code has written the table, which is
    /// worth saying rather than treating as a binding that matches
    /// nothing.
    pub(crate) fn from_stored(value: &str) -> Result<Self> {
        SelectorKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value)
            .ok_or_else(|| Error::Malformed {
                detail: format!("it holds a binding whose selector kind is {value:?}"),
            })
    }
}

impl fmt::Display for SelectorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One attribute and its value: half of what identifies a client, and
/// the whole of what a binding is configured against.
///
/// A selector is not a resource and has no identity of its own. The
/// same MAC in two networks is two unrelated clients, legitimately,
/// so a selector means nothing until a network is named with it --
/// which is why this type is always a parameter alongside a network
/// and never a thing the store will look up on its own.
///
/// The value is held in the form the store compares and stores, which
/// [`Selector::new`] is the only way to reach. See there for which
/// kinds are normalised and which are taken as given.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Selector {
    /// Which attribute this is.
    kind: SelectorKind,
    /// Its value, normalised for its kind.
    value: String,
}

impl Selector {
    /// A selector of `kind` with `value`, in the form the store
    /// compares.
    ///
    /// Matching is byte-for-byte equality on the value, so the form
    /// the value arrives in is the whole of whether a lookup matches a
    /// binding. Exactly one kind is normalised here and the asymmetry
    /// is deliberate.
    ///
    /// A hardware address is normalised to lower-case colon-separated
    /// hex, and nothing else is accepted. It is the one kind with a
    /// written form everybody agrees on, and the one where the hazard
    /// is routine rather than theoretical: an operator copies
    /// `AA:BB:CC:DD:EE:FF` off a label while the client presents
    /// `aa:bb:cc:dd:ee:ff`, and a store comparing those as text has a
    /// reservation that is configured, visible in every listing, and
    /// never matched.
    ///
    /// The other four values are byte strings on the wire with no
    /// canonical text form at all -- a client identifier may be hex
    /// digits, or a string, or an arbitrary opaque blob, and a user
    /// class is free text whose case is meaningful. Inventing a
    /// normalisation for those would silently merge two values an
    /// operator meant to keep apart, so they are stored as given and
    /// the front-end that renders an option into a selector value owns
    /// the choice of form. That choice has to be the same on the
    /// configuring side and the matching side, and this store is not
    /// in a position to check that it is.
    pub fn new(kind: SelectorKind, value: &str) -> Result<Self> {
        let value = match kind {
            SelectorKind::Mac => normalise_mac(value)?,
            _ => {
                crate::ops::validate_name("selector value", value)?;
                value.to_owned()
            }
        };

        Ok(Self { kind, value })
    }

    /// A selector from the two columns a stored row holds.
    ///
    /// Not normalised again: the value is whatever was written, and a
    /// row rewritten on the way out would hide the fact that it is
    /// not what a lookup would match.
    pub(crate) fn from_row(kind: &str, value: String) -> Result<Self> {
        Ok(Self {
            kind: SelectorKind::from_stored(kind)?,
            value,
        })
    }

    /// Which attribute this is.
    pub fn kind(&self) -> SelectorKind {
        self.kind
    }

    /// Its value, as the store compares and stores it.
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for Selector {
    /// The kind and the value, because the value alone is ambiguous:
    /// the same string can be a client identifier and a user class.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind, self.value)
    }
}

/// Everything a client presents, as the store is asked about it.
///
/// The union of the five attributes, with at most one value for each,
/// because a request carries at most one of each and a store that
/// accepted two hardware addresses for one client would be storing a
/// contradiction. A client that presents nothing at all is allowed
/// and matches nothing, which is a real case: a lookup for an
/// identity with no attributes set is how a caller asks what a
/// completely unknown client would be answered.
///
/// Values go in through [`Selector::new`], so an identity cannot hold
/// a value in a form the store would not match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientIdentity {
    /// One slot per kind, indexed by [`SelectorKind::rank`], so that
    /// the storage order and the precedence order cannot drift apart.
    presented: [Option<String>; SelectorKind::ALL.len()],
}

impl ClientIdentity {
    /// An identity presenting nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// This identity, also presenting `value` as its `kind`.
    ///
    /// Consuming and returning, so that building an identity is one
    /// expression and a caller cannot leave half of one lying about.
    /// A second value for a kind replaces the first rather than
    /// joining it.
    pub fn with(mut self, kind: SelectorKind, value: &str) -> Result<Self> {
        let selector = Selector::new(kind, value)?;
        self.presented[kind.rank()] = Some(selector.value);

        Ok(self)
    }

    /// What this identity presents as `kind`, if anything.
    pub fn presents(&self, kind: SelectorKind) -> Option<&str> {
        self.presented[kind.rank()].as_deref()
    }

    /// The selectors this identity presents, highest precedence
    /// first.
    ///
    /// This is the order a lookup consults them in, and the order the
    /// explain operation reports as having been considered. Built
    /// from [`SelectorKind::ALL`] rather than from the order a caller
    /// set them, which is the point: what a client presents first on
    /// the wire has nothing to do with which binding applies.
    pub fn selectors(&self) -> Vec<Selector> {
        SelectorKind::ALL
            .into_iter()
            .filter_map(|kind| {
                self.presents(kind).map(|value| Selector {
                    kind,
                    value: value.to_owned(),
                })
            })
            .collect()
    }
}

/// A hardware address as the store stores it: lower-case hex, octets
/// separated by colons.
///
/// Three written forms are accepted because all three are what an
/// operator has to hand: colon-separated, hyphen-separated, and
/// twelve bare hex digits. Anything else is refused rather than
/// guessed at -- a value that is nearly a hardware address is a
/// reservation that will never match, and the moment to say so is
/// when it is written rather than when a client fails to get it.
fn normalise_mac(value: &str) -> Result<String> {
    let refuse = |why: &str| Error::Invalid {
        what: "hardware address",
        detail: format!("{value:?} {why}"),
    };

    let digits: String = match value.len() {
        // Twelve bare hex digits.
        12 => value.to_owned(),
        // Six octets with five separators between them.
        17 => {
            let separators: Vec<char> = value
                .chars()
                .skip(2)
                .step_by(3)
                .take_while(|c| *c == ':' || *c == '-')
                .collect();
            if separators.len() != 5 {
                return Err(refuse("is not six octets separated by colons or hyphens"));
            }

            // One separator character throughout, rather than a
            // mixture: a value that mixes them is more likely to be
            // two things concatenated than an address.
            if separators.windows(2).any(|pair| pair[0] != pair[1]) {
                return Err(refuse("mixes colons and hyphens between its octets"));
            }

            value.chars().filter(|c| *c != ':' && *c != '-').collect()
        }
        _ => {
            return Err(refuse(
                "is not twelve hex digits, with or without separators",
            ));
        }
    };

    if digits.len() != 12 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(refuse("contains something that is not a hex digit"));
    }

    // Built a byte at a time rather than through a split and a join,
    // so that nothing here can fail: every byte has just been checked
    // to be an ASCII hex digit, and `char::from` on one of those is
    // total.
    let lower = digits.to_ascii_lowercase();
    let mut rendered = String::with_capacity(17);
    for (index, octet) in lower.as_bytes().chunks(2).enumerate() {
        if index > 0 {
            rendered.push(':');
        }
        for byte in octet {
            rendered.push(char::from(*byte));
        }
    }

    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::{ClientIdentity, Selector, SelectorKind};
    use crate::error::Error;

    #[test]
    fn the_precedence_is_the_published_one() {
        // Named rather than derived, so that reordering the enum
        // fails a test that says what the order was supposed to be.
        // The order is not an implementation detail: it decides which
        // binding a client gets, it is this project's own invention,
        // and it is published.
        assert_eq!(
            SelectorKind::ALL,
            [
                SelectorKind::ClientId,
                SelectorKind::DuidIaid,
                SelectorKind::Mac,
                SelectorKind::UserClass,
                SelectorKind::VendorClass,
            ]
        );

        // The ranks agree with that order, and every kind is in it.
        for (position, kind) in SelectorKind::ALL.into_iter().enumerate() {
            assert_eq!(kind.rank(), position, "{kind} is ranked out of place");
        }
    }

    #[test]
    fn an_identity_presents_its_selectors_in_precedence_order() {
        // Set in the reverse of the precedence, to prove that the
        // order a caller happened to build the identity in -- or a
        // client happened to present the attributes in -- has no
        // bearing on which binding wins.
        let identity = ClientIdentity::new()
            .with(SelectorKind::VendorClass, "udhcp 1.0")
            .unwrap()
            .with(SelectorKind::Mac, "AA:BB:CC:DD:EE:FF")
            .unwrap()
            .with(SelectorKind::ClientId, "01:ff:00")
            .unwrap();

        let kinds: Vec<SelectorKind> = identity.selectors().iter().map(Selector::kind).collect();
        assert_eq!(
            kinds,
            [
                SelectorKind::ClientId,
                SelectorKind::Mac,
                SelectorKind::VendorClass
            ]
        );
    }

    #[test]
    fn a_hardware_address_is_stored_in_one_form_however_it_was_written() {
        // The hazard this closes: an operator copies an address off a
        // label in upper case and the client presents it in lower,
        // and a store comparing the text has a reservation that is
        // configured, listed, and never matched.
        let forms = [
            "AA:BB:CC:DD:EE:FF",
            "aa:bb:cc:dd:ee:ff",
            "AA-BB-CC-DD-EE-FF",
            "aabbccddeeff",
            "AABBCCDDEEFF",
        ];

        for form in forms {
            let selector = Selector::new(SelectorKind::Mac, form).unwrap();
            assert_eq!(selector.value(), "aa:bb:cc:dd:ee:ff", "from {form}");
        }
    }

    #[test]
    fn something_that_is_nearly_a_hardware_address_is_refused() {
        for wrong in [
            "aa:bb:cc:dd:ee",
            "aa:bb:cc:dd:ee:ff:00",
            "aa:bb:cc:dd:ee:gg",
            "aa-bb:cc-dd:ee-ff",
            "aabbccddeef",
            "",
            "the one on the label",
        ] {
            assert!(
                matches!(
                    Selector::new(SelectorKind::Mac, wrong),
                    Err(Error::Invalid { .. })
                ),
                "{wrong:?} was accepted as a hardware address"
            );
        }
    }

    #[test]
    fn the_other_kinds_are_stored_exactly_as_they_were_written() {
        // Case is meaningful in a user class and a client identifier
        // may be an arbitrary byte string, so there is no
        // normalisation to apply that would not merge two values an
        // operator meant to keep apart.
        let selector = Selector::new(SelectorKind::UserClass, "Tills").unwrap();
        assert_eq!(selector.value(), "Tills");

        let other = Selector::new(SelectorKind::UserClass, "tills").unwrap();
        assert_ne!(selector, other);
    }

    #[test]
    fn a_selector_value_the_store_could_not_report_back_is_refused() {
        assert!(matches!(
            Selector::new(SelectorKind::ClientId, ""),
            Err(Error::Invalid { .. })
        ));
        assert!(matches!(
            Selector::new(SelectorKind::VendorClass, "two\nlines"),
            Err(Error::Invalid { .. })
        ));
    }

    #[test]
    fn a_stored_kind_round_trips_and_an_unknown_one_is_a_complaint() {
        for kind in SelectorKind::ALL {
            assert_eq!(SelectorKind::from_stored(kind.as_str()).unwrap(), kind);
        }

        assert!(matches!(
            SelectorKind::from_stored("hostname"),
            Err(Error::Malformed { .. })
        ));
    }
}
