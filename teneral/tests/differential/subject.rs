//! The subjects: the things in this repository whose bytes a case can
//! be compared against.
//!
//! There are none, and that is the honest state of the tree rather
//! than an omission. No DHCP or DNS code exists here yet, so there is
//! nothing of teneral's to put beside a captured reply. The front end
//! that lands first registers itself below and flips its cases'
//! `status` from `pending` to `compared`. The harness fails a case
//! whose status and registration disagree in either direction, so
//! neither half of that can be forgotten.

/// Something that answers a captured request the way teneral does.
///
/// The signature is deliberately bytes in, bytes out: a case holds
/// frames as they were captured, and a comparison that took anything
/// else would be comparing a parse of the wire rather than the wire.
pub struct Subject {
    /// The name a case's `comparison.subject` has to match.
    pub name: &'static str,
    /// Teneral's reply to the captured request, as a frame.
    pub answer: fn(&[u8]) -> Result<Vec<u8>, String>,
}

/// Every subject the rig can compare against.
pub const SUBJECTS: &[Subject] = &[];

/// The subject a case names, if it is registered.
pub fn find(name: &str) -> Option<&'static Subject> {
    SUBJECTS.iter().find(|subject| subject.name == name)
}
