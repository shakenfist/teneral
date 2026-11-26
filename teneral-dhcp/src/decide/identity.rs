//! What a packet presents, in the form an operator configures
//! against.
//!
//! The store normalises a hardware address and nothing else, and says
//! why: the other four selector values are byte strings on the wire
//! with no canonical written form, so inventing a normalisation for
//! them would silently merge two values an operator meant to keep
//! apart. The form they take is therefore this front-end's to choose,
//! and the store is explicit that it is not in a position to check the
//! choice.
//!
//! That makes this file the quietest place this project could get
//! something wrong. A form chosen differently here from the form an
//! operator types produces a reservation that is configured, visible
//! in every listing, and never matched -- with every test passing,
//! because a test of a renderer against itself cannot see it. The test
//! that can see it goes the whole way round: it renders from a
//! packet's bytes, configures a binding with the value an operator
//! would type, and asserts that the packet matches the binding.
//!
//! # The forms
//!
//! - **The hardware address** is `chaddr`, truncated to the `hlen`
//!   octets the client declared, as lower-case colon-separated hex.
//!   The store normalises this one and accepts three written forms, so
//!   an operator may type any of them and still match.
//! - **The client identifier**, option 61, is *every* byte of the
//!   option including the leading type byte, as lower-case
//!   colon-separated hex. The precedent is measured rather than
//!   invented: the reference implementation's lease file renders a
//!   client identifier in exactly that form, so an operator moving
//!   from it can paste what they already have.
//! - **The user class and the vendor class**, options 77 and 60, are
//!   the option's bytes as text, when they are valid UTF-8 and the
//!   store will take them.
//! - **DUID and IAID** are never presented. A v4 client has none, and
//!   filling the kind with something that merely looks like one would
//!   be worse than leaving it empty.
//!
//! # An attribute the store will not take is not presented
//!
//! The store refuses a selector value that is empty, longer than its
//! limit, or carries a control character, and it refuses a hardware
//! address that is not six octets. Each of those is possible from a
//! real client: a client identifier may be hundreds of bytes, a vendor
//! class may be any bytes at all, and `hlen` may lawfully be anything
//! from one to sixteen.
//!
//! When the store will not take a value, the attribute is simply not
//! presented and the fact is recorded. That is the same ruling the
//! parser makes about an option whose value will not decode: a client
//! whose vendor class is unprintable loses that attribute and keeps
//! its lease. What it costs is a binding configured against that
//! attribute, which is why it is a note and not a silence -- and if
//! *nothing* survives, the layer above does fall silent, because there
//! is no client for the store to be asked about.

use teneral_store::{ClientIdentity, Selector, SelectorKind};

use crate::decide::outcome::Note;
use crate::wire::Request;

/// What a packet presents, and what was lost on the way.
pub(crate) struct Presented {
    /// The identity to ask the store about. May present nothing.
    pub(crate) identity: ClientIdentity,
    /// What the daemon should say about the attributes that did not
    /// survive.
    pub(crate) notes: Vec<Note>,
}

impl Presented {
    /// Whether there is anything here to ask the store about.
    ///
    /// A lease is recorded against the highest-precedence attribute a
    /// client presented, and a lookup matches against every attribute
    /// in an identity, so an identity presenting nothing is a question
    /// with no subject.
    pub(crate) fn is_empty(&self) -> bool {
        self.identity.selectors().is_empty()
    }
}

/// Read what the client presented into an identity the store will
/// match.
pub(crate) fn presented(request: &Request) -> Presented {
    let mut notes = Vec::new();
    let mut identity = ClientIdentity::new();

    // Highest precedence first, so that the notes read in the order
    // the lookup would have consulted them. The order has no effect on
    // the identity itself -- each kind has its own slot.
    if let Some(identifier) = &request.client_identifier {
        identity = presenting(
            identity,
            SelectorKind::ClientId,
            &colon_hex(identifier),
            &mut notes,
        );
    }

    identity = presenting(
        identity,
        SelectorKind::Mac,
        &colon_hex(&request.hardware_address),
        &mut notes,
    );

    if let Some(class) = &request.user_class {
        identity = presenting_text(identity, SelectorKind::UserClass, class, &mut notes);
    }

    if let Some(class) = &request.vendor_class {
        identity = presenting_text(identity, SelectorKind::VendorClass, class, &mut notes);
    }

    Presented { identity, notes }
}

/// `identity`, also presenting `value` as its `kind`, or `identity`
/// unchanged with a note saying why not.
///
/// `ClientIdentity::with` consumes the identity and hands it back only
/// on success, so the value is checked by itself first and the
/// identity is given up only once that check has passed.
/// [`Selector::new`] is the same check `with` makes, so a value that
/// passed here cannot be refused there -- and the fallback below keeps
/// the identity as it was rather than panicking to prove that.
fn presenting(
    identity: ClientIdentity,
    kind: SelectorKind,
    value: &str,
    notes: &mut Vec<Note>,
) -> ClientIdentity {
    if let Err(refused) = Selector::new(kind, value) {
        notes.push(Note::AttributeNotPresented {
            kind,
            detail: refused.to_string(),
        });

        return identity;
    }

    let unchanged = identity.clone();

    identity.with(kind, value).unwrap_or(unchanged)
}

/// The same, for an option whose value is text when it is text at all.
///
/// Bytes that are not UTF-8 are not presented, and the note says so
/// rather than naming a store refusal, because the store never saw
/// them: the value could not be made into a string to offer it.
fn presenting_text(
    identity: ClientIdentity,
    kind: SelectorKind,
    value: &[u8],
    notes: &mut Vec<Note>,
) -> ClientIdentity {
    match std::str::from_utf8(value) {
        Ok(text) => presenting(identity, kind, text, notes),
        Err(_) => {
            notes.push(Note::AttributeNotPresented {
                kind,
                detail: "its bytes are not valid UTF-8".to_owned(),
            });

            identity
        }
    }
}

/// Bytes as lower-case hex, one octet per pair, colons between.
///
/// The form the reference implementation's lease file renders a client
/// identifier in, and the form the store normalises a hardware address
/// to -- so one function serves both, and the two cannot drift apart.
fn colon_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|octet| format!("{octet:02x}"))
        .collect::<Vec<String>>()
        .join(":")
}
