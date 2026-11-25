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

    /// Ask what an identity on an interface would be answered with,
    /// and what was considered on the way.
    Explain {
        #[command(flatten)]
        connection: ClientArgs,
        /// The namespace the interface is in.
        namespace: String,
        /// The interface's name.
        ifname: String,
        /// One thing the client presents, as KIND=VALUE where KIND is
        /// client-id, duid-iaid, mac, user-class or vendor-class.
        /// Repeat it for each.
        #[arg(long = "present", value_name = "KIND=VALUE", required = true)]
        presents: Vec<String>,
    },

    /// Show the daemon's own view of itself.
    Diagnostics {
        #[command(flatten)]
        connection: ClientArgs,
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
