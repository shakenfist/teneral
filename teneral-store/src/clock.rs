//! Where the store's notion of now comes from.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// The store's clock.
///
/// Injected rather than read from the system wherever the store needs
/// a time, and there is one reason for that which is worth more than
/// tidiness: the things this store times out are leases, a lease's
/// expiry is an absolute instant, and the only honest way to test
/// what happens after one has passed is to move the clock. A test
/// that slept instead would be a test somebody later makes faster,
/// and the thing they would shorten is the margin it was measuring.
///
/// The unit is whole seconds since the Unix epoch, which is what the
/// protocol this store feeds puts on the wire and therefore the only
/// precision any of it can act on.
pub trait Clock: fmt::Debug + Send + Sync {
    /// Now, as seconds since the Unix epoch.
    fn unix_seconds(&self) -> i64;
}

/// The host's clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    /// Seconds since the Unix epoch, or seconds before it if the host
    /// believes the year is earlier than 1970.
    ///
    /// Signed, and saturating rather than panicking, because a host
    /// whose clock is absurd is a host whose leases will be wrong and
    /// not a reason for a daemon serving every network to stop. The
    /// value is only ever compared against another value from the
    /// same clock.
    fn unix_seconds(&self) -> i64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
            Err(before) => i64::try_from(before.duration().as_secs())
                .map(|seconds| -seconds)
                .unwrap_or(i64::MIN),
        }
    }
}
