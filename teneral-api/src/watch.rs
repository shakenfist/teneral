//! The watch stream: one reader of the history, a broadcast, and the
//! join between a consumer's first read and the events that arrive
//! while it is making it.
//!
//! Everything here is about what happens when a consumer has been
//! away, because that is where a push interface quietly becomes wrong.
//! A consumer that resumes and silently misses the event in which an
//! address changed holds a wrong answer forever, and nothing will tell
//! it. So a gap is never silent: it is a resync, named, in band as
//! data on the stream rather than as a connection error -- because a
//! connection error is the thing every client retries blindly, and
//! that retry is what turns a gap into permanent divergence.
//!
//! # Why there is a pump
//!
//! The store says when its order has moved and nothing more, so the
//! only way to learn *what* moved is to read its history -- and every
//! read of it is a view, which costs the store a thread for as long as
//! it is held. A stream that polled would therefore cost a thread per
//! poll per consumer, which makes the arrangement a matter of
//! correctness and cost rather than of tidiness.
//!
//! So one task per daemon waits on that signal, reads
//! `history_everywhere` once per movement however many consumers there
//! are, and broadcasts what it read. Every stream filters that one
//! reading for its own scope. The pump reads every network's history,
//! which is what makes the filter in [`Subscription::wants`] the thing
//! standing between one network's events and another's consumer.
//!
//! # The join, which is the line that matters
//!
//! A stream subscribes to the broadcast **before** it reads anything,
//! and only then takes its snapshot or its catch-up read. The reverse
//! order is the natural way to write it and it is wrong:
//!
//! - Subscribe, then read, and the events between the subscription and
//!   the position read at arrive twice -- once in the read and once
//!   from the broadcast. That **overlap is removable**, because every
//!   event carries its position and the order is total, so an event at
//!   or below the position already delivered can be discarded by
//!   comparing two numbers.
//! - Read, then subscribe, and every event in between is in neither.
//!   That **gap is removable by nothing**. Nothing in the stream says
//!   it happened, the consumer's state is wrong from then on, and the
//!   first anybody knows is an address that two machines believe is
//!   theirs.
//!
//! One of those two mistakes is repairable by arithmetic and the other
//! is the project's stated failure mode, which is the whole of the
//! reason the subscription comes first.
//!
//! # What a consumer is told
//!
//! A consumer with no position gets the state of every network in its
//! scope, read on **one** view so that a multi-network scope is one
//! consistent view rather than several that agree most of the time,
//! then an in-sync marker carrying that position, then the stream from
//! exactly it.
//!
//! A consumer with a position gets either the events after it and an
//! in-sync marker, or a resync naming which of the three things was
//! wrong with the position -- the epoch changed, the events were
//! evicted, or it is a position this store never issued -- and then
//! exactly what a new consumer gets. The three are three values and
//! not one message, in the store and on the wire both, because an
//! operator reads them differently and only the third means somebody
//! should go and look at the daemon. This layer translates
//! [`History`]'s four answers; it invents nothing.
//!
//! A consumer that falls behind is disconnected with a final event
//! saying so, rather than having events dropped from under it: these
//! events carry authoritative state, so a dropped lease event leaves a
//! consumer holding an address that has moved, and a count of how many
//! unspecified things went missing gives it no way back.
//!
//! An idle stream gets a heartbeat carrying a position it can resume
//! from, which is what stops a quiet network being the one most likely
//! to fall out of the retained history.

use std::sync::Arc;
use std::time::Duration;

use teneral_store::{
    BindingStore, Clock, Cursor, Epoch, Event, EventKind, History, SystemClock, TokenScope,
};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

use crate::authz::Authenticated;
use crate::error::Result;
use crate::operations::Context;
use crate::proto::{
    self, DisconnectReason, Disconnecting, EventOrigin, Heartbeat, InSync, ResourceEvent,
    ResourceKind, Response, Resync, ResyncReason, WatchEvent, WatchRequest, response, watch_event,
};
use crate::refusal::{self, Refused};
use crate::resource;
use crate::views::Views;
use crate::wire;

/// How many events a stream may fall behind by before it is
/// disconnected.
///
/// One number for the daemon rather than one per consumer, because the
/// buffer is the broadcast every stream reads from. A thousand-odd
/// events is a few seconds of slack at the rate a hypervisor fleet
/// produces them and hours at a home network's, which is enough for a
/// consumer that is briefly slow to catch up and short enough that a
/// consumer that has stopped reading altogether is noticed rather than
/// accumulated.
const BUFFERED_EVENTS: usize = 1024;

/// What a check on a request answers: the thing, or the refusal to
/// send a consumer instead of it.
///
/// Distinct from [`Result`], which is this crate's, because the two
/// mean different things here: a refusal travels to the consumer as
/// data on its stream, and a [`crate::Error`] means the connection
/// itself could not be written to.
type Checked<T> = std::result::Result<T, Refused>;

/// What the pump broadcasts.
///
/// Two things and not one, because the pump can lose its own place --
/// the history it reads is bounded, and its epoch can be replaced
/// under it -- and a stream fed only events would have no way to learn
/// that what it was being fed is no longer continuous.
#[derive(Clone, Debug)]
pub(crate) enum Movement {
    /// One event, as the store recorded it.
    ///
    /// Shared rather than copied per stream: the broadcast hands every
    /// receiver its own clone of the value, and the value being a
    /// pointer is what keeps a hundred streams from each holding a
    /// copy of the same event's state.
    Recorded(Arc<Event>),

    /// The pump could not carry on from its own position, so no stream
    /// can carry on from its own either. Every one of them resyncs.
    Lost(Lost),
}

/// Why a position cannot be carried on from.
///
/// The three refusals [`History`] answers a remembered position with,
/// translated in one place into the three the wire names. They are
/// three values rather than one message in both, and that is
/// load-bearing: a consumer that had to parse a sentence to tell them
/// apart would stop being able to the day the sentence was reworded.
#[derive(Clone, Debug)]
pub(crate) enum Lost {
    /// The events after the position have been truncated out of the
    /// retained history.
    Evicted {
        /// The highest position whose event is gone for good.
        evicted_through: Cursor,
    },

    /// The position belongs to another order, so its sequence number
    /// means nothing here.
    EpochChanged {
        /// The epoch this store issues positions in.
        store: Epoch,
    },

    /// The position is ahead of everything this store has issued,
    /// which is what a store that has gone backwards looks like from
    /// the outside.
    NeverIssued {
        /// The last position this store has issued.
        latest: Cursor,
    },
}

impl Lost {
    /// The resync a consumer at `presented` is told, which is the one
    /// place each reason is turned into the wire's.
    fn resync(&self, presented: &proto::Cursor) -> Resync {
        let mut resync = Resync {
            reason: ResyncReason::Unspecified as i32,
            presented: Some(presented.clone()),
            evicted_through: None,
            store_epoch: None,
            latest: None,
        };

        match self {
            Lost::Evicted { evicted_through } => {
                resync.reason = ResyncReason::Evicted as i32;
                resync.evicted_through = Some(proto::cursor(evicted_through));
            }
            Lost::EpochChanged { store } => {
                resync.reason = ResyncReason::EpochChanged as i32;
                resync.store_epoch = Some(store.as_str().to_owned());
            }
            Lost::NeverIssued { latest } => {
                resync.reason = ResyncReason::NeverIssued as i32;
                resync.latest = Some(proto::cursor(latest));
            }
        }

        resync
    }
}

/// What this layer does with a history answer.
///
/// [`History`] is `non_exhaustive`, so a variant added to it arrives
/// here as something this build cannot translate rather than as a
/// compile error -- which is the right way round for a dependency, and
/// the reason the arms above it are written out rather than collapsed.
/// What it must never become is a resume: telling a consumer to carry
/// on from an answer this build did not understand is the silent wrong
/// answer this whole module exists to avoid.
enum Caught {
    /// The events after the position presented, and the position they
    /// run up to.
    Resumed {
        /// Every event after the position, in order, intermediate
        /// states included.
        events: Vec<Event>,
        /// The position the events run up to, which is where the
        /// stream carries on from.
        at: Cursor,
    },

    /// The position cannot be carried on from.
    Lost(Lost),

    /// An answer this build has no translation for.
    Untranslatable,
}

impl Caught {
    /// What `history` means to a stream.
    fn of(history: History) -> Self {
        match history {
            History::Resumed { events, cursor } => Caught::Resumed { events, at: cursor },
            History::Evicted {
                evicted_through, ..
            } => Caught::Lost(Lost::Evicted { evicted_through }),
            History::EpochChanged { store, .. } => Caught::Lost(Lost::EpochChanged { store }),
            History::NeverIssued { latest, .. } => Caught::Lost(Lost::NeverIssued { latest }),
            _ => Caught::Untranslatable,
        }
    }
}

/// The one reader of the history, and the channel every stream is fed
/// from.
#[derive(Debug)]
pub(crate) struct Pump {
    /// The channel. The pump task holds the only other sender, so a
    /// stream subscribes here and the pump's own reading is what feeds
    /// it.
    movements: broadcast::Sender<Movement>,
}

impl Pump {
    /// Start the pump, and hand back the task it runs as.
    ///
    /// The task is returned rather than kept so that whatever started
    /// the daemon can wait for it: a pump still reading the history
    /// after the listener has returned would be a view this daemon
    /// left open behind itself. It stops when `stopping` says so.
    pub(crate) fn start(
        store: Arc<dyn BindingStore>,
        views: Arc<Views>,
        stopping: watch::Receiver<bool>,
    ) -> (Arc<Self>, JoinHandle<()>) {
        let (movements, _) = broadcast::channel(BUFFERED_EVENTS);
        let pump = Arc::new(Self {
            movements: movements.clone(),
        });
        let task = tokio::spawn(pump_the_history(store, views, movements, stopping));

        (pump, task)
    }

    /// A receiver that carries every movement from now on.
    ///
    /// Called before a stream reads anything. See this module's
    /// documentation: that order is what turns a gap into an overlap,
    /// and an overlap is the one of the two a comparison can remove.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Movement> {
        self.movements.subscribe()
    }
}

/// Read the history once per movement, and broadcast what it says.
///
/// The position this holds is its own: it advances only over events it
/// has actually read, so a read that failed is retried rather than
/// skipped. A skip would be a set of events no stream in the daemon
/// would ever be given and nothing would say so.
async fn pump_the_history(
    store: Arc<dyn BindingStore>,
    views: Arc<Views>,
    movements: broadcast::Sender<Movement>,
    mut stopping: watch::Receiver<bool>,
) {
    let mut positions = store.subscribe();

    // Start where the store is rather than at the beginning of its
    // history. Every stream's own first read covers everything up to
    // the position it joins at, so broadcasting the retained past
    // would be a read nobody is waiting for.
    let mut position = Cursor::clone(&positions.borrow_and_update());

    loop {
        let latest = Cursor::clone(&positions.borrow_and_update());

        if latest == position {
            // Caught up, so wait to be told the order has moved. The
            // alternative -- a timer -- would be a view, and so a
            // thread, per tick per daemon for a store nothing is
            // writing to.
            tokio::select! {
                biased;
                _ = stopping.changed() => return,
                moved = positions.changed() => {
                    if moved.is_err() {
                        // The store has gone, which is the daemon
                        // shutting down from this side of it.
                        return;
                    }
                }
            }

            continue;
        }

        // Nobody is listening, so there is nothing to read the history
        // for. The position moves on without a view being opened,
        // which is what keeps a daemon with no consumers from spending
        // a thread on every write.
        //
        // This cannot lose a stream its events. A stream subscribes
        // before it reads, and the position it then reads at is one
        // the store had already reached when this skipped -- so every
        // event skipped here is at or below that position, and is in
        // the stream's own first read.
        if movements.receiver_count() == 0 {
            position = latest;
            continue;
        }

        let view = tokio::select! {
            biased;
            _ = stopping.changed() => return,
            view = views.waiting(&*store) => view,
        };

        let Ok(view) = view else {
            // The store could not give a view. Retried rather than
            // skipped, after a pause so that a store that is failing
            // is not asked again as fast as it can refuse. If the
            // failure outlasts the retained history, the read that
            // eventually succeeds answers "evicted" and every stream
            // is told to resync -- which is the right end for a daemon
            // that could not keep up with its own store.
            if paused(&mut stopping, RETRY_AFTER_FAILURE).await {
                return;
            }

            continue;
        };

        let at = view.cursor().clone();
        let read = view.history_everywhere(&position).await;

        // Before the broadcast, because a view is a thread holding a
        // read transaction and the send below can wait on a slow
        // receiver.
        drop(view);

        match read {
            Ok(history) => match Caught::of(history) {
                Caught::Resumed { events, at } => {
                    for event in events {
                        // The result says only whether anybody was
                        // listening, which is not this task's
                        // business: a stream that went away between
                        // the count above and here is a connection
                        // that closed.
                        let _ = movements.send(Movement::Recorded(Arc::new(event)));
                    }

                    position = at;
                }

                Caught::Lost(lost) => {
                    // The pump itself cannot carry on from where it
                    // was, so neither can any stream reading from it.
                    // Each one is told, and each one converges again.
                    let _ = movements.send(Movement::Lost(lost));
                    position = at;
                }

                // Nothing to say that would be true. A stream told to
                // carry on would be carrying on from an answer this
                // build did not understand, so the position is left
                // where it is and the read is retried: either a later
                // build understands it or the history evicts under
                // this one, which every stream is told about.
                Caught::Untranslatable => {
                    if paused(&mut stopping, RETRY_AFTER_FAILURE).await {
                        return;
                    }
                }
            },

            Err(_) => {
                if paused(&mut stopping, RETRY_AFTER_FAILURE).await {
                    return;
                }
            }
        }
    }
}

/// How long the pump waits before reading again after a read it could
/// not use.
///
/// Short, because the events it has not read yet are events no
/// consumer has been given, and long enough that a store failing every
/// read is not asked again as fast as it can say no.
const RETRY_AFTER_FAILURE: Duration = Duration::from_millis(100);

/// Wait for `delay`, or until the daemon is stopping.
///
/// `true` when it is stopping, which is the caller's cue to return.
async fn paused(stopping: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    tokio::select! {
        biased;
        _ = stopping.changed() => true,
        () = tokio::time::sleep(delay) => false,
    }
}

/// What a stream was asked for.
///
/// Both halves are checked before anything is read, and both are
/// refused rather than narrowed. A consumer handed a narrower stream
/// than it asked for believes it is seeing everything in the scope it
/// named, which is exactly the silently wrong answer this design
/// exists to prevent.
struct Subscription {
    /// The networks, as the request named them.
    scope: TokenScope,
    /// The kinds of resource wanted.
    kinds: Vec<EventKind>,
}

impl Subscription {
    /// What `request` asks for, or the refusal it gets instead.
    fn of(request: &WatchRequest, caller: &Authenticated) -> Checked<Self> {
        let scope = match request.scope.as_ref().and_then(proto::scope_from_wire) {
            Some(scope) => scope,
            // A watch with no scope is refused rather than read as a
            // watch over everything, for the reason a mint's scope is:
            // the commonest way to widen a scope by accident is to
            // leave the field empty.
            None => {
                return Err(invalid(
                    "a watch says which networks it covers, and the every-network form is a value \
                     a caller writes rather than a field it leaves empty",
                ));
            }
        };

        let scope = match scope {
            TokenScope::EveryNetwork => {
                // Refused rather than narrowed to what this caller may
                // reach. A consumer that asked for every network and
                // was given one of them would be told nothing about
                // the rest and would believe there was nothing to
                // tell.
                if !caller.reaches_every_network() {
                    return Err(Box::new(refusal::outside_scope()));
                }

                TokenScope::EveryNetwork
            }

            TokenScope::Networks(names) => {
                if names.is_empty() {
                    return Err(invalid(
                        "a watch with no network in its scope is a stream that could carry \
                         nothing",
                    ));
                }

                for name in &names {
                    caller.require_reach(name)?;
                }

                // A set rather than a list: a network named twice is
                // one network, and a snapshot taken twice for it would
                // be the same resources delivered twice.
                let mut names = names;
                names.sort();
                names.dedup();

                TokenScope::Networks(names)
            }
        };

        Ok(Self {
            scope,
            kinds: kinds(&request.kinds)?,
        })
    }

    /// Whether this stream covers `network`.
    fn reaches(&self, network: &str) -> bool {
        match &self.scope {
            TokenScope::EveryNetwork => true,
            TokenScope::Networks(names) => names.iter().any(|name| name == network),
        }
    }

    /// Whether `event` belongs on this stream.
    ///
    /// One function with two callers: the catch-up read and the live
    /// delivery. A stream whose resume filtered differently from its
    /// stream would tell a consumer about a network on one path and
    /// not on the other, and the pump reads every network's history --
    /// so this is the thing standing between one network's events and
    /// another's consumer.
    fn wants(&self, event: &Event) -> bool {
        self.reaches(&event.network) && self.kinds.contains(&event.kind)
    }
}

/// The kinds a request asked for, or the refusal it gets.
///
/// An unknown kind is refused rather than dropped from the
/// subscription, which turns a consumer built against a fuller
/// vocabulary into a version mismatch it hears about instead of a
/// stream that is silently missing what it asked for.
fn kinds(values: &[i32]) -> Checked<Vec<EventKind>> {
    if values.is_empty() {
        return Err(invalid(
            "a watch says which kinds of resource it wants, and a stream of no kinds at all could \
             carry nothing",
        ));
    }

    let mut kinds = Vec::with_capacity(values.len());

    for value in values {
        let kind = ResourceKind::try_from(*value)
            .ok()
            .and_then(proto::resource_kind_from_wire);

        match kind {
            Some(kind) => kinds.push(kind),
            None => return Err(Box::new(refusal::unknown_resource_kind(*value))),
        }
    }

    Ok(kinds)
}

/// How far a stream has got.
///
/// Two positions and not one, because they answer different questions.
#[derive(Clone, Debug)]
struct Progress {
    /// The position every event in this stream's scope up to which has
    /// been delivered to the consumer. What a resync says the consumer
    /// presented, and what a disconnection says it has been given.
    delivered: Cursor,

    /// The highest position this stream has had from the pump, whether
    /// or not it was in scope.
    ///
    /// This is the position a heartbeat carries, and the difference
    /// between the two is the whole reason the heartbeat is worth
    /// anything. A stream watching a quiet network while another
    /// network is busy receives the busy one's events and delivers
    /// none of them, so this advances where the one above does not --
    /// and a consumer resuming here loses nothing, because every event
    /// at or below it was received and every one of those in scope was
    /// delivered.
    ///
    /// The store's own latest position would be the wrong thing to
    /// carry: it can include an event this stream has not been handed
    /// yet, and a consumer that resumed from it would never be told
    /// about that event.
    received: Cursor,
}

impl Progress {
    /// A stream that has just joined at `position`.
    fn at(position: Cursor) -> Self {
        Self {
            delivered: position.clone(),
            received: position,
        }
    }

    /// Whether `at` is a position in the same order as what has been
    /// delivered.
    ///
    /// Two positions are comparable only once their epochs are known
    /// to match. The pump announces an epoch change as a loss before
    /// it broadcasts anything from the new order, so this is the guard
    /// that the comparison below is never made across two orders
    /// rather than the path an epoch change ordinarily takes.
    fn comparable(&self, at: &Cursor) -> bool {
        at.epoch() == self.delivered.epoch()
    }

    /// Whether `at` has already been delivered.
    ///
    /// The dedupe at the join, and the reason subscribing before
    /// reading is safe: the events between the subscription and the
    /// position read at arrive twice, and this is what makes the
    /// second copy of each a comparison rather than a duplicate. At
    /// the position itself rather than below it, because a read
    /// answers *up to and including* the position it was taken at, so
    /// an event at exactly that position has already been given to the
    /// consumer.
    fn covers(&self, at: &Cursor) -> bool {
        at.sequence() <= self.delivered.sequence()
    }

    /// Note that `at` arrived, in scope or not.
    fn saw(&mut self, at: &Cursor) {
        if at.sequence() > self.received.sequence() {
            self.received = at.clone();
        }
    }

    /// Note that `at` was delivered, which is also to say it arrived.
    fn sent(&mut self, at: &Cursor) {
        self.saw(at);

        if at.sequence() > self.delivered.sequence() {
            self.delivered = at.clone();
        }
    }

    /// The position the consumer holds.
    fn consumer_holds(&self) -> &Cursor {
        &self.delivered
    }

    /// The position the consumer may resume from.
    fn resume_point(&self) -> &Cursor {
        &self.received
    }
}

/// Serve a watch, which is the last thing a connection does.
///
/// The request has already been checked against the caller's
/// capability; what is checked here is the scope and the kinds it
/// names, because both are a watch's own and neither is a network any
/// other operation would have checked.
pub(crate) async fn serve(
    socket: &mut UnixStream,
    context: &Context<'_>,
    pump: &Pump,
    heartbeat: Duration,
    stopping: watch::Receiver<bool>,
    request: &WatchRequest,
) -> Result<()> {
    let subscription = match Subscription::of(request, context.caller) {
        Ok(subscription) => subscription,
        Err(refusal) => return refuse(socket, *refusal).await,
    };

    // The line this whole module is built around. Subscribing before
    // reading leaves an overlap, which the comparison in
    // [`Progress::covers`] removes; reading before subscribing leaves
    // a gap, which nothing removes and nothing reports. See the
    // module's documentation for the argument in full.
    let mut movements = pump.subscribe();

    let presented = request.after.as_ref().map(presented);
    let converged = converge(socket, context, &subscription, presented.as_ref()).await?;

    let Some(progress) = converged else {
        // Refused, and the refusal has been sent.
        return Ok(());
    };

    deliver(
        socket,
        context,
        &subscription,
        &mut movements,
        progress,
        heartbeat,
        stopping,
    )
    .await
}

/// Bring a consumer up to date, and say where it now stands.
///
/// `Ok(None)` means the stream was refused and told why.
async fn converge(
    socket: &mut UnixStream,
    context: &Context<'_>,
    subscription: &Subscription,
    presented: Option<&Cursor>,
) -> Result<Option<Progress>> {
    let Some(presented) = presented else {
        return snapshot(socket, context, subscription).await;
    };

    let mut view = match context.view().await {
        Ok(view) => view,
        Err(refusal) => {
            refuse(socket, *refusal).await?;
            return Ok(None);
        }
    };

    let read = catch_up(&mut view, subscription, presented).await;
    drop(view);

    let caught = match read {
        Ok(caught) => caught,
        Err(error) => {
            refuse(socket, refusal::for_store_error(&error)).await?;
            return Ok(None);
        }
    };

    match caught {
        Caught::Resumed { events, at } => {
            // The events the consumer missed, intermediate states
            // included: a lease created and then released while it was
            // away produces both events and not neither, because a
            // consumer holding the earlier state would otherwise never
            // see it end.
            let changes = match changes(subscription, &events, EventOrigin::Change) {
                Ok(changes) => changes,
                Err(refusal) => {
                    refuse(socket, *refusal).await?;
                    return Ok(None);
                }
            };

            for change in changes {
                send_change(socket, change).await?;
            }

            send_in_sync(socket, &at).await?;

            Ok(Some(Progress::at(at)))
        }

        Caught::Lost(lost) => {
            // In band, as data on the stream, and then exactly what a
            // new consumer gets. Never a connection error: that is the
            // thing a client retries blindly, and a blind retry is how
            // a gap becomes permanent divergence.
            send_resync(socket, &lost, &proto::cursor(presented)).await?;

            snapshot(socket, context, subscription).await
        }

        Caught::Untranslatable => {
            refuse(
                socket,
                refusal::internal("translate the answer its store gave a remembered position"),
            )
            .await?;

            Ok(None)
        }
    }
}

/// Read what happened after `presented` in this stream's scope.
///
/// The every-network scope is the store's own store-global read; a
/// scope of named networks is one read per network on the one view,
/// merged. The merge is a sort, because the positions are one total
/// order across the whole store and every event carries its own.
///
/// The named-network path reads through the network-scoped history
/// deliberately rather than filtering the store-global one: that read
/// checks the network of every row it returns and refuses and counts a
/// row from another, which is a guard the store-global read has no way
/// to make.
/// A unique borrow of the view rather than a shared one, because a
/// view is `Send` and not `Sync`: a shared borrow of one cannot be
/// held across an await inside a task, and a connection is a task.
async fn catch_up(
    view: &mut crate::views::View,
    subscription: &Subscription,
    presented: &Cursor,
) -> teneral_store::Result<Caught> {
    match &subscription.scope {
        TokenScope::EveryNetwork => Ok(Caught::of(view.history_everywhere(presented).await?)),

        TokenScope::Networks(names) => {
            let mut events = Vec::new();

            for name in names {
                match Caught::of(view.history(name, presented).await?) {
                    Caught::Resumed { events: read, .. } => events.extend(read),
                    // Every network's read is answered from the
                    // position presented and the store's own marks
                    // rather than from the network, so the refusals
                    // cannot differ between them -- which is why the
                    // first is the answer rather than one of several
                    // to reconcile.
                    refusal => return Ok(refusal),
                }
            }

            events.sort_by_key(|event| event.cursor.sequence());

            Ok(Caught::Resumed {
                events,
                at: view.cursor().clone(),
            })
        }
    }
}

/// Send the state of every network in scope, read on one view, and the
/// in-sync marker that says it was the whole of it.
///
/// `Ok(None)` means the stream was refused and told why.
///
/// One view for the whole scope, which is the point: several
/// independent reads of a changing store agree most of the time, and a
/// snapshot assembled from two of them could describe two states that
/// were never true together with nothing in the result saying so.
async fn snapshot(
    socket: &mut UnixStream,
    context: &Context<'_>,
    subscription: &Subscription,
) -> Result<Option<Progress>> {
    let view = match context.view().await {
        Ok(view) => view,
        Err(refusal) => {
            refuse(socket, *refusal).await?;
            return Ok(None);
        }
    };

    let at = view.cursor().clone();

    let networks = match &subscription.scope {
        // Every network the store holds at this position, so that a
        // network created while a consumer was away is in its
        // snapshot.
        TokenScope::EveryNetwork => match view.networks().await {
            Ok(networks) => networks.into_iter().map(|network| network.name).collect(),
            Err(error) => {
                refuse(socket, refusal::for_store_error(&error)).await?;
                return Ok(None);
            }
        },

        TokenScope::Networks(names) => names.clone(),
    };

    let mut events = Vec::new();
    for network in &networks {
        // A network this view does not hold is refused rather than
        // answered with an empty snapshot, by the store. That is the
        // answer wanted: the act a consumer performs on a complete
        // snapshot is to delete whatever it was not told about, so a
        // consumer watching a network that does not exist must not be
        // told that it is empty.
        match view.snapshot(network).await {
            Ok(snapshot) => events.extend(snapshot.events),
            Err(error) => {
                refuse(socket, refusal::for_store_error(&error)).await?;
                return Ok(None);
            }
        }
    }

    drop(view);

    // By position, which across networks as well as within one puts
    // every resource after the network it belongs to: nothing could
    // have been written before the network row it is configured in.
    events.sort_by_key(|event| event.cursor.sequence());

    let changes = match changes(subscription, &events, EventOrigin::Snapshot) {
        Ok(changes) => changes,
        Err(refusal) => {
            refuse(socket, *refusal).await?;
            return Ok(None);
        }
    };

    for change in changes {
        send_change(socket, change).await?;
    }

    send_in_sync(socket, &at).await?;

    Ok(Some(Progress::at(at)))
}

/// Deliver from the broadcast until the stream ends.
///
/// Three things can end it and each is said rather than simply done: a
/// consumer that fell behind, a daemon that is stopping, and a
/// connection whose peer has gone. The first two are a final event on
/// the stream, because a closed socket is indistinguishable from a
/// network that went away and the two want different behaviour from a
/// consumer.
async fn deliver(
    socket: &mut UnixStream,
    context: &Context<'_>,
    subscription: &Subscription,
    movements: &mut broadcast::Receiver<Movement>,
    mut progress: Progress,
    heartbeat: Duration,
    mut stopping: watch::Receiver<bool>,
) -> Result<()> {
    let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
    // A stream that was blocked for several intervals owes its
    // consumer one heartbeat and not a burst of them: what the
    // consumer wants to know is the position now.
    beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;

            _ = stopping.changed() => {
                return disconnect(
                    socket,
                    DisconnectReason::ShuttingDown,
                    "this daemon is shutting down".to_owned(),
                    &progress,
                )
                .await;
            }

            movement = movements.recv() => match movement {
                Ok(Movement::Recorded(event)) => {
                    if !progress.comparable(&event.cursor) {
                        // A position from another order, which cannot
                        // be compared with the one delivered. The pump
                        // announces an epoch change before it
                        // broadcasts anything from the new order, so
                        // reaching this means the two came out of step
                        // -- and converging again is the only answer
                        // that cannot be wrong.
                        let lost = Lost::EpochChanged {
                            store: event.cursor.epoch().clone(),
                        };

                        match resynced(socket, context, subscription, &lost, &progress).await? {
                            Some(next) => progress = next,
                            None => return Ok(()),
                        }

                        continue;
                    }

                    // Everything the read at the join already covered,
                    // discarded here. This is the overlap that
                    // subscribing before reading leaves, and the one
                    // thing about it that matters is that it is
                    // removable at all.
                    if progress.covers(&event.cursor) {
                        progress.saw(&event.cursor);
                        continue;
                    }

                    if !subscription.wants(&event) {
                        // Not this stream's, but it went past: the
                        // position is still one this consumer may
                        // resume from, which is what a heartbeat
                        // carries.
                        progress.saw(&event.cursor);
                        continue;
                    }

                    match change(&event, EventOrigin::Change, now()) {
                        Ok(change) => {
                            send_change(socket, change).await?;
                            progress.sent(&event.cursor);
                        }
                        Err(refusal) => return refuse(socket, *refusal).await,
                    }
                }

                Ok(Movement::Lost(lost)) => {
                    match resynced(socket, context, subscription, &lost, &progress).await? {
                        Some(next) => progress = next,
                        None => return Ok(()),
                    }
                }

                // The consumer fell behind the buffer. Disconnected
                // with a final event rather than having the events it
                // missed dropped from under it: these carry
                // authoritative state, so a dropped lease event leaves
                // a consumer holding an address that has moved, and a
                // count of how many unspecified things went missing
                // gives it no way back. The position in the final
                // event is what makes the recovery exact.
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    return disconnect(
                        socket,
                        DisconnectReason::Lagged,
                        format!(
                            "this stream fell {missed} events behind the {BUFFERED_EVENTS} this \
                             daemon buffers"
                        ),
                        &progress,
                    )
                    .await;
                }

                // The pump has stopped, so no event can arrive on this
                // stream again. That is the daemon shutting down from
                // this side of it, and a consumer told so reconnects
                // rather than waiting on a stream that will stay
                // silent.
                Err(broadcast::error::RecvError::Closed) => {
                    return disconnect(
                        socket,
                        DisconnectReason::ShuttingDown,
                        "this daemon has stopped reading its own history".to_owned(),
                        &progress,
                    )
                    .await;
                }
            },

            _ = beat.tick() => {
                send_heartbeat(socket, progress.resume_point()).await?;
            }
        }
    }
}

/// Tell a consumer it has to converge again, and converge it.
///
/// `Ok(None)` means the stream was refused and is over. The resync
/// names the position the consumer holds rather than the one it may
/// resume from, because what it is being told is that the state it has
/// cannot be carried forward.
async fn resynced(
    socket: &mut UnixStream,
    context: &Context<'_>,
    subscription: &Subscription,
    lost: &Lost,
    progress: &Progress,
) -> Result<Option<Progress>> {
    send_resync(socket, lost, &proto::cursor(progress.consumer_holds())).await?;

    snapshot(socket, context, subscription).await
}

/// The events of `events` that belong on this stream, as the wire
/// carries them.
///
/// Converted before any of them is written, so that a build that
/// cannot read what its store wrote refuses the stream rather than
/// sending half of a snapshot and then a refusal.
fn changes(
    subscription: &Subscription,
    events: &[Event],
    origin: EventOrigin,
) -> Checked<Vec<ResourceEvent>> {
    events
        .iter()
        .filter(|event| subscription.wants(event))
        .map(|event| change(event, origin, now()))
        .collect()
}

/// One event, as the wire carries it.
///
/// Through the same converter a read of the resource goes through, for
/// the reason [`crate::resource`] gives: two spellings of one resource
/// is the defect that arrangement produces, and it stays invisible
/// until a consumer applies a streamed event over a read it was
/// already holding.
///
/// A state that does not parse is a refusal rather than an event with
/// fields missing. It means this build and the store disagree about
/// what an event carries, which is a thing to report and not to paper
/// over -- and a consumer that received the resource with a field
/// quietly defaulted would hold a wrong answer with nothing to tell
/// it.
fn change(event: &Event, origin: EventOrigin, now: i64) -> Checked<ResourceEvent> {
    let cursor = proto::cursor(&event.cursor);

    let state = match event.state.as_deref() {
        Some(state) => match resource::of_event(event.kind, state, &cursor, now) {
            Ok(resource) => Some(resource),
            Err(_) => {
                return Err(Box::new(refusal::internal(
                    "read the state its own store recorded for an event",
                )));
            }
        },
        // A deletion, which is the same shape with no state: that
        // being the post-state of something removed.
        None => None,
    };

    Ok(ResourceEvent {
        cursor: Some(cursor),
        network: event.network.clone(),
        kind: proto::resource_kind(event.kind) as i32,
        resource_id: event.resource_id.clone(),
        origin: origin as i32,
        state,
        recorded_at: event.recorded_at,
        actor: event.actor.clone(),
    })
}

/// Send one resource event.
async fn send_change(socket: &mut UnixStream, change: ResourceEvent) -> Result<()> {
    let position = change.cursor.clone();
    let network = change.network.clone();

    send(
        socket,
        position,
        Some(network),
        watch_event::Event::Change(change),
    )
    .await
}

/// Send the marker that says the consumer now holds the whole of its
/// scope.
async fn send_in_sync(socket: &mut UnixStream, at: &Cursor) -> Result<()> {
    let at = proto::cursor(at);

    send(
        socket,
        Some(at.clone()),
        None,
        watch_event::Event::InSync(InSync { cursor: Some(at) }),
    )
    .await
}

/// Send a resync, which names its reason.
async fn send_resync(
    socket: &mut UnixStream,
    lost: &Lost,
    presented: &proto::Cursor,
) -> Result<()> {
    // No position in the envelope: a resync is the daemon saying that
    // the position the consumer holds is not one to carry on from, so
    // the positions it carries are inside it where each is named.
    send(
        socket,
        None,
        None,
        watch_event::Event::Resync(lost.resync(presented)),
    )
    .await
}

/// Send a heartbeat carrying the position a consumer may resume from.
async fn send_heartbeat(socket: &mut UnixStream, at: &Cursor) -> Result<()> {
    let at = proto::cursor(at);

    send(
        socket,
        Some(at.clone()),
        None,
        watch_event::Event::Heartbeat(Heartbeat { cursor: Some(at) }),
    )
    .await
}

/// Send the last event on the stream, and end it.
async fn disconnect(
    socket: &mut UnixStream,
    reason: DisconnectReason,
    detail: String,
    progress: &Progress,
) -> Result<()> {
    let at = proto::cursor(progress.consumer_holds());

    send(
        socket,
        Some(at.clone()),
        None,
        watch_event::Event::Disconnecting(Disconnecting {
            reason: reason as i32,
            detail,
            cursor: Some(at),
        }),
    )
    .await
}

/// Write one event on to the connection.
///
/// The envelope carries the position the event sits at and the network
/// it is about, exactly as every other answer on this wire does, and
/// the event carries them again: a consumer that reads only the
/// envelope and one that reads only the event are both reading the
/// truth, and a handler that had learned to infer the network from the
/// subscription would be wrong the day somebody widened it.
async fn send(
    socket: &mut UnixStream,
    position: Option<proto::Cursor>,
    network: Option<String>,
    event: watch_event::Event,
) -> Result<()> {
    wire::write_message(
        socket,
        &Response {
            position,
            network,
            answer: Some(response::Answer::Event(WatchEvent { event: Some(event) })),
        },
    )
    .await
}

/// Refuse the stream, which ends it.
async fn refuse(socket: &mut UnixStream, refusal: proto::Refusal) -> Result<()> {
    wire::write_message(
        socket,
        &Response {
            // A refusal is not a position in anything, and it names no
            // network: a caller refused for a network it may not reach
            // must not be told the network exists.
            position: None,
            network: None,
            answer: Some(response::Answer::Refusal(refusal)),
        },
    )
    .await
}

/// A position a caller presented.
fn presented(position: &proto::Cursor) -> Cursor {
    Cursor::presented(&position.epoch, position.sequence)
}

/// The request cannot be acted on at all.
fn invalid(detail: &str) -> Refused {
    Box::new(refusal::invalid_request(detail))
}

/// Now, as the instant a lease's expiry is evaluated against.
///
/// The store's own clock type, so that this layer and the store have
/// one notion of what the time is and one precision for it. It is used
/// for exactly one thing: whether a lease in an event has run out. The
/// store evaluates that for a read and cannot for an event, an event's
/// state being text it wrote when nothing had expired.
fn now() -> i64 {
    SystemClock.unix_seconds()
}

#[cfg(test)]
mod tests {
    use super::{
        BUFFERED_EVENTS, DisconnectReason, Lost, Movement, Progress, Response, Subscription,
        deliver, kinds, response, watch_event,
    };
    use crate::authz::Authenticated;
    use crate::operations::Context;
    use crate::proto::{self, RefusalCode, ResourceKind, WatchRequest};
    use crate::testing::Directory;
    use crate::views::Views;
    use crate::wire;
    use std::sync::Arc;
    use std::time::Duration;
    use teneral_store::{Cursor, EventKind, TokenScope};
    use tokio::net::UnixStream;
    use tokio::sync::{broadcast, watch};

    /// A position in this store's order.
    fn at(sequence: u64) -> Cursor {
        Cursor::presented("abc", sequence)
    }

    /// A position in some other store's order.
    fn elsewhere(sequence: u64) -> Cursor {
        Cursor::presented("not-this-store", sequence)
    }

    /// A watch over every network, for every kind.
    fn watching_everything() -> WatchRequest {
        WatchRequest {
            scope: Some(proto::scope(&TokenScope::EveryNetwork)),
            kinds: EventKind::ALL
                .into_iter()
                .map(|kind| proto::resource_kind(kind) as i32)
                .collect(),
            after: None,
        }
    }

    #[test]
    fn an_event_at_the_position_the_join_was_made_at_is_not_delivered_twice() {
        // The dedupe at the join. A read answers up to and including
        // the position it was taken at, so the event at exactly that
        // position is one the consumer has already been given -- and
        // the broadcast carries it too, because the subscription came
        // first. A comparison that was off by one in this direction
        // would deliver it a second time, which a consumer applying
        // events in order would read as the resource changing twice.
        let progress = Progress::at(at(7));

        assert!(progress.covers(&at(7)));
        assert!(progress.covers(&at(6)));
    }

    #[test]
    fn the_first_event_after_the_join_is_delivered() {
        // The other direction of the same comparison, and the one that
        // matters more: an off-by-one here discards the first event
        // after the join, and the consumer is never told. That is the
        // silent gap this module exists to make impossible, arrived at
        // from the other side.
        let progress = Progress::at(at(7));

        assert!(!progress.covers(&at(8)));
        assert!(!progress.covers(&at(9)));
    }

    #[test]
    fn a_position_from_another_order_is_not_compared_with_one_from_this() {
        // Sequence numbers from two epochs are two different orders'
        // numbers, so comparing them says nothing. What the stream
        // does with an event from another order is converge again; the
        // point here is that it does not reach the comparison.
        let progress = Progress::at(at(7));

        assert!(progress.comparable(&at(8)));
        assert!(!progress.comparable(&elsewhere(8)));
        assert!(!progress.comparable(&elsewhere(1)));
    }

    #[test]
    fn a_heartbeat_carries_a_position_past_what_this_stream_was_given() {
        // The reason the two positions are two. An event out of this
        // stream's scope went past, so the consumer may resume from
        // it; what it holds is still the last thing it was handed.
        let mut progress = Progress::at(at(7));
        progress.saw(&at(9));

        assert_eq!(progress.resume_point(), &at(9));
        assert_eq!(progress.consumer_holds(), &at(7));

        progress.sent(&at(11));

        assert_eq!(progress.resume_point(), &at(11));
        assert_eq!(progress.consumer_holds(), &at(11));
    }

    #[test]
    fn each_of_the_three_things_wrong_with_a_position_is_its_own_reason() {
        let presented = proto::cursor(&at(7));

        let evicted = Lost::Evicted {
            evicted_through: at(9),
        }
        .resync(&presented);
        assert_eq!(evicted.reason, proto::ResyncReason::Evicted as i32);
        assert_eq!(
            evicted.evicted_through.map(|cursor| cursor.sequence),
            Some(9)
        );
        assert_eq!(evicted.latest, None);

        let changed = Lost::EpochChanged {
            store: at(1).epoch().clone(),
        }
        .resync(&presented);
        assert_eq!(changed.reason, proto::ResyncReason::EpochChanged as i32);
        assert_eq!(changed.store_epoch.as_deref(), Some("abc"));
        assert_eq!(changed.evicted_through, None);

        let never = Lost::NeverIssued { latest: at(3) }.resync(&presented);
        assert_eq!(never.reason, proto::ResyncReason::NeverIssued as i32);
        assert_eq!(never.latest.map(|cursor| cursor.sequence), Some(3));

        // And each says what the consumer presented, which is what
        // tells an operator reading a log which consumer it was about.
        assert_eq!(never.presented, Some(presented));
    }

    #[test]
    fn a_watch_with_no_scope_or_no_kind_is_refused_rather_than_widened() {
        let caller = Authenticated::OwningPeer(0);

        let nothing = WatchRequest {
            scope: None,
            ..watching_everything()
        };
        assert_eq!(
            Subscription::of(&nothing, &caller)
                .err()
                .map(|refusal| refusal.code),
            Some(RefusalCode::InvalidRequest as i32)
        );

        let empty = WatchRequest {
            scope: Some(proto::scope(&TokenScope::Networks(Vec::new()))),
            ..watching_everything()
        };
        assert_eq!(
            Subscription::of(&empty, &caller)
                .err()
                .map(|refusal| refusal.code),
            Some(RefusalCode::InvalidRequest as i32)
        );

        let kindless = WatchRequest {
            kinds: Vec::new(),
            ..watching_everything()
        };
        assert_eq!(
            Subscription::of(&kindless, &caller)
                .err()
                .map(|refusal| refusal.code),
            Some(RefusalCode::InvalidRequest as i32)
        );
    }

    #[test]
    fn a_kind_this_daemon_does_not_serve_is_refused_rather_than_dropped() {
        // A consumer built against a fuller vocabulary hears about the
        // mismatch instead of watching a stream that never carries
        // what it asked for.
        let refusal = kinds(&[proto::resource_kind(EventKind::Lease) as i32, 7])
            .expect_err("a kind this build cannot name is refused");

        assert_eq!(refusal.code, RefusalCode::UnknownResourceKind as i32);

        // And the unspecified value is not a kind either: a field
        // nobody set is not a subscription.
        let unset =
            kinds(&[ResourceKind::Unspecified as i32]).expect_err("a kind nobody set is refused");
        assert_eq!(unset.code, RefusalCode::UnknownResourceKind as i32);
    }

    #[test]
    fn a_network_named_twice_is_watched_once() {
        let caller = Authenticated::OwningPeer(0);
        let request = WatchRequest {
            scope: Some(proto::scope(&TokenScope::Networks(vec![
                "alpha".to_owned(),
                "alpha".to_owned(),
            ]))),
            ..watching_everything()
        };

        let subscription =
            Subscription::of(&request, &caller).expect("the owning peer may watch any network");

        match subscription.scope {
            TokenScope::Networks(names) => assert_eq!(names, vec!["alpha".to_owned()]),
            other => panic!("the scope came back as {other:?}"),
        }
    }

    #[test]
    fn the_buffer_is_what_a_lagging_consumer_is_measured_against() {
        // The number is in the final event a disconnected consumer
        // reads, so it is worth one assertion that it is the one the
        // design chose rather than whatever a later edit left behind.
        assert_eq!(BUFFERED_EVENTS, 1024);
    }

    #[tokio::test]
    async fn a_consumer_that_fell_behind_is_disconnected_with_a_final_event() {
        // Nothing is dropped silently. The alternative -- a bounded
        // queue that discards the oldest and reports a count -- is
        // right for diagnostics and wrong for these events, which
        // carry authoritative state: a dropped lease event leaves a
        // consumer holding an address that has moved, and a count of
        // how many unspecified things went missing gives it no way
        // back.
        let directory = Directory::new("lagged");
        let store = directory.store().await;
        let views = Views::new(4);
        let caller = Authenticated::OwningPeer(0);
        let context = Context {
            store: &*store,
            tokens: &*store,
            caller: &caller,
            views: &views,
        };

        let (mut client, mut server) = UnixStream::pair().expect("a socket pair for a test");

        // A buffer of two, overflowed before the stream reads any of
        // it, which is what a consumer that has stopped reading does
        // to the real one.
        let (movements, mut receiver) = broadcast::channel(2);
        for sequence in 1..=5 {
            let _ = movements.send(Movement::Recorded(Arc::new(recorded(sequence))));
        }

        let (_stopping, stopped) = watch::channel(false);
        let subscription = Subscription::of(&watching_everything(), &caller)
            .expect("the owning peer may watch everything");

        deliver(
            &mut server,
            &context,
            &subscription,
            &mut receiver,
            Progress::at(at(0)),
            // Longer than this test, so that the heartbeat cannot be
            // what ends the stream.
            Duration::from_secs(60),
            stopped,
        )
        .await
        .expect("a stream that ends says so rather than failing");

        // The daemon's end of the connection, closed as it would be by
        // the connection task returning. Dropped before the reads
        // below so that what follows the final event is the end of the
        // stream rather than a wait for something that is not coming.
        drop(server);

        let response: Response = wire::read_message(&mut client)
            .await
            .expect("the consumer is told why the stream ended");

        match answered(&response) {
            watch_event::Event::Disconnecting(disconnecting) => {
                assert_eq!(disconnecting.reason, DisconnectReason::Lagged as i32);
                assert!(
                    disconnecting.detail.contains("behind"),
                    "the final event says what happened: {}",
                    disconnecting.detail
                );
                // The position it has been delivered up to, which is
                // what makes the recovery exact rather than a
                // reconnection into the present.
                assert_eq!(
                    disconnecting.cursor.map(|cursor| cursor.sequence),
                    Some(0),
                    "the final event names no position to resume from"
                );
            }
            other => panic!("the stream ended with {other:?}"),
        }

        // And it ended: nothing follows the final event.
        assert!(
            wire::read_message::<_, Response>(&mut client)
                .await
                .is_err(),
            "the stream carried on after saying it was disconnecting"
        );
    }

    /// One event the pump might have broadcast.
    fn recorded(sequence: u64) -> teneral_store::Event {
        teneral_store::Event {
            cursor: at(sequence),
            network: "alpha".to_owned(),
            kind: EventKind::Network,
            resource_id: "alpha".to_owned(),
            state: Some("{\"name\":\"alpha\",\"written_by\":\"peer:0\"}".to_owned()),
            recorded_at: Some(0),
            actor: "peer:0".to_owned(),
        }
    }

    /// The watch event an answer carries, or a panic naming what
    /// arrived instead.
    fn answered(response: &Response) -> watch_event::Event {
        match response.answer.as_ref() {
            Some(response::Answer::Event(event)) => event
                .event
                .clone()
                .expect("a watch event says which event it is"),
            other => panic!("a watch stream carried {other:?} rather than an event"),
        }
    }
}
