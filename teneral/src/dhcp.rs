//! Serving DHCPv4, when the daemon was asked to.
//!
//! Absent `--dhcp-interface` this module does nothing at all and the
//! daemon is exactly what it is without it: no privileged port is
//! bound, no socket below the IP layer is opened, and nothing here
//! runs. Naming the interfaces is deliberate rather than serving every
//! interface the store holds an attachment for -- a process that bound
//! a privileged port because somebody wrote a row would have turned a
//! configuration write into a privileged action, and the capabilities
//! this needs are ones an operator should be granting for named
//! interfaces.
//!
//! # What is refused, and when
//!
//! An interface named on the command line that no network has claimed
//! is a refusal to start rather than a warning. A daemon that bound
//! the protocol's port and then answered nothing on it would be worse
//! than one that did not come up: the clients on that link would wait
//! out their timeouts against a server that was listening, and the
//! operator would have no line to read. The same goes for an interface
//! whose address an answer cannot be derived from, because every
//! option but the lease time comes from that address.
//!
//! Both are read again for every request, because both can change
//! while the daemon runs: an interface can be detached and an address
//! can be removed. The difference is what happens -- at startup it is
//! a refusal to come up, and afterwards it is a line in the log and a
//! request that goes unanswered.
//!
//! # What the startup line is for
//!
//! One line per served interface, naming the network that claimed it,
//! the namespace it is attached under, the address and prefix derived
//! from it, and the lease time it will hand out. That is the whole of
//! where an answer's contents come from, made visible, and it is the
//! first thing to read when a client got an answer nobody expected.
//!
//! # Stopping
//!
//! Each interface is a task reading its own socket, and they are told
//! to stop and waited for rather than cancelled with the runtime. The
//! order matters: a task holds a handle on the store, so a store
//! dropped while one was still reading would be a daemon blocked on a
//! thread something was still using.

use std::sync::Arc;

use teneral_dhcp::datapath::{
    self, Interfaces, RECEIVE_BUFFER_BYTES, ReceiveFailure, Receiver, Sender,
};
use teneral_dhcp::decide::{Arrival, Decider};
use teneral_dhcp::wire;
use teneral_store::{BindingStore, SqliteStore};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::cli::ServeArgs;
use crate::serve::log;

type Failure = Box<dyn std::error::Error>;

/// How many reads in a row may fail before the socket is the problem.
///
/// A condition about one datagram cannot repeat without datagrams
/// arriving, and the three the datapath reports cannot be provoked by
/// a client -- so a run this long says the read itself is failing for
/// a reason the classification did not see, and the bound is what
/// stops the loop spinning on it. High enough that no plausible run
/// of genuinely bad datagrams reaches it.
const FAILURES_BEFORE_THE_SOCKET_IS_BLAMED: u32 = 64;

/// The sockets this daemon serves DHCPv4 on, and how to stop them.
pub struct Service {
    /// Told once, read by every task.
    stop: watch::Sender<bool>,
    /// Set by a task whose socket can no longer be read.
    ///
    /// Shared rather than cloned per task because a `watch` sender is
    /// not clonable, and every task needs to be able to say this.
    failed: Arc<watch::Sender<bool>>,
    /// One task per served interface.
    tasks: Vec<JoinHandle<()>>,
}

impl Service {
    /// Watch for a socket this service can no longer serve on.
    ///
    /// The daemon selects on this beside its signals, because a
    /// process holding the protocol's port and answering nothing on it
    /// is the state this module's startup refusals exist to prevent --
    /// and reaching that state while running is no better than
    /// starting in it. Stopping hands the operator's supervisor
    /// something it can act on.
    pub fn failed(&self) -> watch::Receiver<bool> {
        self.failed.subscribe()
    }

    /// Stop serving, and wait for every socket to be closed.
    ///
    /// Returns only when each task has returned, which is when its
    /// sockets are closed and its handle on the store is given back.
    pub async fn stop(self) {
        // A send with no receivers left means every task has already
        // returned, which is the outcome being asked for.
        let _ = self.stop.send(true);

        for task in self.tasks {
            // A task that panicked has already said so on its own
            // account; there is nothing further to report and nothing
            // to do about it while stopping.
            let _ = task.await;
        }
    }
}

/// Open a socket for each interface named, and start answering on it.
///
/// `None` when no interface was named, which is the case in which this
/// daemon is what it was before DHCP existed.
pub async fn start(store: &Arc<SqliteStore>, args: &ServeArgs) -> Result<Option<Service>, Failure> {
    if args.dhcp_interface.is_empty() {
        return Ok(None);
    }

    named_once(&args.dhcp_interface)?;

    let (stop, _) = watch::channel(false);
    let failed = Arc::new(watch::channel(false).0);
    let mut tasks = Vec::with_capacity(args.dhcp_interface.len());

    for ifname in &args.dhcp_interface {
        let (receiver, sender) = open(store, ifname, args.dhcp_port).await?;
        let decider = Decider::new(Arc::clone(store) as Arc<dyn BindingStore>, sender);

        tasks.push(tokio::spawn(answer_on(
            receiver,
            decider,
            stop.subscribe(),
            Arc::clone(&failed),
        )));
    }

    Ok(Some(Service {
        stop,
        failed,
        tasks,
    }))
}

/// Refuse a list that names an interface twice.
///
/// Two sockets on one port and one interface is a conflict the kernel
/// reports as a port already in use, which names neither the flag nor
/// the interface that caused it. Checked over the whole list before
/// anything is opened, so that the refusal comes before a socket does.
fn named_once(interfaces: &[String]) -> Result<(), Failure> {
    let mut seen: Vec<&str> = Vec::with_capacity(interfaces.len());

    for ifname in interfaces {
        if seen.contains(&ifname.as_str()) {
            return Err(format!("--dhcp-interface {ifname} was given more than once").into());
        }
        seen.push(ifname);
    }

    Ok(())
}

/// Check one interface, open its sockets, and say what it will answer
/// with.
///
/// Everything in the line this logs is read here rather than
/// remembered: the network from the attachment, the lease time from
/// that network, and the address and prefix from the interface itself.
async fn open(
    store: &Arc<SqliteStore>,
    ifname: &str,
    port: u16,
) -> Result<(Receiver, Sender), Failure> {
    let namespace = datapath::UNNAMESPACED;
    let attachment = store
        .interface_owner(namespace, ifname)
        .await
        .map_err(|error| format!("asking which network has claimed {ifname}: {error}"))?
        .ok_or_else(|| {
            format!(
                "no network has claimed the interface {ifname} in the namespace {namespace}; \
                 attach it before asking this daemon to serve DHCP on it"
            )
        })?;

    let view = store
        .read()
        .await
        .map_err(|error| format!("reading the store: {error}"))?;
    let network = view
        .network(&attachment.network)
        .await
        .map_err(|error| format!("reading the network {}: {error}", attachment.network))?
        .ok_or_else(|| {
            format!(
                "the network {} claims {ifname} and no longer exists",
                attachment.network
            )
        })?;
    drop(view);

    // The interface before the sockets, so that an interface nothing
    // can be derived from is refused without a privileged port having
    // been bound on it.
    let facts = Interfaces::open()
        .and_then(|interfaces| interfaces.facts(ifname))
        .map_err(|error| format!("serving DHCPv4 on {ifname}: {error}"))?;
    let (receiver, sender) = datapath::open(namespace, ifname, port)
        .map_err(|error| format!("serving DHCPv4 on {ifname}: {error}"))?;

    log(&format!(
        "serving DHCPv4 on {ifname} in the namespace {namespace}: network {}, address {}/{}, \
         lease {} seconds, port {port}",
        attachment.network,
        facts.address(),
        facts.prefix_length(),
        network.lease_seconds,
    ));

    Ok((receiver, sender))
}

/// Read one interface's socket until the daemon stops.
///
/// One datagram at a time: the request being decided holds a view of
/// the store, and reading the next datagram before this one is
/// answered would make how many views this process holds a number a
/// client chose rather than a number an operator configured.
async fn answer_on(
    receiver: Receiver,
    decider: Decider<Sender>,
    mut stop: watch::Receiver<bool>,
    failed: Arc<watch::Sender<bool>>,
) {
    let mut buffer = Vec::with_capacity(RECEIVE_BUFFER_BYTES);
    let mut consecutive = 0_u32;

    loop {
        let arrived = tokio::select! {
            arrived = receiver.receive(&mut buffer) => arrived,
            _ = stop.changed() => return,
        };

        match arrived {
            Ok(arrived) => {
                consecutive = 0;
                answer(&receiver, &decider, arrived.ifindex, &buffer).await;
            }
            // One datagram the kernel could not describe. It is lost
            // and said so, and the next one is still worth waiting
            // for: a control message that did not fit or arrived
            // without an interface says nothing about the socket.
            Err(failure) if after(&failure, consecutive) == Cost::TheDatagram => {
                consecutive += 1;
                log(&format!(
                    "a datagram on {} was dropped rather than answered: {failure}",
                    receiver.ifname()
                ));
            }
            // The socket cannot be read, or has failed this way so
            // many times in a row that the classification above is
            // not describing what is happening. Either way this
            // process is holding the protocol's port on a link it can
            // no longer answer on, so it stops rather than staying up
            // as a server that listens and says nothing.
            Err(failure) => {
                log(&format!(
                    "reading a datagram on {}: {failure}; DHCPv4 can no longer be served \
                     on it, and this daemon is stopping rather than holding the port \
                     without answering",
                    receiver.ifname()
                ));

                // A send with no receivers is the daemon already
                // shutting down for its own reasons, which is the
                // outcome this was asking for.
                let _ = failed.send(true);

                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::watch;

    use super::{
        Answerable, Cost, FAILURES_BEFORE_THE_SOCKET_IS_BLAMED, ReceiveFailure, after, answerable,
        wait_for_failure,
    };

    /// Long enough to be sure a future that was going to resolve has,
    /// and short enough that four of them are not felt.
    const A_MOMENT: Duration = Duration::from_millis(50);

    #[test]
    fn a_datagram_from_the_interface_this_socket_serves_is_answered_from_it() {
        assert_eq!(
            answerable(Some("gw-n1"), "gw-n1"),
            Answerable::From("gw-n1")
        );
    }

    #[test]
    fn a_datagram_that_arrived_on_another_interface_is_dropped_and_names_it() {
        // Two networks, each with an interface, in one daemon: the
        // whole point of asking the kernel where a datagram came from
        // is that answering this one from gw-n1's network is the
        // cross-network answer this project exists to prevent. The
        // name travels with the refusal so that the line logged about
        // it can say which interface it was.
        assert_eq!(
            answerable(Some("gw-n2"), "gw-n1"),
            Answerable::ArrivedElsewhere("gw-n2")
        );
    }

    #[test]
    fn a_datagram_whose_interface_has_no_name_is_dropped_rather_than_assumed() {
        // The socket serves exactly one interface, so an index that
        // cannot be named is probably that interface -- and probably
        // is not an answer. Nothing confirmed where this datagram
        // arrived, so nothing may be derived from where it did not.
        assert_eq!(answerable(None, "gw-n1"), Answerable::ArrivedNowhereNamed);
    }

    #[test]
    fn a_datagram_the_kernel_could_not_describe_costs_only_that_datagram() {
        // The condition the datapath synthesises when a control
        // message does not fit or arrives without an interface. The
        // loop this feeds used to return on it, which stopped serving
        // the interface while leaving the daemon holding its port.
        let lost = ReceiveFailure::from(std::io::Error::other(
            "the kernel reported no arrival interface for a datagram",
        ));

        assert_eq!(after(&lost, 0), Cost::TheDatagram);
        assert_eq!(
            after(&lost, FAILURES_BEFORE_THE_SOCKET_IS_BLAMED - 1),
            Cost::TheDatagram
        );
    }

    #[test]
    fn a_socket_that_cannot_be_read_costs_the_daemon() {
        // EBADF, written as its number because this crate has no
        // libc dependency and one for a test would be a dependency
        // the whole binary carries.
        let dead = ReceiveFailure::from(std::io::Error::from_raw_os_error(9));

        assert_eq!(after(&dead, 0), Cost::TheSocket);
    }

    #[test]
    fn a_run_of_lost_datagrams_long_enough_is_the_socket_after_all() {
        // The bound exists because the classification is a reading of
        // what an error means, and a reading can be wrong. Without it,
        // an error this code calls per-datagram but which never clears
        // is a loop that spins forever.
        let lost = ReceiveFailure::from(std::io::Error::other("something unforeseen"));

        assert_eq!(
            after(&lost, FAILURES_BEFORE_THE_SOCKET_IS_BLAMED),
            Cost::TheSocket,
            "a condition that repeats this many times is not about one datagram"
        );
    }

    #[tokio::test]
    async fn a_daemon_serving_no_interface_waits_forever() {
        // The case the daemon is in without --dhcp-interface, where
        // resolving would stop a daemon nobody asked to serve DHCP.
        let outcome = tokio::time::timeout(A_MOMENT, wait_for_failure(None)).await;

        assert!(outcome.is_err(), "it resolved, and nothing had failed");
    }

    #[tokio::test]
    async fn a_socket_that_failed_stops_the_wait() {
        let (sender, watcher) = watch::channel(false);
        let waiting = tokio::spawn(wait_for_failure(Some(watcher)));

        sender.send(true).expect("the waiter is listening");

        tokio::time::timeout(A_MOMENT, waiting)
            .await
            .expect("the wait ends when a socket fails")
            .expect("the waiter did not panic");
    }

    #[tokio::test]
    async fn a_failure_before_the_wait_began_is_not_missed() {
        // A task can fail while the daemon is still assembling the
        // future that waits for it, and a wait that only watched for
        // changes would then wait for a second failure that never
        // comes.
        let (sender, watcher) = watch::channel(false);
        sender.send(true).expect("nothing is listening yet");

        tokio::time::timeout(A_MOMENT, wait_for_failure(Some(watcher)))
            .await
            .expect("the value was already there to be read");
    }

    #[tokio::test]
    async fn a_service_dropped_while_stopping_is_not_a_failure() {
        // Every sender gone is the daemon shutting down for its own
        // reasons. Reporting that as a DHCP failure would put a line
        // in the log blaming the sockets for every ordinary SIGTERM.
        let (sender, watcher) = watch::channel(false);
        drop(sender);

        let outcome = tokio::time::timeout(A_MOMENT, wait_for_failure(Some(watcher))).await;

        assert!(outcome.is_err(), "a closed channel is not a failure");
    }
}

/// What one failed read cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cost {
    /// The datagram in hand. Serving continues.
    TheDatagram,
    /// The socket, and so this daemon.
    TheSocket,
}

/// What a failed read cost, given how many have failed in a row.
///
/// Separate from the loop because the loop cannot be driven from a
/// test -- it owns a socket the build container may not open -- and
/// this decision is the whole of what the fix for a daemon that
/// retired an interface and stayed up consists of.
fn after(failure: &ReceiveFailure, consecutive: u32) -> Cost {
    if failure.is_per_datagram() && consecutive < FAILURES_BEFORE_THE_SOCKET_IS_BLAMED {
        Cost::TheDatagram
    } else {
        Cost::TheSocket
    }
}

/// Wait for a served socket to fail, or forever when none is served.
///
/// `None` is the daemon that was not asked to serve DHCP at all, and
/// it waits forever so that the caller can select on this without
/// knowing which daemon it is.
pub async fn wait_for_failure(watcher: Option<watch::Receiver<bool>>) {
    let Some(mut watcher) = watcher else {
        std::future::pending::<()>().await;

        return;
    };

    loop {
        if *watcher.borrow_and_update() {
            return;
        }

        // Every sender gone means the service was dropped, which
        // happens when the daemon is already stopping. That is not a
        // failure to report, so this waits out whatever else is
        // stopping the daemon rather than claiming one.
        if watcher.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Whether a datagram may be answered, and from which interface.
///
/// Carries the name rather than answering yes, so that a caller cannot
/// go on to answer from a name this did not approve: the name a reply
/// is derived from is the name the network is resolved from, and the
/// one failure this whole project is shaped around is one network's
/// answer leaving by another's interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answerable<'a> {
    /// Yes, from the named interface.
    From(&'a str),
    /// No: the interface index the kernel reported has no name here,
    /// so where the datagram arrived cannot be established at all.
    ArrivedNowhereNamed,
    /// No: it arrived on an interface this socket does not serve.
    ArrivedElsewhere(&'a str),
}

/// Judge one datagram's arrival against the interface being served.
///
/// A separate function for the same reason [`after`] is one: the loop
/// around it owns a socket the build container may not open, so a
/// branch left inside it is a branch no test can reach. This is the
/// daemon's own check that a request is answered from the network that
/// claimed the interface it actually arrived on, and it is worth more
/// than its two comparisons.
///
/// An index with no name is refused rather than assumed. The shortcut
/// is tempting -- this socket serves exactly one interface, so an
/// unnameable index is *probably* that one -- and it is the same
/// mistake in a different costume: answering from an interface nothing
/// confirmed the datagram arrived on.
fn answerable<'a>(arrived_on: Option<&'a str>, served: &str) -> Answerable<'a> {
    match arrived_on {
        None => Answerable::ArrivedNowhereNamed,
        Some(name) if name == served => Answerable::From(name),
        Some(name) => Answerable::ArrivedElsewhere(name),
    }
}

/// Answer one datagram, or say why it was not answered.
async fn answer(receiver: &Receiver, decider: &Decider<Sender>, ifindex: u32, datagram: &[u8]) {
    // The name the kernel has for the interface the datagram arrived
    // on, rather than the name this socket was opened with. The two
    // agree, the socket serving one interface, and asking is what
    // makes the network a request is answered from follow the kernel's
    // own account of where it came from.
    let named = receiver.name_of(ifindex);
    if let Err(error) = &named {
        log(&format!(
            "a datagram arrived on interface {ifindex}, which has no name: {error}"
        ));
    }
    let named = named.ok();

    let arrived_on = match answerable(named.as_deref(), receiver.ifname()) {
        Answerable::From(name) => name,
        // Already logged above, where the error that explains it was
        // in hand.
        Answerable::ArrivedNowhereNamed => return,
        Answerable::ArrivedElsewhere(name) => {
            log(&format!(
                "a datagram that arrived on {name} reached the socket serving {}, and was \
                 dropped rather than answered from the wrong network",
                receiver.ifname()
            ));

            return;
        }
    };

    let facts = match receiver.facts() {
        Ok(facts) => facts,
        Err(error) => {
            log(&format!(
                "a request on {arrived_on} was not answered: {error}"
            ));

            return;
        }
    };

    let request = match wire::parse(datagram) {
        Ok(request) => request,
        Err(refusal) => {
            log(&format!(
                "a datagram on {arrived_on} was refused: {refusal}"
            ));

            return;
        }
    };

    let arrival = Arrival {
        namespace: datapath::UNNAMESPACED,
        ifname: arrived_on,
        server: facts,
    };

    match decider.answer(&arrival, &request).await {
        Ok(decided) => {
            for note in &decided.notes {
                log(&note.to_string());
            }
        }
        Err(error) => log(&format!(
            "a request on {arrived_on} was not answered: {error}"
        )),
    }
}
