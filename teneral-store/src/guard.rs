//! The check that no answer leaves the store under the wrong
//! network, and the count of the times it had to fire.
//!
//! Every row the store hands back carries the network it came from,
//! and every read that named a network compares the two before the
//! row becomes an answer. A disagreement is returned as an error and
//! counted; it is never served.
//!
//! The error half is what makes this a guard rather than a dashboard.
//! A mechanism that notices a cross-network leak and then serves the
//! answer anyway has converted a bug into a metric, and the bug is
//! the one this whole project exists to prevent: a client told an
//! address that belongs to another network's client, by a daemon that
//! looked entirely healthy while it happened.
//!
//! The counter half is an alarm with no baseline to interpret. In a
//! correct store it stays at zero for the life of the process, so a
//! single non-zero reading is unambiguous -- which is worth more than
//! any rate or dashboard, and is why the number is exposed rather
//! than only logged.
//!
//! Where it can actually fire is worth being precise about, because
//! most of its call sites cannot. A read that filters on the network
//! it was given can only return rows from that network, so comparing
//! them is a tautology *while the statement is right*. The comparison
//! is there for the statement that stops being right: a `WHERE`
//! clause dropped in an edit, a parameter order rotated by one, an
//! index or a cache keyed on an address or a MAC instead of on a
//! network and one of those. Each of those is a one-character change
//! that every single-network test passes, and each turns into a
//! refused read and a non-zero counter here instead of a wrong
//! answer.
//!
//! One read can fire today from data alone, and it is the one the API
//! actually offers: a lookup that is given both a network and the
//! interface a client arrived on. Those two facts come from different
//! places -- the network from the caller, the interface's network
//! from the store -- so they can disagree, and that disagreement is
//! exactly the quantity the specification asks to be counted: a
//! request whose arrival interface resolved to one network while the
//! answer would have come from another.
//!
//! The count belongs to the store rather than to a network in it. The
//! event *is* a disagreement between two networks, so attributing it
//! to one of them would hide which pair it was; the error names both,
//! which is where that detail belongs.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{Error, Result};

/// The comparison every network-scoped answer passes, and the number
/// of times it has refused one.
#[derive(Debug, Default)]
pub(crate) struct CrossNetworkGuard {
    /// How many answers have been refused for naming the wrong
    /// network. Should be zero for the life of the process.
    refused: AtomicU64,
}

impl CrossNetworkGuard {
    /// Refuse `found` if it is not the network that was asked about.
    ///
    /// `id` is a closure rather than a string because it is only ever
    /// used in the refusal: a store answering correctly would
    /// otherwise pay for a formatted identity on every row it
    /// returned, and this is called once per row.
    pub(crate) fn check<F>(&self, asked: &str, found: &str, kind: &'static str, id: F) -> Result<()>
    where
        F: FnOnce() -> String,
    {
        if asked == found {
            return Ok(());
        }

        // Counted before the error is built, so that a caller which
        // discards the error -- or a future one that logs and carries
        // on -- cannot leave the count behind.
        self.refused.fetch_add(1, Ordering::AcqRel);

        Err(Error::CrossNetwork {
            kind,
            id: id(),
            asked: asked.to_owned(),
            found: found.to_owned(),
        })
    }

    /// How many answers this store has refused for naming the wrong
    /// network.
    pub(crate) fn refused(&self) -> u64 {
        self.refused.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::CrossNetworkGuard;
    use crate::error::Error;

    #[test]
    fn an_answer_from_the_network_that_was_asked_about_is_not_counted() {
        let guard = CrossNetworkGuard::default();

        guard
            .check("alpha", "alpha", "binding", || "mac:aa".to_owned())
            .unwrap();

        assert_eq!(guard.refused(), 0);
    }

    #[test]
    fn an_answer_from_another_network_is_refused_counted_and_names_both() {
        let guard = CrossNetworkGuard::default();

        let refusal = guard
            .check("alpha", "beta", "binding", || "mac:aa".to_owned())
            .unwrap_err();

        match &refusal {
            Error::CrossNetwork {
                kind,
                id,
                asked,
                found,
            } => {
                assert_eq!(*kind, "binding");
                assert_eq!(id, "mac:aa");
                assert_eq!(asked, "alpha");
                assert_eq!(found, "beta");
            }
            other => panic!("expected a refusal naming both networks, got {other:?}"),
        }

        // An operator reads the sentence rather than the variant, and
        // the pair is the whole of what makes the reading actionable.
        let message = refusal.to_string();
        assert!(message.contains("alpha"), "{message}");
        assert!(message.contains("beta"), "{message}");

        assert_eq!(guard.refused(), 1);
    }
}
