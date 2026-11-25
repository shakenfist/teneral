//! What a consumer links.
//!
//! A connection, a handshake, and the answers that follow. This is the
//! half another repository depends on: the product whose interface is
//! a browser forwards what it reads here rather than holding a model
//! of the resources of its own, which is what keeps the rules that
//! scope one network's answers from existing in two places.
//!
//! The reconnect bookkeeping belongs here too, for the same reason.
//! Every consumer that hand-rolls it gets it wrong in the same way,
//! and the failure is silent divergence: a consumer that resumes a
//! stream and misses the event in which an address changed holds a
//! wrong answer forever and nothing tells it.

use std::path::Path;
use std::time::Duration;

use tokio::net::UnixStream;

use crate::error::{Error, Result};
use crate::proto::{Hello, HelloResult, Welcome, hello_result};
use crate::{PROTOCOL_VERSION, wire};

/// An open connection to a daemon, past its handshake.
pub struct Connection {
    /// The socket.
    stream: UnixStream,
    /// What the daemon said about itself and about this caller.
    welcome: Welcome,
}

impl Connection {
    /// Connect to the socket at `path` and complete the handshake.
    ///
    /// `token_secret` is omitted only by the peer the daemon runs as,
    /// which is how the first credential comes to exist. Anything else
    /// omitting it is refused.
    ///
    /// A refusal arrives as [`Error::Refused`] carrying the refusal
    /// itself, not a sentence rendered out of it: a caller deciding
    /// what to do next reads the code, and a version it cannot speak
    /// is a different thing to do next from a credential it should
    /// replace.
    pub async fn connect(path: &Path, token_secret: Option<&str>) -> Result<Self> {
        let mut stream = UnixStream::connect(path)
            .await
            .map_err(|error| Error::io(format!("connecting to {}", path.display()), error))?;

        let hello = Hello {
            protocol_version: PROTOCOL_VERSION,
            token_secret: token_secret.map(str::to_owned),
        };

        match greet(&mut stream, &hello).await?.outcome {
            Some(hello_result::Outcome::Welcome(welcome)) => Ok(Self { stream, welcome }),
            Some(hello_result::Outcome::Refusal(refusal)) => Err(Error::Refused {
                refusal: Box::new(refusal),
            }),
            None => Err(Error::Unexpected {
                detail: "the daemon answered the handshake with neither a welcome nor a refusal"
                    .to_owned(),
            }),
        }
    }

    /// What the daemon said when the connection opened: its position,
    /// its schema version, and the scope and capabilities this caller
    /// was granted.
    ///
    /// Worth comparing with what the caller believes it holds. A
    /// consumer that finds it was served as the owning peer when it
    /// meant to present a credential is a consumer running with more
    /// reach than it thinks.
    pub fn welcome(&self) -> &Welcome {
        &self.welcome
    }

    /// Take the socket and the welcome apart.
    ///
    /// The seam between the handshake and whatever reads the
    /// connection next. It is here rather than inside this type so
    /// that the handshake has no opinion about what follows it: the
    /// requests, the stream, and the reconnect bookkeeping are all
    /// written against a connection that is already past its
    /// handshake, and a caller holding the pieces can be any of them.
    pub fn into_parts(self) -> (UnixStream, Welcome) {
        (self.stream, self.welcome)
    }
}

/// Send a Hello and read what comes back.
///
/// Shared with the daemon's own startup check, which is a handshake
/// and not a connect: a path that accepts a connection may be a
/// socket somebody left bound with nothing serving it, and the
/// difference matters when the question is whether to refuse to start.
pub(crate) async fn greet(stream: &mut UnixStream, hello: &Hello) -> Result<HelloResult> {
    wire::write_message(stream, hello).await?;

    wire::read_message(stream).await
}

/// Whether something is already answering a handshake at `path`.
///
/// Used by the daemon before it binds. Anything that answers -- with a
/// welcome or with a refusal -- is something serving that path, and a
/// refusal is the likelier of the two: the probe presents no
/// credential, and the daemon it is probing has no reason to think the
/// probing process is its own owning peer.
///
/// A path that accepts a connection and then says nothing within
/// `timeout` is in use as well, and so is one that accepts and then
/// fails. The question being asked is whether anything holds that
/// socket, and only the kernel refusing the connection answers it
/// with a no: a socket file whose daemon is gone is closed with it, so
/// connecting to one fails rather than hanging. Anything that accepts
/// is something to leave alone -- taking a socket from a live process
/// would leave it holding the store and answering nobody, which is
/// worse than refusing to start and saying so.
///
/// The handshake is attempted rather than only the connection because
/// an answer is what makes the refusal legible: it says a teneral is
/// there, not merely that something is.
pub(crate) async fn is_answering(path: &Path, timeout: Duration) -> bool {
    let mut stream = match UnixStream::connect(path).await {
        Ok(stream) => stream,
        Err(_) => return false,
    };

    let hello = Hello {
        protocol_version: PROTOCOL_VERSION,
        token_secret: None,
    };

    // The outcome is not read: a welcome and a refusal both say
    // something is serving, and so does a timeout. What this waits for
    // is the chance of an answer, bounded, so that a daemon does not
    // hang at startup on a socket that accepts and never speaks.
    let _ = tokio::time::timeout(timeout, greet(&mut stream, &hello)).await;

    true
}
