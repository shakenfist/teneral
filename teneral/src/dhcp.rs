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

use teneral_dhcp::datapath::{self, Interfaces, RECEIVE_BUFFER_BYTES, Receiver, Sender};
use teneral_dhcp::decide::{Arrival, Decider};
use teneral_dhcp::wire;
use teneral_store::{BindingStore, SqliteStore};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::cli::ServeArgs;
use crate::serve::log;

type Failure = Box<dyn std::error::Error>;

/// The sockets this daemon serves DHCPv4 on, and how to stop them.
pub struct Service {
    /// Told once, read by every task.
    stop: watch::Sender<bool>,
    /// One task per served interface.
    tasks: Vec<JoinHandle<()>>,
}

impl Service {
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
    let mut tasks = Vec::with_capacity(args.dhcp_interface.len());

    for ifname in &args.dhcp_interface {
        let (receiver, sender) = open(store, ifname, args.dhcp_port).await?;
        let decider = Decider::new(Arc::clone(store) as Arc<dyn BindingStore>, sender);

        tasks.push(tokio::spawn(answer_on(receiver, decider, stop.subscribe())));
    }

    Ok(Some(Service { stop, tasks }))
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
async fn answer_on(receiver: Receiver, decider: Decider<Sender>, mut stop: watch::Receiver<bool>) {
    let mut buffer = Vec::with_capacity(RECEIVE_BUFFER_BYTES);

    loop {
        let arrived = tokio::select! {
            arrived = receiver.receive(&mut buffer) => arrived,
            _ = stop.changed() => return,
        };

        match arrived {
            Ok(arrived) => answer(&receiver, &decider, arrived.ifindex, &buffer).await,
            // Nothing a client sends reaches here: the readiness this
            // read follows has already happened, so what is left is a
            // socket that cannot be read at all. Serving this
            // interface stops, loudly, rather than spinning on an
            // error that will not clear.
            Err(error) => {
                log(&format!(
                    "reading a datagram on {}: {error}; DHCPv4 is no longer served on it",
                    receiver.ifname()
                ));

                return;
            }
        }
    }
}

/// Answer one datagram, or say why it was not answered.
async fn answer(receiver: &Receiver, decider: &Decider<Sender>, ifindex: u32, datagram: &[u8]) {
    // The name the kernel has for the interface the datagram arrived
    // on, rather than the name this socket was opened with. The two
    // agree, the socket serving one interface, and asking is what
    // makes the network a request is answered from follow the kernel's
    // own account of where it came from.
    let arrived_on = match receiver.name_of(ifindex) {
        Ok(ifname) => ifname,
        Err(error) => {
            log(&format!(
                "a datagram arrived on interface {ifindex}, which has no name: {error}"
            ));

            return;
        }
    };
    if arrived_on != receiver.ifname() {
        log(&format!(
            "a datagram that arrived on {arrived_on} reached the socket serving {}, and was \
             dropped rather than answered from the wrong network",
            receiver.ifname()
        ));

        return;
    }

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
        ifname: &arrived_on,
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
