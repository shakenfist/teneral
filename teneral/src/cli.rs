//! The command line.
//!
//! There is no configuration file, and there will not be one: the
//! daemon is configured by these flags at start and by its API
//! afterwards. Every number it has is a flag with a default, and
//! `--help` is the reference for them.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Where the one piece of durable state lives.
pub const DEFAULT_STORE: &str = "/var/lib/teneral/store.sqlite";

/// Where the one listener goes. The directory is the deployment's: a
/// unit with a runtime directory and a group is what makes it
/// reachable by the right people.
pub const DEFAULT_SOCKET: &str = "/run/teneral/teneral.sock";

#[derive(Debug, Parser)]
#[command(
    version,
    about = "A DHCP and DNS daemon for many networks in one process"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the daemon.
    Serve(ServeArgs),

    #[command(flatten)]
    Client(ClientCommand),
}

/// The daemon's flags.
#[derive(Debug, Args)]
pub struct ServeArgs {
    /// The database file, which is the only durable state. It is
    /// opened and migrated if it is there and created if it is not,
    /// but its directory is never created: a daemon pointed at a
    /// directory that is not there has been given the wrong path.
    #[arg(long, value_name = "PATH", default_value = DEFAULT_STORE)]
    pub store: PathBuf,

    /// The control socket, created at mode 0660. Its group is the
    /// deployment's to set; admin is a capability on a token rather
    /// than a second socket.
    #[arg(long, value_name = "PATH", default_value = DEFAULT_SOCKET)]
    pub socket: PathBuf,

    /// The most events of history kept. This is the operative bound: a
    /// consumer further behind than this resyncs rather than resumes.
    #[arg(long, value_name = "EVENTS", default_value_t = 50_000,
          value_parser = clap::value_parser!(u64).range(1..))]
    pub history_events: u64,

    /// The oldest an event of history may be, in seconds. The count
    /// above is the operative bound; this one is a ceiling on the log
    /// of a store where little happens. Six hours is tighter than the
    /// library's own default of a week, which keeps that log small and
    /// means a quiet deployment reaches the resync path at all rather
    /// than leaving it exercised only by the test suite. Evicting
    /// history a consumer could have resumed from is a cost, not a
    /// benefit: raise this where consumers are away for hours as a
    /// matter of course.
    #[arg(long, value_name = "SECONDS", default_value_t = 21_600,
          value_parser = clap::value_parser!(u64).range(1..))]
    pub history_age: u64,

    /// The most views of the store held at once. Each costs a thread
    /// for as long as it is held.
    #[arg(long, value_name = "N", default_value_t = 64,
          value_parser = clap::value_parser!(u64).range(1..))]
    pub max_live_views: u64,

    /// The most connections open at once. A connection is a task, so
    /// this is what stops an accept loop becoming unbounded memory.
    #[arg(long, value_name = "N", default_value_t = 256,
          value_parser = clap::value_parser!(u64).range(1..))]
    pub max_connections: u64,

    /// How often an open stream is sent a heartbeat, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 15,
          value_parser = clap::value_parser!(u64).range(1..))]
    pub heartbeat_seconds: u64,
}

/// What every client subcommand shares.
#[derive(Debug, Args)]
pub struct ClientArgs {
    /// The daemon's control socket.
    #[arg(long, value_name = "PATH", default_value = DEFAULT_SOCKET)]
    pub socket: PathBuf,

    /// Read the token's secret from the first line of standard input.
    /// Without this the secret is read from the TENERAL_TOKEN
    /// environment variable, and with neither the connection is made
    /// as the owning peer, which needs no token. There is no flag
    /// that takes the secret itself, because an argument is visible
    /// to every user on the machine in the process table.
    #[arg(long)]
    pub token_stdin: bool,
}

/// The subcommands that talk to a running daemon.
#[derive(Debug, Subcommand)]
pub enum ClientCommand {
    /// Read one resource.
    Get {
        #[command(flatten)]
        connection: ClientArgs,
        #[command(subcommand)]
        resource: Resource,
    },

    /// Read every resource of one kind that is in reach.
    List {
        #[command(flatten)]
        connection: ClientArgs,
        #[command(subcommand)]
        collection: Collection,
    },

    /// Ask a question whose answer the asking does not name: which
    /// network has claimed an interface, or what a client presenting
    /// an identity is matched by and holds.
    Lookup {
        #[command(flatten)]
        connection: ClientArgs,
        #[command(subcommand)]
        query: Query,
    },

    /// Ask what an identity on an interface would be answered with,
    /// and what was considered on the way.
    Explain {
        #[command(flatten)]
        connection: ClientArgs,
        /// The namespace the interface is in.
        namespace: String,
        /// The interface's name.
        ifname: String,
        #[command(flatten)]
        identity: PresentsArgs,
    },

    /// Print a network's digest, which changes when its configuration
    /// changes. It is opaque: compare two for equality and nothing
    /// else.
    Digest {
        #[command(flatten)]
        connection: ClientArgs,
        network: String,
    },

    /// Show the daemon's own view of itself.
    Diagnostics {
        #[command(flatten)]
        connection: ClientArgs,
    },

    /// Create a network, which is what every other resource is keyed
    /// by. It starts with the schema's default lease time.
    CreateNetwork {
        #[command(flatten)]
        connection: ClientArgs,
        name: String,
    },

    /// Change how long a network holds the addresses it gives out.
    UpdateNetwork {
        #[command(flatten)]
        connection: ClientArgs,
        name: String,
        /// The whole new lease time in seconds, rather than a change
        /// to the old one.
        #[arg(value_name = "SECONDS")]
        lease_seconds: i64,
        #[command(flatten)]
        position: ExpectedArgs,
    },

    /// Delete a network.
    DeleteNetwork {
        #[command(flatten)]
        connection: ClientArgs,
        name: String,
        #[command(flatten)]
        position: ExpectedArgs,
    },

    /// Claim an interface for a network, so that a request arriving on
    /// it is answered from that network.
    AttachInterface {
        #[command(flatten)]
        connection: ClientArgs,
        network: String,
        /// The namespace the interface is in. An interface in no
        /// namespace at all is attached under `root`.
        namespace: String,
        ifname: String,
    },

    /// Give an interface back, so that nothing answers on it.
    DetachInterface {
        #[command(flatten)]
        connection: ClientArgs,
        network: String,
        namespace: String,
        ifname: String,
        #[command(flatten)]
        position: ExpectedArgs,
    },

    /// Configure a span of addresses a network may give out.
    CreateAddressRange {
        #[command(flatten)]
        connection: ClientArgs,
        network: String,
        name: String,
        /// The lowest address in the span, which is in it.
        first: String,
        /// The highest address in the span, which is also in it: both
        /// ends are inclusive, as an operator writes a pool.
        last: String,
    },

    /// Move a range's ends, both of them, to where they should now be.
    UpdateAddressRange {
        #[command(flatten)]
        connection: ClientArgs,
        network: String,
        name: String,
        /// The lowest address in the span, which is in it.
        first: String,
        /// The highest address in the span, which is also in it.
        last: String,
        #[command(flatten)]
        position: ExpectedArgs,
    },

    /// Delete a range, which leaves the leases taken from it alone.
    DeleteAddressRange {
        #[command(flatten)]
        connection: ClientArgs,
        network: String,
        name: String,
        #[command(flatten)]
        position: ExpectedArgs,
    },

    /// Configure a network's answer for one client.
    CreateBinding {
        #[command(flatten)]
        connection: ClientArgs,
        network: String,
        /// client-id, duid-iaid, mac, user-class or vendor-class.
        kind: String,
        value: String,
        address: String,
        #[command(flatten)]
        naming: HostnameArgs,
        /// Take the lease holding this address away from its holder as
        /// part of writing the binding, if that lease still carries
        /// this id. It is a lease's `lease_id` from `get lease` and
        /// not its position: the id changes when and only when the
        /// address changes holder, so a renewal in between does not
        /// invalidate the instruction and a reassignment does. Without
        /// it a lease in the way is reported rather than taken.
        #[arg(long, value_name = "EPOCH:SEQUENCE")]
        revoke_lease: Option<String>,
    },

    /// Change the address or the name a binding answers with. Its
    /// selector cannot change, that being the binding's identity.
    UpdateBinding {
        #[command(flatten)]
        connection: ClientArgs,
        network: String,
        /// client-id, duid-iaid, mac, user-class or vendor-class.
        kind: String,
        value: String,
        address: String,
        #[command(flatten)]
        naming: HostnameArgs,
        #[command(flatten)]
        position: ExpectedArgs,
    },

    /// Delete a binding, which leaves any lease on its address alone.
    DeleteBinding {
        #[command(flatten)]
        connection: ClientArgs,
        network: String,
        /// client-id, duid-iaid, mac, user-class or vendor-class.
        kind: String,
        value: String,
        #[command(flatten)]
        position: ExpectedArgs,
    },

    /// Take a lease away from the client holding it. It is the only
    /// write to a lease there is: a lease is made by the daemon
    /// answering a client, so one cannot be invented here.
    RevokeLease {
        #[command(flatten)]
        connection: ClientArgs,
        network: String,
        address: String,
        #[command(flatten)]
        position: ExpectedArgs,
    },

    /// Mint a token and print its secret, once. Needs the admin
    /// capability or the owning peer, and cannot mint more than the
    /// minter holds.
    MintToken {
        #[command(flatten)]
        connection: ClientArgs,
        /// A name for the token, for a person to recognise it by.
        name: String,
        #[command(flatten)]
        reach: ReachArgs,
        /// A capability the token holds. Repeat it for each.
        #[arg(long = "capability", value_enum, required = true)]
        capabilities: Vec<CapabilityArg>,
    },

    /// Delete a token, which cannot be presented again afterwards.
    /// Needs the admin capability or the owning peer, and reaches only
    /// a token whose scope lies inside the caller's.
    DeleteToken {
        #[command(flatten)]
        connection: ClientArgs,
        /// The token's id, from `list tokens` or from the mint that
        /// made it.
        id: String,
    },

    /// Replace the epoch, so that every consumer holding a position
    /// resyncs. Needs the admin capability or the owning peer.
    MintEpoch {
        #[command(flatten)]
        connection: ClientArgs,
    },

    /// Print events as they arrive, until the daemon ends the stream
    /// or this is interrupted.
    Watch {
        #[command(flatten)]
        connection: ClientArgs,
        #[command(flatten)]
        reach: ReachArgs,
        /// Only these kinds of resource. Repeat it for each; with none,
        /// every kind.
        #[arg(long = "kind", value_enum)]
        kinds: Vec<KindArg>,
        /// Resume after this position, as EPOCH:SEQUENCE, rather than
        /// starting with a snapshot.
        #[arg(long, value_name = "EPOCH:SEQUENCE")]
        after: Option<String>,
    },
}

/// Which networks a token or a watch reaches.
#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
pub struct ReachArgs {
    /// A network it reaches. Repeat it for each.
    #[arg(long = "network", value_name = "NAME")]
    pub networks: Vec<String>,

    /// Every network, including ones created later.
    #[arg(long)]
    pub every_network: bool,
}

/// The position a write expects the resource to already be at.
///
/// Required rather than optional on every command that takes one. A
/// write without it is refused by the daemon, so a flag that could be
/// left off would only move that refusal from the command line to the
/// socket.
#[derive(Debug, Args)]
pub struct ExpectedArgs {
    /// The position the resource is expected to be at, as
    /// EPOCH:SEQUENCE. It is the `cursor` of a `get` or a `list` of
    /// that resource, and the write is refused if the resource has
    /// moved on since -- which is what stops two writers computing a
    /// new value from the same stale read.
    #[arg(long, value_name = "EPOCH:SEQUENCE")]
    pub expected: String,
}

/// What a client presents, for the questions asked about an identity
/// rather than about a key.
#[derive(Debug, Args)]
pub struct PresentsArgs {
    /// One thing the client presents, as KIND=VALUE where KIND is
    /// client-id, duid-iaid, mac, user-class or vendor-class.
    /// Repeat it for each.
    #[arg(long = "present", value_name = "KIND=VALUE", required = true)]
    pub presents: Vec<String>,
}

/// The name a binding contributes to its network's zones.
#[derive(Debug, Args)]
pub struct HostnameArgs {
    /// The name this binding contributes, if the operator gives it
    /// one. Leaving the flag off and passing it empty are different
    /// states of the binding rather than two spellings of one.
    #[arg(long, value_name = "NAME")]
    pub hostname: Option<String>,
}

/// The collections `list` reads.
///
/// Each is a sequence of answers ended by a marker rather than one
/// message holding a list, so what is printed is one resource per
/// answer and then the count the marker carries.
#[derive(Debug, Subcommand)]
pub enum Collection {
    /// Every network in reach. The one listing that names no network,
    /// being filtered to what the caller may see rather than refused.
    Networks,
    /// Every interface a network has claimed.
    Attachments { network: String },
    /// Every range a network may give addresses out of.
    AddressRanges { network: String },
    /// Every binding an operator configured in a network.
    Bindings { network: String },
    /// Every lease in a network, the expired ones included; each says
    /// whether it had expired when it was read.
    Leases { network: String },
    /// Every token whose scope lies inside the caller's. Never carries
    /// a secret.
    Tokens,
}

/// The questions `lookup` asks.
///
/// They are not `get` because the request does not name what comes
/// back: the answer is resolved from an interface or matched against an
/// identity, and which resource it turns out to be is the point of
/// asking.
#[derive(Debug, Subcommand)]
pub enum Query {
    /// Which network has claimed an interface. A caller whose scope
    /// does not reach the answer is refused exactly as it would be for
    /// an interface no network has claimed.
    InterfaceOwner { namespace: String, ifname: String },

    /// Which binding applies to a client presenting this identity. The
    /// answer names the selector that matched, so an operator who
    /// configured one reservation and is told another matched has
    /// found a precedence problem.
    Binding {
        network: String,
        #[command(flatten)]
        identity: PresentsArgs,
    },

    /// The same question asked the way a packet arrives: a client on
    /// an interface, inside the network the caller believes that
    /// interface belongs to. The two can disagree, and a request where
    /// they do is refused and counted.
    BindingOnInterface {
        namespace: String,
        ifname: String,
        network: String,
        #[command(flatten)]
        identity: PresentsArgs,
    },

    /// What this client holds in this network, which is a list: a
    /// client may legitimately hold an address in each of several
    /// networks.
    LeasesHeldBy {
        network: String,
        #[command(flatten)]
        identity: PresentsArgs,
    },
}

/// The resources `get` reads.
#[derive(Debug, Subcommand)]
pub enum Resource {
    /// A network.
    Network { name: String },
    /// An interface's attachment to a network.
    Attachment {
        network: String,
        namespace: String,
        ifname: String,
    },
    /// A named range of addresses.
    AddressRange { network: String, name: String },
    /// The binding for a selector.
    Binding {
        network: String,
        /// client-id, duid-iaid, mac, user-class or vendor-class.
        kind: String,
        value: String,
    },
    /// The lease on an address.
    Lease { network: String, address: String },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CapabilityArg {
    Read,
    Write,
    Watch,
    Admin,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum KindArg {
    Network,
    Attachment,
    AddressRange,
    Binding,
    Lease,
}
