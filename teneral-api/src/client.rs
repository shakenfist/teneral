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
//!
//! # Two ways to read a stream, and the second is the one to use
//!
//! [`Connection::watch`] hands out a [`Stream`], which is one
//! connection's worth of events and ends when that connection does.
//! [`Watch`] is the same stream across connections: it holds the
//! position it may resume from, reconnects with a bounded backoff,
//! presents that position, and hands its caller the resync or the
//! disconnection rather than papering over either.
//!
//! The transport is why this is written here rather than left to each
//! consumer. A browser's event-source client replays the last event
//! id by itself, and that id is the position; nothing on a socket
//! carrying protobuf does, so the replay has to be written
//! somewhere. Written once here it can be reviewed and tested;
//! written in each consumer it is the one mechanism whose failure
//! nobody notices.
//!
//! # The two positions that are not the same position
//!
//! What a hand-rolled version gets wrong is not the backoff. It is
//! that the last position a stream *delivered* is not always a
//! position the next connection may *present*:
//!
//! - A snapshot's events carry the position each resource was last
//!   written at, which is not a place in the stream. Resuming from
//!   the highest of them asks the daemon to carry on from a point the
//!   consumer never reached, and the rest of the snapshot -- every
//!   resource whose last write was earlier -- is then never sent
//!   again. The consumer holds part of the world believing it holds
//!   all of it, which is the failure this interface is arranged
//!   against, and no error is reported to anybody.
//! - A heartbeat's position *is* one to resume from, and it is the
//!   one a quiet consumer most needs: it is how a stream that has
//!   heard nothing for hours stays inside the retained history
//!   instead of resyncing.
//!
//! So the position here advances from a change, from an in-sync
//! marker and from a heartbeat, and from nothing else -- and while a
//! snapshot is in flight it advances from nothing at all. A
//! connection lost mid-snapshot therefore presents the position held
//! before that snapshot began and is answered with another one, which
//! costs the daemon a read and costs the consumer nothing it can be
//! wrong about.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::net::UnixStream;

use crate::error::{Error, Result};
use crate::proto::{
    Cursor, EventOrigin, Hello, HelloResult, Request, Response, WatchEvent, WatchRequest, Welcome,
    hello_result, request, response, watch_event,
};
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

    /// Make one request and read the one answer it produces.
    ///
    /// A refusal comes back as the answer it is rather than as an
    /// error: it travels inside the envelope the daemon put it in, and
    /// a caller that wants to know both that a request was refused and
    /// what the daemon said about it reads one value.
    ///
    /// This is for the operations that answer once. A collection
    /// arrives as a sequence ended by a marker, and a caller that read
    /// one message of one would leave the rest on the connection to be
    /// read as the answer to whatever it asked next -- so a collection
    /// goes through [`Connection::request_collection`] instead.
    pub async fn request(&mut self, request: request::Kind) -> Result<Response> {
        self.send(request).await?;

        wire::read_message(&mut self.stream).await
    }

    /// Make one request and read the whole of the collection it
    /// answers with, the marker included.
    ///
    /// The marker is kept rather than dropped, because the count it
    /// carries is the check a consumer makes against what it kept: a
    /// connection that failed halfway through a collection otherwise
    /// looks exactly like a collection that was that short. A refusal
    /// ends the reading too, there being no collection to wait for.
    pub async fn request_collection(&mut self, request: request::Kind) -> Result<Vec<Response>> {
        self.send(request).await?;

        let mut answers = Vec::new();
        loop {
            let response: Response = wire::read_message(&mut self.stream).await?;
            let ended = matches!(
                response.answer,
                Some(response::Answer::CollectionEnd(_)) | Some(response::Answer::Refusal(_))
            );

            answers.push(response);

            if ended {
                return Ok(answers);
            }
        }
    }

    /// Write one request on to the connection.
    async fn send(&mut self, request: request::Kind) -> Result<()> {
        wire::write_message(
            &mut self.stream,
            &Request {
                kind: Some(request),
            },
        )
        .await
    }

    /// Ask for a stream, which is the last thing a connection does.
    ///
    /// Consuming, because a watch is terminal: nothing follows it on
    /// the connection but events, so a caller that could still make a
    /// request would be a caller holding a connection whose answers
    /// are no longer answers to anything it asked.
    ///
    /// The request is written and nothing is read, so a refusal
    /// arrives from [`Stream::next`] rather than from here -- which is
    /// what keeps a refused watch from consuming the first event of a
    /// watch that was not refused.
    pub async fn watch(self, request: WatchRequest) -> Result<Stream> {
        let (mut socket, welcome) = self.into_parts();

        wire::write_message(
            &mut socket,
            &Request {
                kind: Some(request::Kind::Watch(request)),
            },
        )
        .await?;

        Ok(Stream { socket, welcome })
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

/// A watch stream: a connection that carries events and nothing else.
///
/// What a consumer does with it is read until it stops, and the one
/// thing it must not do is treat a resync as a reason to reconnect
/// blindly. A resync arrives here as an event, which means a consumer
/// has to parse it to carry on reading -- so the only consumer that
/// can ignore one is a consumer that has stopped reading, which is the
/// whole reason a gap is data on the stream and not a connection
/// error.
pub struct Stream {
    /// The socket, past the handshake and past the request.
    socket: UnixStream,
    /// What the daemon said when the connection opened.
    welcome: Welcome,
}

impl Stream {
    /// What the daemon said when the connection opened: its position,
    /// its schema version, and the scope and capabilities this caller
    /// was granted.
    pub fn welcome(&self) -> &Welcome {
        &self.welcome
    }

    /// The next event, or why there will not be one.
    ///
    /// A refusal is an error here rather than an answer, unlike a
    /// request's: a refused watch has no stream, so there is nothing
    /// for a caller to go on reading and the refusal is the whole of
    /// what happened. The final event of a stream that is being
    /// disconnected is an ordinary event, delivered as one, and the
    /// error after it is the connection closing.
    pub async fn next(&mut self) -> Result<WatchEvent> {
        let response: Response = wire::read_message(&mut self.socket).await?;

        match response.answer {
            Some(response::Answer::Event(event)) => Ok(event),
            Some(response::Answer::Refusal(refusal)) => Err(Error::Refused {
                refusal: Box::new(refusal),
            }),
            other => Err(Error::Unexpected {
                detail: format!("a watch stream carried {other:?} rather than an event"),
            }),
        }
    }
}

/// Where a [`Watch`] reconnects to, what it presents, and how
/// patiently.
///
/// The request is a template rather than a one-off: its scope and its
/// kinds are what every connection asks for, and its `after` is where
/// the watch starts -- which is how a consumer that persists
/// [`Watch::position`] across its own restarts carries on from where
/// it left off rather than from the present. Every reconnection after
/// that replaces `after` with the position the watch is holding.
#[derive(Clone, Debug)]
pub struct WatchConfig {
    /// The socket to connect to, read again on every attempt so that
    /// a daemon restarted behind the same path is reached.
    pub socket_path: PathBuf,

    /// The credential to present, or none for the owning peer.
    pub token_secret: Option<String>,

    /// The scope and kinds every connection asks for, and the
    /// position the first one presents.
    pub request: WatchRequest,

    /// How long to wait before the first attempt at a reconnection.
    ///
    /// Short, because the common interruption is a daemon being
    /// restarted and a consumer that waited seconds for it would make
    /// a one-second restart look like a one-second outage to
    /// everything downstream. Not zero, because a daemon that accepts
    /// a connection and immediately drops it would otherwise be met
    /// with a loop as fast as the machine can open sockets.
    pub first_delay: Duration,

    /// The longest it will ever wait between attempts.
    ///
    /// This is the number that decides what a daemon which is *gone*
    /// costs: without a ceiling the doubling eventually waits hours,
    /// and with no doubling at all a consumer left running against a
    /// socket nobody serves is a busy loop in somebody's logs
    /// forever.
    pub max_delay: Duration,

    /// Whether a refusal is something to try again.
    ///
    /// Off, because a refusal is an answer rather than a failure: a
    /// credential the daemon does not hold, a scope wider than that
    /// credential reaches, or a kind this daemon does not know will
    /// be refused in exactly the same terms on every attempt, and a
    /// helper that retried would present a credential that cannot
    /// work until somebody noticed the socket's load.
    ///
    /// The one deployment it is here for is a consumer started before
    /// the credential it is to use has been minted. Turning it on
    /// anywhere else converts a loud failure into a quiet one that
    /// only [`Observation::Reconnecting`] mentions.
    pub retry_refusals: bool,
}

impl WatchConfig {
    /// The default first delay: prompt enough that a daemon restart
    /// is not an outage.
    pub const FIRST_DELAY: Duration = Duration::from_millis(100);

    /// The default ceiling: often enough that a daemon coming back is
    /// noticed within half a minute, rare enough to cost nothing.
    pub const MAX_DELAY: Duration = Duration::from_secs(30);

    /// Watch `request`'s scope on the socket at `socket_path`, as the
    /// owning peer, with the delays above.
    pub fn new(socket_path: impl Into<PathBuf>, request: WatchRequest) -> Self {
        Self {
            socket_path: socket_path.into(),
            token_secret: None,
            request,
            first_delay: Self::FIRST_DELAY,
            max_delay: Self::MAX_DELAY,
            retry_refusals: false,
        }
    }

    /// The same, presenting `secret` on every connection.
    pub fn with_credential(mut self, secret: impl Into<String>) -> Self {
        self.token_secret = Some(secret.into());

        self
    }
}

/// What a [`Watch`] hands its caller.
///
/// Two variants and not one, because a consumer has to be able to
/// tell an event the daemon sent from this crate's own account of a
/// connection that went away. Everything the daemon says travels in
/// [`Observation::Event`] exactly as it arrived -- a resync is a
/// resync and the final disconnection is the event the daemon sent,
/// not a flag derived from it -- which is what keeps a consumer that
/// treats a resync as fatal able to do so.
#[derive(Debug)]
pub enum Observation {
    /// An event, as the daemon sent it.
    Event(WatchEvent),

    /// The connection ended and another will be opened.
    ///
    /// Handed to the caller rather than dealt with silently: a
    /// consumer that is told nothing cannot report that it spent four
    /// minutes reconnecting, and a reconnection is the one moment at
    /// which its state is of an unknown age.
    Reconnecting(Reconnecting),
}

/// A connection that ended, and what the watch will do about it.
#[derive(Debug)]
pub struct Reconnecting {
    /// What ended it.
    pub cause: Error,

    /// The position the next connection will present, or none if the
    /// watch has no position it may resume from -- which asks for a
    /// snapshot rather than resuming from the present.
    pub from: Option<Cursor>,

    /// Which attempt the next one will be, counting from one and
    /// reset by every connection that delivers an event.
    pub attempt: u32,

    /// How long the watch will wait before making it.
    pub delay: Duration,
}

/// A watch that outlives its connections.
///
/// Driven like a stream -- call [`Watch::next`] until it answers
/// `None` -- and holding, between calls, the one piece of state a
/// consumer must not get wrong: the position it may resume from. The
/// module's own notes say which positions those are and why a
/// snapshot's are not among them.
///
/// It reconnects after a disconnection and after a transport failure,
/// and it surrenders everything else to its caller. A refusal, a
/// frame that did not decode and an answer of a shape this exchange
/// does not allow are all the same kind of thing: the two ends
/// disagree, or the daemon has said no, and another connection would
/// be answered identically.
///
/// [`Watch::next`] is not cancellation-safe, for the reason
/// [`Stream::next`] is not: it is reading one length-delimited
/// message off a socket, and a read abandoned halfway leaves the next
/// one starting mid-message.
pub struct Watch {
    /// Where to connect, what to ask for, and how long to wait.
    config: WatchConfig,

    /// The connection being read, if there is one.
    stream: Option<Stream>,

    /// The position the next connection presents.
    ///
    /// The whole point of the type. It advances only from the things
    /// a daemon says a consumer may resume from, and never backwards
    /// within an epoch, so that a snapshot's events -- which carry
    /// the position each resource was last written at -- cannot drag
    /// it to a place in the stream the consumer never reached.
    resume_from: Option<Cursor>,

    /// Whether a snapshot is in flight.
    ///
    /// While it is, no position is taken from anything. A snapshot is
    /// only complete at its in-sync marker, so a connection lost
    /// during one has delivered a part of the world and must ask for
    /// the whole of it again.
    ///
    /// This is about completeness rather than about which message a
    /// position arrived in, which is why it is not the same check as
    /// the one on an event's origin. Today's daemon sends nothing but
    /// a snapshot's own events inside a snapshot, and the wire
    /// promises no such thing: a heartbeat sent during a long one
    /// would be serving the contract as written, and the position it
    /// carried would be a real place in the stream that this consumer
    /// still may not resume from.
    in_snapshot: bool,

    /// How many attempts since the last connection that delivered
    /// something.
    attempt: u32,

    /// What the next attempt waits first, set when the last one was
    /// announced so that the announcement and the wait are the same
    /// number.
    delay: Option<Duration>,

    /// Whether a terminal error has already been handed to the
    /// caller.
    ///
    /// Once it has, this stops rather than reconnecting: the error
    /// was the answer, and a caller looping over `next` without
    /// examining what it returns gets a loop that ends instead of one
    /// that hammers a socket with a credential that will never work.
    finished: bool,
}

impl Watch {
    /// A watch that has not connected yet. The first [`Watch::next`]
    /// opens the connection.
    pub fn new(config: WatchConfig) -> Self {
        let resume_from = config.request.after.clone();

        Self {
            config,
            stream: None,
            resume_from,
            in_snapshot: false,
            attempt: 0,
            delay: None,
            finished: false,
        }
    }

    /// The position a reconnection would present.
    ///
    /// What a consumer persists if it wants its own restart to be a
    /// resume: put it back in [`WatchConfig::request`]'s `after`.
    /// Absent until the first thing arrives that may be resumed from,
    /// and absent again for nothing -- it never goes backwards.
    pub fn position(&self) -> Option<&Cursor> {
        self.resume_from.as_ref()
    }

    /// What the daemon said when the connection in hand opened, or
    /// none while there is no connection.
    ///
    /// Worth reading again after a reconnection rather than
    /// remembering: the daemon on the other end may have been
    /// restarted, and a scope or an epoch that changed under a
    /// consumer is exactly what it wants to know.
    pub fn welcome(&self) -> Option<&Welcome> {
        self.stream.as_ref().map(Stream::welcome)
    }

    /// The next observation, or `None` once this watch has finished.
    ///
    /// It finishes only by handing the caller an error first, so a
    /// loop that reads until `None` has already been told why. Every
    /// other outcome is an observation: the daemon's events, and this
    /// crate's account of the connections it opens to get them.
    pub async fn next(&mut self) -> Option<Result<Observation>> {
        if self.finished {
            return None;
        }

        loop {
            // Read from the connection in hand, or open one. The two
            // are in one loop because opening is not an answer: a
            // caller gets an event or an account of a connection
            // ending, and never a turn of this loop by itself.
            let read = match self.stream.as_mut() {
                Some(stream) => stream.next().await,
                None => match self.open().await {
                    Ok(()) => continue,
                    Err(error) => Err(error),
                },
            };

            match read {
                Ok(event) => {
                    // A connection that has delivered something is
                    // one the daemon is serving, so the backoff
                    // starts again from the first delay. Resetting on
                    // the connection alone would let a daemon that
                    // accepts and drops be retried at full speed
                    // forever.
                    self.attempt = 0;
                    self.absorb(&event);

                    return Some(Ok(Observation::Event(event)));
                }
                Err(error) => return Some(self.interrupted(error)),
            }
        }
    }

    /// Open a connection and ask it for the watch, presenting the
    /// position this watch holds.
    async fn open(&mut self) -> Result<()> {
        if let Some(delay) = self.delay.take() {
            tokio::time::sleep(delay).await;
        }

        let connection = Connection::connect(
            &self.config.socket_path,
            self.config.token_secret.as_deref(),
        )
        .await?;

        let mut request = self.config.request.clone();
        request.after = self.resume_from.clone();

        // A connection presenting no position is answered with a
        // snapshot, and one presenting a position may still be: the
        // resync that says so sets this again when it arrives.
        self.in_snapshot = request.after.is_none();
        self.stream = Some(connection.watch(request).await?);

        Ok(())
    }

    /// Decide what a failed read or a failed connection means, and
    /// either announce a reconnection or hand the error over.
    fn interrupted(&mut self, error: Error) -> Result<Observation> {
        self.stream = None;

        if !self.reconnects_after(&error) {
            self.finished = true;

            return Err(error);
        }

        self.attempt = self.attempt.saturating_add(1);
        let delay = self.backoff();
        self.delay = Some(delay);

        Ok(Observation::Reconnecting(Reconnecting {
            cause: error,
            from: self.resume_from.clone(),
            attempt: self.attempt,
            delay,
        }))
    }

    /// Whether `error` is the kind of thing another connection would
    /// fix.
    ///
    /// A socket that went away and a peer that closed are; a refusal
    /// is an answer, and the remaining variants mean the two ends
    /// disagree about the protocol, which is not a condition a retry
    /// improves. The catch-all is deliberate as well as required --
    /// [`Error`] is open, and a variant added later is surfaced
    /// rather than quietly retried until somebody reads this
    /// function.
    fn reconnects_after(&self, error: &Error) -> bool {
        match error {
            Error::Io { .. } | Error::Closed { .. } => true,
            Error::Refused { .. } => self.config.retry_refusals,
            _ => false,
        }
    }

    /// How long the attempt just counted waits: the first delay,
    /// doubled once per attempt before it, up to the ceiling.
    fn backoff(&self) -> Duration {
        // Clamped before the shift rather than after the
        // multiplication: a consumer left running for a week reaches
        // an attempt count that would overflow the doubling, and the
        // answer wanted there is the ceiling rather than a panic in a
        // debug build and a short delay in a release one.
        let doublings = self.attempt.saturating_sub(1).min(31);
        let factor = 1_u32.checked_shl(doublings).unwrap_or(u32::MAX);

        self.config
            .first_delay
            .saturating_mul(factor)
            .min(self.config.max_delay)
    }

    /// Take from `event` whatever it says about where this watch may
    /// resume.
    fn absorb(&mut self, event: &WatchEvent) {
        match event.event.as_ref() {
            // A snapshot's event carries the position its resource
            // was last written at, which is not a place in the
            // stream; only a change's is.
            Some(watch_event::Event::Change(change)) => {
                if change.origin == EventOrigin::Change as i32 {
                    self.advance(change.cursor.as_ref());
                }
            }

            // The marker that makes a snapshot complete, and so the
            // first position after one that may be presented.
            Some(watch_event::Event::InSync(marker)) => {
                self.in_snapshot = false;
                self.advance(marker.cursor.as_ref());
            }

            // The reason a quiet consumer stays inside the retained
            // history. Ignoring these is the defect that only shows
            // up on the network nothing happens on.
            Some(watch_event::Event::Heartbeat(beat)) => self.advance(beat.cursor.as_ref()),

            // What follows is a snapshot, so nothing until its
            // marker is a position to resume from.
            Some(watch_event::Event::Resync(_)) => self.in_snapshot = true,

            // The position the daemon says it delivered up to, which
            // is what makes recovering from a disconnection exact.
            Some(watch_event::Event::Disconnecting(ending)) => {
                self.advance(ending.cursor.as_ref());
            }

            None => {}
        }
    }

    /// Move the position to `position`, if that is forwards.
    fn advance(&mut self, position: Option<&Cursor>) {
        if self.in_snapshot {
            return;
        }

        let Some(position) = position else {
            return;
        };

        // An epoch that differs is a different order, whose sequence
        // numbers cannot be compared with the held one's at all: the
        // only thing to do with a position from the order the daemon
        // is now issuing is to take it. Within one epoch the
        // comparison is what keeps a snapshot's events, which arrive
        // in position order from a low position, from dragging this
        // backwards.
        let forwards = match self.resume_from.as_ref() {
            None => true,
            Some(held) => held.epoch != position.epoch || position.sequence > held.sequence,
        };

        if forwards {
            self.resume_from = Some(position.clone());
        }
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

#[cfg(test)]
mod tests {
    //! The position bookkeeping, driven directly.
    //!
    //! What a [`Watch`] does with an event does not depend on the
    //! socket it came off, and the rules worth checking here are the
    //! ones a daemon cannot be made to exercise over one. Today's
    //! daemon sends nothing but a snapshot's own events inside a
    //! snapshot, so the two guards against taking a position from an
    //! unfinished one -- the origin of an event, and knowing a
    //! snapshot is in flight -- cover the same case over a socket and
    //! each hides the other's absence. The wire promises neither: a
    //! daemon that sent a heartbeat during a long snapshot would be
    //! serving the contract as written, and a consumer that advanced
    //! its resume point on one would hold a part of the world
    //! believing it held all of it. So each guard is checked here, on
    //! its own.

    use super::*;
    use crate::proto::{Disconnecting, Heartbeat, InSync, ResourceEvent, Resync};

    /// A position in this store's order.
    fn at(sequence: u64) -> Cursor {
        Cursor {
            epoch: "this-store".to_owned(),
            sequence,
        }
    }

    /// A position in somebody else's, which is what a minted epoch
    /// leaves every consumer holding.
    fn elsewhere(sequence: u64) -> Cursor {
        Cursor {
            epoch: "another-store".to_owned(),
            sequence,
        }
    }

    /// A watch that has never connected, which is all the
    /// bookkeeping needs.
    fn watch() -> Watch {
        Watch::new(WatchConfig::new(
            "/nonexistent",
            WatchRequest {
                scope: None,
                kinds: Vec::new(),
                after: None,
            },
        ))
    }

    /// One event of a kind, as the daemon would send it.
    fn sent(event: watch_event::Event) -> WatchEvent {
        WatchEvent { event: Some(event) }
    }

    /// An event about a resource, from a snapshot or from a change.
    fn about_a_resource(origin: EventOrigin, cursor: Cursor) -> WatchEvent {
        sent(watch_event::Event::Change(ResourceEvent {
            cursor: Some(cursor),
            origin: origin as i32,
            ..Default::default()
        }))
    }

    #[test]
    fn a_snapshots_own_events_are_not_positions_to_resume_from() {
        // The position in one of them is where that resource was last
        // written, which is not a place in the stream. A watch that
        // took the highest of them would ask a later daemon to carry
        // on from a point it never reached, and every resource whose
        // last write was earlier would never be sent again.
        let mut watch = watch();

        watch.absorb(&about_a_resource(EventOrigin::Snapshot, at(99)));

        assert_eq!(watch.position(), None);

        // A change's position is one to resume from, which is what
        // makes the distinction above a distinction and not a refusal
        // to advance at all.
        watch.absorb(&about_a_resource(EventOrigin::Change, at(5)));

        assert_eq!(watch.position(), Some(&at(5)));
    }

    #[test]
    fn nothing_inside_an_unfinished_snapshot_moves_the_position() {
        // A resync says a snapshot is coming, and until its marker
        // arrives the consumer holds a part of the world. Every
        // position that arrives meanwhile is a real place in the
        // stream and still must not be taken: resuming from one would
        // ask for the events after a snapshot the consumer never
        // finished receiving.
        let mut watch = watch();
        watch.absorb(&about_a_resource(EventOrigin::Change, at(5)));
        watch.absorb(&sent(watch_event::Event::Resync(Resync::default())));

        watch.absorb(&sent(watch_event::Event::Heartbeat(Heartbeat {
            cursor: Some(at(99)),
        })));
        watch.absorb(&sent(watch_event::Event::Disconnecting(Disconnecting {
            cursor: Some(at(98)),
            ..Default::default()
        })));

        assert_eq!(watch.position(), Some(&at(5)));

        // And the marker is what makes the snapshot whole, so it is
        // the first position after one that may be presented.
        watch.absorb(&sent(watch_event::Event::InSync(InSync {
            cursor: Some(at(70)),
        })));

        assert_eq!(watch.position(), Some(&at(70)));
    }

    #[test]
    fn the_position_does_not_go_backwards_inside_one_order() {
        // A resume delivers in position order, so this is not the
        // common case; it is what keeps a single event that arrives
        // out of order from undoing the catch-up around it, which
        // would have a daemon send that stretch of history again and
        // a consumer apply it twice.
        let mut watch = watch();
        watch.absorb(&about_a_resource(EventOrigin::Change, at(9)));
        watch.absorb(&about_a_resource(EventOrigin::Change, at(4)));

        assert_eq!(watch.position(), Some(&at(9)));

        watch.absorb(&sent(watch_event::Event::Heartbeat(Heartbeat {
            cursor: Some(at(12)),
        })));

        assert_eq!(watch.position(), Some(&at(12)));
    }

    #[test]
    fn a_position_from_another_order_is_taken_rather_than_compared() {
        // Two epochs' sequence numbers are two orders' numbers, so
        // comparing them says nothing at all: the whole point of the
        // epoch is that a remembered position from the order that has
        // gone cannot be mistaken for one in the order the daemon is
        // issuing now. A watch that compared them would keep
        // presenting the old order's position and be resynced for it
        // on every reconnection.
        let mut watch = watch();
        watch.absorb(&about_a_resource(EventOrigin::Change, at(9)));
        watch.absorb(&sent(watch_event::Event::InSync(InSync {
            cursor: Some(elsewhere(1)),
        })));

        assert_eq!(watch.position(), Some(&elsewhere(1)));
    }

    #[test]
    fn the_delay_between_attempts_is_bounded_however_long_a_daemon_is_gone() {
        // A consumer left running against a socket nobody serves
        // reaches an attempt count that would overflow the doubling
        // inside a week. What is wanted there is the ceiling, and the
        // arithmetic that produces it rather than a panic in a debug
        // build is not something a test over a socket can reach: it
        // would have to wait out every delay up to it.
        let mut watch = watch();
        watch.config.first_delay = Duration::from_millis(100);
        watch.config.max_delay = Duration::from_secs(30);

        watch.attempt = 1;
        assert_eq!(watch.backoff(), Duration::from_millis(100));

        watch.attempt = 4;
        assert_eq!(watch.backoff(), Duration::from_millis(800));

        watch.attempt = 40;
        assert_eq!(watch.backoff(), Duration::from_secs(30));

        watch.attempt = u32::MAX;
        assert_eq!(watch.backoff(), Duration::from_secs(30));
    }
}
