//! The vocabulary everything is refused with, and the one place the
//! store's refusals are translated into it.
//!
//! Every refusal this daemon sends is built by a function here. That
//! is not tidiness: a refusal built at the point it is needed is a
//! refusal whose code somebody chose in a hurry, and the two rules
//! below are exactly the kind that get lost that way.
//!
//! **A refusal never names a resource outside the caller's scope.**
//! "That interface belongs to a network you may not see" and "no
//! network has claimed that interface" are the same refusal with the
//! same detail, because the difference between those two answers is
//! itself the thing being kept from the caller. The functions that
//! take a network name are the ones used after a scope check has
//! passed; the ones that take none are for the refusals that happen
//! because it did not.
//!
//! **A failure says what the daemon was doing and not what it found.**
//! The store's own messages name the network they were working in,
//! which is the right thing in a log and the wrong thing in an answer
//! to a caller who may not see it.

use teneral_store::Error as StoreError;

use crate::proto::{Capability, Refusal, RefusalCode, VersionMismatch};

/// A refusal in the error position of a `Result`.
///
/// Boxed, because a refusal is a message with a dozen optional fields
/// and a couple of hundred bytes of them: an unboxed one would make
/// every call that cannot be refused pay for the size of the one that
/// can. It is also how a refusal is carried by
/// [`crate::Error::Refused`], so handing one from a check to a
/// caller's error is a move rather than a copy.
pub type Refused = Box<Refusal>;

/// The caller's credential does not hold what the request needs.
pub fn missing_capability(required: Capability) -> Refusal {
    Refusal {
        code: RefusalCode::MissingCapability as i32,
        detail: format!(
            "that operation needs the {} capability, which this credential does not hold",
            name_of(required)
        ),
        required_capability: Some(required as i32),
        ..Refusal::default()
    }
}

/// The request reached outside the caller's scope.
///
/// Takes no network and names none, deliberately. A caller refused for
/// a network it may not see must not be told that the network exists,
/// and this is the refusal both halves of that get: the one that named
/// a network outside the scope, and the one whose answer turned out to
/// be such a network.
pub fn outside_scope() -> Refusal {
    Refusal {
        code: RefusalCode::OutsideScope as i32,
        detail: "that is outside the networks this credential may reach".to_owned(),
        ..Refusal::default()
    }
}

/// There is no such resource, or no resource matched.
pub fn no_such_resource(kind: &str, id: &str) -> Refusal {
    Refusal {
        code: RefusalCode::NoSuchResource as i32,
        detail: format!("there is no {kind} {id}"),
        resource_kind: Some(kind.to_owned()),
        resource_id: Some(id.to_owned()),
        ..Refusal::default()
    }
}

/// Something is already there, and this says what.
pub fn conflict(kind: &str, id: &str, held_by: &str) -> Refusal {
    Refusal {
        code: RefusalCode::Conflict as i32,
        detail: format!("the {kind} {id} conflicts with {held_by}"),
        resource_kind: Some(kind.to_owned()),
        resource_id: Some(id.to_owned()),
        held_by: Some(held_by.to_owned()),
        ..Refusal::default()
    }
}

/// The resource moved between the caller reading it and asking for it
/// to be changed.
pub fn changed(kind: &str, id: &str, expected: &str, found: &str) -> Refusal {
    Refusal {
        code: RefusalCode::Changed as i32,
        detail: format!(
            "the {kind} {id} is at {found} rather than the {expected} the request expected"
        ),
        resource_kind: Some(kind.to_owned()),
        resource_id: Some(id.to_owned()),
        expected_position: Some(expected.to_owned()),
        found_position: Some(found.to_owned()),
        ..Refusal::default()
    }
}

/// The daemon is holding as many views of the store as it will.
pub fn too_many_views(live: usize, bound: usize, retry_after_seconds: u32) -> Refusal {
    Refusal {
        code: RefusalCode::TooManyViews as i32,
        detail: format!(
            "this daemon is reading the store through {live} views of the {bound} it allows; try \
             again shortly"
        ),
        retry_after_seconds: Some(retry_after_seconds),
        ..Refusal::default()
    }
}

/// The request named a resource kind this daemon does not serve.
///
/// Refused rather than dropped from a subscription: a consumer written
/// against a fuller vocabulary should learn that it is talking to an
/// older daemon, not watch a stream that never carries what it asked
/// for.
pub fn unknown_resource_kind(value: i32) -> Refusal {
    Refusal {
        code: RefusalCode::UnknownResourceKind as i32,
        detail: format!("this daemon does not serve resources of kind {value}"),
        ..Refusal::default()
    }
}

/// The protocol version presented is not one this build speaks.
pub fn version_mismatch(presented: u32) -> Refusal {
    Refusal {
        code: RefusalCode::VersionMismatch as i32,
        detail: format!(
            "this daemon speaks version {supported}, and the connection presented version \
             {presented}",
            supported = rendered_versions(),
        ),
        versions: Some(VersionMismatch {
            presented,
            supported: crate::SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
        }),
        ..Refusal::default()
    }
}

/// The connection presented no credential this daemon will act on.
///
/// The detail says which of the two it was -- nothing presented, or
/// something unrecognised -- because the fixes differ: one is a
/// credential to mint and the other a credential to replace. It does
/// not say what uid the connection came from, which the caller already
/// knows and a log is the place for.
pub fn unauthenticated(detail: &str) -> Refusal {
    Refusal {
        code: RefusalCode::Unauthenticated as i32,
        detail: detail.to_owned(),
        ..Refusal::default()
    }
}

/// The request is not one the daemon can act on at all.
pub fn invalid_request(detail: &str) -> Refusal {
    Refusal {
        code: RefusalCode::InvalidRequest as i32,
        detail: detail.to_owned(),
        ..Refusal::default()
    }
}

/// The network had no address to give.
pub fn no_address(detail: &str) -> Refusal {
    Refusal {
        code: RefusalCode::NoAddress as i32,
        detail: detail.to_owned(),
        ..Refusal::default()
    }
}

/// The request named no operation at all, which is what a client
/// built from a newer schema sends.
pub fn unknown_operation() -> Refusal {
    Refusal {
        code: RefusalCode::UnknownOperation as i32,
        detail: "that request named no operation this daemon serves".to_owned(),
        ..Refusal::default()
    }
}

/// The caller asked about one network for an interface another holds.
///
/// Named in full, which is why this is the one function here that
/// takes two network names: it is built only for a caller whose scope
/// is every network, where there is no network outside the scope for a
/// name to leak and the disagreement itself is the thing somebody has
/// to look at. A caller with a scope is refused through
/// [`no_such_resource`] instead, identically to an interface no
/// network has claimed -- the handler that chooses between the two
/// carries the reasoning.
///
/// The code is the one [`for_store_error`] gives every other
/// cross-network failure, because that is what this is: the daemon
/// cannot serve the answer it was asked for and nobody should go
/// looking for a request to fix.
pub fn cross_network(kind: &str, id: &str, asked: &str, found: &str) -> Refusal {
    Refusal {
        code: RefusalCode::Internal as i32,
        detail: format!(
            "the {kind} {id} was asked about in {asked} and belongs to {found}, so the daemon              refused the answer rather than serving one network's configuration to another"
        ),
        resource_kind: Some(kind.to_owned()),
        resource_id: Some(id.to_owned()),
        held_by: Some(found.to_owned()),
        ..Refusal::default()
    }
}

/// The request named an operation the schema carries and this build
/// does not serve.
///
/// The same code as the refusal above, because from the caller's side
/// the two are one thing -- it asked for something that is not there
/// -- and the detail is what says which. `operation` completes "this
/// daemon does not serve ...".
pub fn not_served(operation: &str) -> Refusal {
    Refusal {
        code: RefusalCode::UnknownOperation as i32,
        detail: format!("this daemon does not serve {operation}"),
        ..Refusal::default()
    }
}

/// A frame's length prefix was over the bound.
pub fn message_too_large(length: u64) -> Refusal {
    Refusal {
        code: RefusalCode::MessageTooLarge as i32,
        detail: format!(
            "a frame claimed {length} bytes, over the {bound} this wire carries; the connection is \
             closing because its body was not read",
            bound = crate::wire::MAX_MESSAGE_BYTES,
        ),
        ..Refusal::default()
    }
}

/// The daemon failed rather than refused.
///
/// `context` completes "the daemon could not ...", and it is the
/// daemon's own description of what it was attempting rather than
/// anything it learned while attempting it. What it learned belongs in
/// a log, where naming a network is safe.
pub fn internal(context: &str) -> Refusal {
    Refusal {
        code: RefusalCode::Internal as i32,
        detail: format!("the daemon could not {context}"),
        ..Refusal::default()
    }
}

/// What the store's refusal is on this wire.
///
/// The one translation, so that two handlers cannot come to disagree
/// about whether a compare-and-set failure is a conflict. The arms
/// name every variant the store has today; the final arm exists
/// because that type is `non_exhaustive`, so a variant added to it
/// arrives here as a failure rather than as a compile error -- which
/// is the right way round for a dependency, and the reason the arms
/// above it are written out rather than collapsed.
pub fn for_store_error(error: &StoreError) -> Refusal {
    match error {
        StoreError::UnknownNetwork { network } => no_such_resource("network", network),

        StoreError::NotFound { kind, id } => no_such_resource(kind, id),

        StoreError::Conflict { kind, id, held_by } => conflict(kind, id, held_by),

        StoreError::Changed {
            kind,
            id,
            expected,
            found,
        } => changed(kind, id, expected, found),

        StoreError::Invalid { what, detail } => invalid_request(&format!(
            "the {what} is not one the store will take: {detail}"
        )),

        StoreError::NoAddress { network, detail } => {
            // The network is named because the caller named it: a
            // request this refuses asked the store to choose inside a
            // scope the caller had already passed a check for.
            no_address(&format!("{network} had no address to give: {detail}"))
        }

        // The store's own authorisation rule, which is that a
        // credential grants no more than it holds. The capability half
        // of that is refused before the store is reached, by the one
        // function that decides which capability a request needs, so
        // what arrives here is the scope half.
        StoreError::Unauthorised { .. } => outside_scope(),

        // By construction unreachable: the store refuses an answer
        // that came from the wrong network and counts it. It names
        // both networks, one of which the caller may have no business
        // knowing exists, so this is the one failure whose detail
        // carries nothing from the error at all.
        StoreError::CrossNetwork { .. } => internal("serve an answer it could not scope"),

        StoreError::Storage { context, .. } => internal(context),
        StoreError::Unavailable { context } => internal(context),
        StoreError::Threads { context, .. } => internal(context),

        // A database a newer build wrote, a pragma that did not take,
        // a file that is not shaped like a store. Each is an
        // operator's problem rather than a caller's, and none of them
        // can be answered around.
        StoreError::SchemaAhead { .. }
        | StoreError::Pragma { .. }
        | StoreError::Malformed { .. } => internal("use the store it was given"),

        _ => internal("complete that operation"),
    }
}

/// What a capability is called in a sentence an operator reads.
///
/// The wire's spelling is the enum's number; this is the word, and it
/// is the store's word rather than a second set.
fn name_of(capability: Capability) -> &'static str {
    match crate::proto::capability_from_wire(capability) {
        Some(held) => held.as_str(),
        None => "unknown",
    }
}

/// The versions this build speaks, for a sentence.
fn rendered_versions() -> String {
    crate::SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::{for_store_error, outside_scope, version_mismatch};
    use crate::proto::RefusalCode;
    use teneral_store::Error as StoreError;

    #[test]
    fn a_version_refusal_names_both_versions() {
        let refusal = version_mismatch(2);

        assert_eq!(refusal.code, RefusalCode::VersionMismatch as i32);

        let versions = refusal.versions.expect("a version refusal names the pair");
        assert_eq!(versions.presented, 2);
        assert_eq!(versions.supported, vec![crate::PROTOCOL_VERSION]);
        assert!(refusal.detail.contains('2'));
        assert!(
            refusal
                .detail
                .contains(&crate::PROTOCOL_VERSION.to_string())
        );
    }

    #[test]
    fn a_scope_refusal_names_no_network() {
        // The point of the refusal: a caller outside its scope learns
        // nothing about what it was outside of, because the name of a
        // network is its identity and "that one exists" is the leak.
        let refusal = outside_scope();

        assert_eq!(refusal.code, RefusalCode::OutsideScope as i32);
        assert_eq!(refusal.resource_id, None);
        assert_eq!(refusal.held_by, None);
    }

    #[test]
    fn a_cross_network_answer_is_a_failure_that_names_neither_network() {
        // The store catches an answer it could not scope and names
        // both networks in its own message, which is right for a log
        // and wrong for a caller: one of the two is a network this
        // caller may have no business knowing exists.
        let refusal = for_store_error(&StoreError::CrossNetwork {
            kind: "lease",
            id: "10.0.0.5".to_owned(),
            asked: "alpha".to_owned(),
            found: "beta".to_owned(),
        });

        assert_eq!(refusal.code, RefusalCode::Internal as i32);
        assert!(!refusal.detail.contains("alpha"));
        assert!(!refusal.detail.contains("beta"));
    }

    #[test]
    fn a_conflict_names_what_is_in_the_way() {
        let refusal = for_store_error(&StoreError::Conflict {
            kind: "attachment",
            id: "ns/eth0".to_owned(),
            held_by: "the network alpha".to_owned(),
        });

        assert_eq!(refusal.code, RefusalCode::Conflict as i32);
        assert_eq!(refusal.held_by.as_deref(), Some("the network alpha"));
    }

    #[test]
    fn a_compare_and_set_failure_is_not_reported_as_a_conflict() {
        // A caller reads these two differently: a conflict is repaired
        // by taking what is in the way or giving up, where this is
        // repaired by reading again and deciding.
        let refusal = for_store_error(&StoreError::Changed {
            kind: "binding",
            id: "mac:00:11:22:33:44:55".to_owned(),
            expected: "abc:7".to_owned(),
            found: "abc:9".to_owned(),
        });

        assert_eq!(refusal.code, RefusalCode::Changed as i32);
        assert_eq!(refusal.expected_position.as_deref(), Some("abc:7"));
        assert_eq!(refusal.found_position.as_deref(), Some("abc:9"));
    }
}
