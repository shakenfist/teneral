//! Why this layer could not reach a decision at all.
//!
//! The line between this and a silence is worth stating, because most
//! of what goes wrong with a DHCP request is not in here. A relayed
//! request, an interface no network has claimed, a client asking for
//! an address another client holds, a network with nothing left to
//! allocate -- each of those is a *decision*, reached successfully,
//! whose outcome is a refusal or a silence with a note on it. They are
//! the ordinary condition of a network and they are reported through
//! [`crate::decide::Decided`].
//!
//! What is in here is the store failing to answer a question, the
//! socket layer failing to send, and a configured value no answer can
//! be built from. None of the three is something a client can provoke
//! and all three are worth a daemon's attention rather than a client's.

use std::fmt;

/// The result of asking this layer to answer a request.
pub type Result<T> = std::result::Result<T, Error>;

/// Why a request could not be decided.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The store would not answer, or would not accept a write.
    ///
    /// The refusals this layer expects and acts on -- an address
    /// another client holds, a lease that has lapsed, a network with
    /// no address left -- never reach here: they are read for what
    /// they say and turned into an answer. So a store error arriving
    /// here is one of the kinds no request should be able to cause,
    /// including the cross-network refusal, which must surface rather
    /// than be absorbed into a silence.
    Store(teneral_store::Error),

    /// An answer could not be built from the parameters it was given.
    ///
    /// Both causes are configuration rather than traffic: a prefix
    /// longer than an address has bits, and a lease time no lease time
    /// option can carry. The store's own constraint already forbids
    /// the second, so either means something between the column and
    /// this layer is wrong.
    Wire(crate::wire::Error),

    /// The bytes could not be put on the interface.
    Send(std::io::Error),

    /// The network the arrival interface resolved to was gone by the
    /// time the configuration was read.
    ///
    /// A narrow race and an operator's: the store refuses to delete a
    /// network that still has an interface attached, so reaching this
    /// takes a detach and a delete between one read and the next. It
    /// is reported rather than defaulted, because the schema's default
    /// lease time stands in for a network nobody configured and not
    /// for a network nobody has.
    NetworkGone {
        /// The network that was named.
        network: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Store(error) => write!(formatter, "the store would not answer: {error}"),
            Error::Wire(error) => write!(formatter, "an answer could not be built: {error}"),
            Error::Send(error) => write!(formatter, "the reply could not be sent: {error}"),
            Error::NetworkGone { network } => write!(
                formatter,
                "the network {network} went away between resolving the interface and reading it"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Store(error) => Some(error),
            Error::Wire(error) => Some(error),
            Error::Send(error) => Some(error),
            Error::NetworkGone { .. } => None,
        }
    }
}

impl From<teneral_store::Error> for Error {
    fn from(error: teneral_store::Error) -> Self {
        Error::Store(error)
    }
}

impl From<crate::wire::Error> for Error {
    fn from(error: crate::wire::Error) -> Self {
        Error::Wire(error)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Send(error)
    }
}
