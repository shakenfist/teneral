//! teneral's DHCPv4 front-end as a subject of the rig.
//!
//! One captured request, against the configuration the case records,
//! through the real parser, the real decision layer and the real
//! store. Nothing here is a stand-in for any of those: the whole value
//! of a differential case is that teneral's own bytes go beside
//! dnsmasq's, so a fake anywhere in this path would make the case an
//! assertion about the fake.
//!
//! What is *not* here is the socket. A reply leaves by one of three
//! paths -- broadcast, unicast by ordinary routing, or a hand-built
//! frame below the IP layer -- and two of them need a privilege this
//! test target does not have. So the rig asks the send path for its
//! choice of delivery, which is a value, and compares that against
//! where the captured reply went. What is left untested here is "the
//! bytes left the right file descriptor", which is what the lab
//! measures with a real client instead.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use teneral_dhcp::datapath::{self, UNNAMESPACED};
use teneral_dhcp::decide::{Arrival, Decider, Egress, Reply};
use teneral_dhcp::wire::{self, ServerFacts};
use teneral_store::{
    Actor, BindingStore, ClientIdentity, Clock, Selector, SelectorKind, SqliteStore,
};

use crate::corpus::Configuration;
use crate::frame;
use crate::subject::{Answered, Delivery, Sent};

/// The actor the case's own configuration is written by.
///
/// A peer rather than the daemon: this is the operator's half of the
/// fixture, and a case that recorded its own setup as the front-end
/// would make the front-end's writes indistinguishable from them.
const OPERATOR: Actor = Actor::Peer(1000);

/// Distinguishes the store directories of cases in one run.
static NEXT: AtomicU32 = AtomicU32::new(0);

/// A clock stopped well inside the range a lease time can be added
/// to.
///
/// Stopped rather than real so that a lease configured by the case is
/// still live when the request is answered, however long the run takes
/// and whatever the lease time in the case is.
#[derive(Debug)]
struct Stopped;

impl Clock for Stopped {
    fn unix_seconds(&self) -> i64 {
        1_700_000_000
    }
}

/// An egress that keeps what it was given instead of sending it, with
/// the delivery the real send path would have chosen.
#[derive(Debug, Default)]
struct Recorder {
    sent: Mutex<Vec<Sent>>,
}

impl Recorder {
    /// Everything recorded, oldest first.
    fn taken(&self) -> Result<Vec<Sent>, String> {
        let mut held = self.sent.lock().unwrap_or_else(PoisonError::into_inner);

        Ok(std::mem::take(&mut held))
    }
}

impl Egress for Recorder {
    fn send(&self, reply: &Reply<'_>) -> std::io::Result<()> {
        let delivery = match datapath::delivery(reply)? {
            datapath::Delivery::Broadcast => Delivery::Broadcast,
            datapath::Delivery::Unicast { address } => Delivery::Unicast { address },
            datapath::Delivery::BelowIp {
                address,
                hardware_address,
            } => Delivery::BelowIp {
                address,
                hardware_address: hardware_address.to_vec(),
            },
        };

        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Sent {
                datagram: reply.datagram().to_vec(),
                source: reply.server_address(),
                delivery,
            });

        Ok(())
    }
}

/// A directory holding one case's store, removed when the case ends.
struct Directory {
    path: PathBuf,
}

impl Directory {
    fn new(label: &str) -> Result<Self, String> {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "teneral-differential-{}-{}-{}",
            label.replace(['/', '.'], "-"),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path)
            .map_err(|why| format!("cannot make a directory for a store: {why}"))?;

        Ok(Directory { path })
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        // The directory rather than the file: the write-ahead log
        // leaves a `-wal` and a `-shm` beside the database. A failure
        // to clean up is not the case's verdict.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// What teneral answers the captured request with.
pub fn answer(configuration: &Configuration, request_frame: &[u8]) -> Result<Answered, String> {
    let request = frame::read(request_frame)
        .map_err(|why| format!("the captured request {why}"))?
        .datagram;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|why| format!("cannot build a runtime for the subject: {why}"))?;

    runtime.block_on(decide(configuration, &request))
}

/// Build the store the case describes, and ask the front-end.
async fn decide(configuration: &Configuration, datagram: &[u8]) -> Result<Answered, String> {
    let directory = Directory::new(&configuration.name)?;
    let store = Arc::new(
        SqliteStore::with_clock(directory.path.join("store.sqlite"), Arc::new(Stopped) as _)
            .await
            .map_err(|why| format!("cannot open a store for the case: {why}"))?,
    );
    configure(store.as_ref(), configuration).await?;

    let decider = Decider::new(
        Arc::clone(&store) as Arc<dyn BindingStore>,
        Recorder::default(),
    );
    let server = ServerFacts::new(configuration.server_address, configuration.prefix_length)
        .map_err(|why| format!("the case's server address and prefix are not usable: {why}"))?;
    let arrival = Arrival {
        namespace: UNNAMESPACED,
        ifname: &configuration.interface,
        server,
    };

    // The parse is part of the subject rather than ahead of it. A
    // refusal here is one of teneral's answers -- the front-end
    // declines four kinds of request the reference implementation
    // considers -- so it has to reach the comparison as a silence with
    // its reason, not as a failure of the rig.
    let parsed = match wire::parse(datagram) {
        Ok(parsed) => parsed,
        Err(why) => return Ok(Answered::Silent(format!("the datagram was refused: {why}"))),
    };

    let decided = decider
        .answer(&arrival, &parsed)
        .await
        .map_err(|why| format!("the front-end could not answer the captured request: {why}"))?;

    let mut sent = decider.egress().taken()?;
    if sent.len() > 1 {
        return Err(format!(
            "the front-end sent {} replies to one request, and a case compares one",
            sent.len()
        ));
    }
    match sent.pop() {
        Some(one) => Ok(Answered::Sent(one)),
        None => Ok(Answered::Silent(format!("{:?}", decided.outcome))),
    }
}

/// Write the case's configuration into the store, as an operator
/// would.
async fn configure(store: &SqliteStore, configuration: &Configuration) -> Result<(), String> {
    let network = configuration.name.as_str();
    store
        .create_network(&OPERATOR, network)
        .await
        .map_err(|why| format!("cannot create the network {network}: {why}"))?;

    // The lease time is set rather than assumed, even where the case
    // records the schema's own default: a case says what the capture
    // was taken against, and a default that moved would otherwise
    // silently change what every case asserts.
    let read = store
        .read()
        .await
        .map_err(|why| format!("cannot read the store back: {why}"))?;
    let made = read
        .network(network)
        .await
        .map_err(|why| format!("cannot read the network {network}: {why}"))?
        .ok_or_else(|| format!("the network {network} was not there after being created"))?;
    drop(read);
    store
        .update_network(
            &OPERATOR,
            network,
            &made.cursor,
            configuration.lease_seconds,
        )
        .await
        .map_err(|why| format!("cannot set the lease time of {network}: {why}"))?;

    store
        .attach_interface(&OPERATOR, network, UNNAMESPACED, &configuration.interface)
        .await
        .map_err(|why| {
            format!(
                "cannot attach {} to {network}: {why}",
                configuration.interface
            )
        })?;

    for range in &configuration.ranges {
        store
            .create_address_range(&OPERATOR, network, &range.name, range.first, range.last)
            .await
            .map_err(|why| format!("cannot create the range {}: {why}", range.name))?;
    }

    for held in &configuration.leases {
        let identity = identity(&held.mac)?;
        store
            .acquire_lease(
                &OPERATOR,
                network,
                &identity,
                held.address,
                configuration.lease_seconds,
            )
            .await
            .map_err(|why| {
                format!(
                    "cannot record {} as held by {} in {network}: {why}",
                    held.address, held.mac
                )
            })?;
    }

    Ok(())
}

/// A client identified by the hardware address an operator would
/// type.
fn identity(mac: &str) -> Result<ClientIdentity, String> {
    let selector = Selector::new(SelectorKind::Mac, mac)
        .map_err(|why| format!("{mac} is not a hardware address an operator could type: {why}"))?;

    ClientIdentity::new()
        .with(selector.kind(), selector.value())
        .map_err(|why| format!("{mac} is not an identity the store will hold: {why}"))
}
