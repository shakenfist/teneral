//! What this crate returns when it cannot do what was asked.
//!
//! Two kinds of thing go wrong here and they are not the same kind.
//!
//! A **refusal** is an answer: the daemon understood the request and
//! will not serve it, and it says why in a vocabulary a consumer can
//! branch on. Refusals travel on the wire as data, and on this side
//! they arrive as [`Error::Refused`] carrying the message itself
//! rather than a sentence rendered out of it.
//!
//! A **failure** is the machinery underneath: a socket that went away,
//! a frame that did not decode, a length prefix over the bound. Those
//! are the rest of the variants, and a consumer handles them the way
//! it handles any connection failure -- by reconnecting, which is
//! exactly what the cursor makes safe.

use std::error::Error as StdError;
use std::fmt;
use std::path::PathBuf;

use crate::proto::Refusal;

/// The result of an operation in this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Why an operation on the control plane did not happen.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The daemon refused, and the refusal says why.
    ///
    /// The message itself rather than a copy of its fields, so that
    /// nothing is lost between the wire and the caller: a consumer
    /// deciding whether to retry reads the code, and a consumer
    /// logging the failure reads the detail.
    Refused {
        /// The refusal as it arrived.
        refusal: Box<Refusal>,
    },

    /// A read or a write on the socket failed. `context` says what was
    /// being attempted, because the operating system's message never
    /// does.
    Io {
        /// What was being attempted, as a phrase that completes
        /// "while ...".
        context: String,
        /// The operating system's complaint.
        source: std::io::Error,
    },

    /// The peer closed the connection.
    ///
    /// Its own variant rather than an unexpected end of file, because
    /// it is the ordinary way a connection finishes and a server that
    /// logged it as a failure would log one line per client.
    Closed {
        /// What was being waited for when the connection went.
        context: String,
    },

    /// A frame's length prefix was over the bound, so its body was
    /// never read.
    ///
    /// The bound is checked against the prefix rather than after the
    /// fact, which is the whole point of the variant: a length that
    /// came off a socket is not a size to allocate first and judge
    /// afterwards.
    TooLarge {
        /// The length the prefix claimed.
        length: u64,
    },

    /// A frame's length prefix was not a length this wire can carry.
    ///
    /// A varint longer than the bound needs is either a different
    /// protocol on the socket or a deliberate attempt at an
    /// arithmetic that overflows, and neither is worth continuing a
    /// connection for.
    Framing {
        /// What was wrong with it.
        detail: String,
    },

    /// A frame did not decode as the message it should have been.
    Decode {
        /// What was being read.
        context: String,
        /// The decoder's complaint.
        source: prost::DecodeError,
    },

    /// The peer's answer was not a shape this exchange allows: an
    /// answer with no answer in it, or one carrying something the
    /// request could not have produced.
    ///
    /// Distinct from a decode failure. The bytes were a valid message;
    /// what was wrong was which message, which means the two ends
    /// disagree about the protocol rather than about the encoding, and
    /// that is a different thing to go and look at.
    Unexpected {
        /// What was expected and what arrived.
        detail: String,
    },

    /// Something is already serving the socket path.
    ///
    /// A reason to stop rather than to take over. A daemon that
    /// replaced a live one's socket would leave it holding the store
    /// and answering nobody, which is worse than refusing to start and
    /// saying so.
    InUse {
        /// The path something else is answering on.
        path: PathBuf,
    },

    /// The socket was created but could not be given the mode the
    /// daemon intends.
    ///
    /// The mode is the only thing that decides who may connect at all,
    /// so serving on a socket whose mode is not the one asked for is
    /// serving on an unknown permission.
    Mode {
        /// The socket.
        path: PathBuf,
        /// The mode that was asked for.
        wanted: u32,
        /// What the file system reports instead.
        found: u32,
    },
}

impl Error {
    /// Wrap an operating-system error with what was being attempted.
    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Error::Io {
            context: context.into(),
            source,
        }
    }

    /// The refusal this error carries, if it is one.
    ///
    /// Here rather than at each call site so that a caller deciding
    /// whether to retry does not have to match a variant to find out
    /// whether it was refused or merely disconnected.
    pub fn refusal(&self) -> Option<&Refusal> {
        match self {
            Error::Refused { refusal } => Some(refusal),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Refused { refusal } => {
                write!(f, "the daemon refused the request: {}", refusal.detail)
            }
            Error::Io { context, source } => write!(f, "while {context}: {source}"),
            Error::Closed { context } => {
                write!(f, "the connection closed while {context}")
            }
            Error::TooLarge { length } => write!(
                f,
                "a frame claimed {length} bytes, over the {} this wire carries, so its body was \
                 not read",
                crate::wire::MAX_MESSAGE_BYTES
            ),
            Error::Framing { detail } => write!(f, "the frame's length is unreadable: {detail}"),
            Error::Decode { context, source } => {
                write!(f, "while decoding {context}: {source}")
            }
            Error::Unexpected { detail } => write!(f, "the peer answered unexpectedly: {detail}"),
            Error::InUse { path } => write!(
                f,
                "something is already answering on {}, so this daemon will not take it over",
                path.display()
            ),
            Error::Mode {
                path,
                wanted,
                found,
            } => write!(
                f,
                "{} was created but is mode {found:04o} rather than the {wanted:04o} asked for",
                path.display()
            ),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::Decode { source, .. } => Some(source),
            _ => None,
        }
    }
}
