//! A position in the store's one order of mutations.

use std::fmt;

use crate::error::{Error, Result};
use crate::identity::Epoch;

/// Where a consumer has reached in the store's history, and where a
/// row was last written.
///
/// Two halves, and both are load-bearing. The sequence is the
/// position: it is assigned by the store when a mutation commits, it
/// is never issued twice, and it increases over every network's
/// mutations together. The epoch says whose sequence numbers these
/// are, so that a consumer presenting a remembered position is told
/// "carry on from here" rather than being handed a different
/// history's events under numbers it recognises.
///
/// A cursor is not a timestamp and the order it describes is not
/// causality between networks. Two networks' mutations are interleaved
/// by nothing more than when they arrived, so a consumer that reads
/// "this happened because of that" out of two adjacent sequence
/// numbers in different networks has invented a relationship the store
/// never claimed.
///
/// There is deliberately no ordering implementation. Comparing two
/// cursors is meaningful only once their epochs are known to match,
/// and an `Ord` that quietly ignored the epoch would make the
/// meaningless comparison the easy one to write.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Cursor {
    /// Whose sequence numbers these are.
    epoch: Epoch,
    /// The position in that store's order.
    sequence: u64,
}

impl Cursor {
    /// A cursor at `sequence` in `epoch`'s history.
    pub(crate) fn new(epoch: Epoch, sequence: u64) -> Self {
        Self { epoch, sequence }
    }

    /// A cursor from a sequence number the engine handed back.
    ///
    /// The engine's integers are signed, so the type system allows a
    /// value no sequence this store issues could be. A negative one
    /// means the column has been written by something other than this
    /// code, which is worth a complaint rather than a silent
    /// reinterpretation.
    pub(crate) fn from_row(epoch: &Epoch, sequence: i64) -> Result<Self> {
        let sequence = u64::try_from(sequence).map_err(|_| Error::Malformed {
            detail: format!("it records a position of {sequence}, which no teneral ever issued"),
        })?;

        Ok(Self::new(epoch.clone(), sequence))
    }

    /// A position a caller presented, from the two values it arrived
    /// as.
    ///
    /// The one constructor outside this crate, and it exists because
    /// every compare-and-set the store offers takes a position the
    /// caller read earlier -- which, for a caller on the far side of a
    /// socket, means a position reassembled from an epoch and a
    /// sequence it was handed. Without this, a remembered position
    /// could be read out of the store and never put back into one.
    ///
    /// It makes no claim about the store. An epoch that is not this
    /// store's fails every comparison it is used in, which is exactly
    /// the answer wanted: a caller presenting a position from another
    /// store's order is told the epoch changed rather than quietly
    /// matched against a sequence number that means something else
    /// here.
    pub fn presented(epoch: &str, sequence: u64) -> Self {
        Self::new(Epoch::presented(epoch), sequence)
    }

    /// Whose history this position belongs to.
    pub fn epoch(&self) -> &Epoch {
        &self.epoch
    }

    /// The position itself.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}

impl fmt::Display for Cursor {
    /// The epoch and the sequence together, because either alone is
    /// an incomplete answer and a log that recorded only the sequence
    /// would be unreadable after a restore from a backup.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.epoch, self.sequence)
    }
}
