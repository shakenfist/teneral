//! The daemon.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use teneral_api::{Listener, ServerConfig};
use teneral_store::{BindingStore, Retention, SqliteStore, SystemClock, TokenStore};
use tokio::runtime::Runtime;
use tokio::signal::unix::{SignalKind, signal};

use crate::cli::ServeArgs;

type Failure = Box<dyn std::error::Error>;

/// How long a stopping daemon waits for open connections to finish
/// what they are sending before it gives up on them. Long enough for a
/// final event to cross a local socket, short enough that a peer that
/// never reads cannot hold a restart hostage.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// Standard error, one line a message. See `main` for why there is no
/// logging framework behind it.
fn log(message: &str) {
    eprintln!("teneral: {message}");
}

pub fn run(runtime: Runtime, args: &ServeArgs) -> Result<(), Failure> {
    // Everything that can be refused is refused before anything is
    // created, so that a refusal leaves nothing behind.
    let retention = Retention::new(args.history_events, args.history_age)?;
    check_store_path(&args.store)?;

    let mut config = ServerConfig::new(&args.socket);
    config.max_live_views = to_usize("--max-live-views", args.max_live_views)?;
    config.max_connections = to_usize("--max-connections", args.max_connections)?;
    config.heartbeat = Duration::from_secs(args.heartbeat_seconds);

    let store = runtime.block_on(serve(config, &args.store, retention))?;

    // The order is the point, and it is why this is not left to the
    // end of scope. The listener has stopped accepting and every open
    // stream has been told this daemon is stopping, and `serve` has
    // waited for the connections to finish with the store. Only now is
    // the runtime dropped, which cancels whatever did not finish in
    // time and so releases the views those connections held. The store
    // goes last, because dropping it stops its writer thread and joins
    // it: a store dropped while a connection still held a view would
    // be a daemon blocked on a thread that something was still using.
    runtime.shutdown_timeout(STOP_GRACE);
    drop(store);

    log("stopped");
    Ok(())
}

/// Bind, open, announce and serve until asked to stop. Hands the store
/// back rather than dropping it, so that the caller decides when its
/// writer thread is joined.
async fn serve(
    config: ServerConfig,
    store_path: &Path,
    retention: Retention,
) -> Result<Arc<SqliteStore>, Failure> {
    // The socket before the store. Binding is what finds out that a
    // live daemon already answers on this path, and that has to come
    // first: opening the same database file under a running daemon
    // would be a second writer on it before anything had refused.
    // A failure after this drops the listener, which removes the
    // socket file again.
    let listener = Listener::bind(config).await?;

    let store = Arc::new(
        SqliteStore::with_clock_and_retention(store_path, Arc::new(SystemClock), retention)
            .await
            .map_err(|error| format!("opening the store at {}: {error}", store_path.display()))?,
    );

    announce(&store, store_path, listener.path()).await?;

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let stop = async move {
        tokio::select! {
            _ = interrupt.recv() => log("stopping on SIGINT"),
            _ = terminate.recv() => log("stopping on SIGTERM"),
        }
    };

    let bindings: Arc<dyn BindingStore> = Arc::clone(&store) as Arc<dyn BindingStore>;
    let tokens: Arc<dyn TokenStore> = Arc::clone(&store) as Arc<dyn TokenStore>;
    listener.serve(bindings, tokens, stop).await?;

    // `serve` has stopped accepting and told every stream, but the
    // connection tasks send their final event on their own time, and
    // each holds the store until it has. Wait for them rather than
    // racing them, with a bound for the peer that never reads.
    let deadline = tokio::time::Instant::now() + STOP_GRACE;
    while Arc::strong_count(&store) > 1 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    Ok(store)
}

/// Say what this daemon is serving.
///
/// The epoch, who minted it and when, the schema version and the
/// latest position are how an operator tells a store that has been
/// minted since a restore from one that has not. The daemon does not
/// try to detect a restored file itself, because any detector needs
/// state outside the database and a restore rewinds that too; this
/// line is the cheap thing that does survive, as long as somebody
/// reads it.
async fn announce(store: &SqliteStore, store_path: &Path, socket: &Path) -> Result<(), Failure> {
    let identity = store
        .identity()
        .await
        .map_err(|error| format!("reading the store's identity: {error}"))?;
    let latest = store.subscribe().borrow().clone();

    let minted = match identity.minted_at {
        Some(at) => format!(
            "minted by {} at {at} (seconds since the Unix epoch)",
            identity.minted_by
        ),
        None => format!(
            "never minted, as created with the database ({})",
            identity.minted_by
        ),
    };

    log(&format!(
        "store {}: epoch {}, {minted}, schema version {}, latest position {latest}",
        store_path.display(),
        identity.epoch.as_str(),
        store.schema_version(),
    ));
    log(&format!("serving on {}", socket.display()));

    Ok(())
}

/// Refuse a store path that cannot be what the operator meant.
///
/// SQLite creates a database file that is not there, which is right
/// for a first start and wrong for a typo: the daemon would come up
/// serving an empty database at the mistaken path, with a fresh epoch
/// that every consumer would then be told to resync to. It does not
/// create directories, so insisting that the parent exists is what
/// turns a wrong path into a refusal. Anything else that stops the
/// file being opened is reported by the open itself.
fn check_store_path(path: &Path) -> Result<(), Failure> {
    if path.is_dir() {
        return Err(format!("the store path {} is a directory", path.display()).into());
    }

    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };

    if !parent.is_dir() {
        return Err(format!(
            "the store path {} is in a directory that does not exist; create it rather than \
             having the daemon guess",
            path.display()
        )
        .into());
    }

    Ok(())
}

fn to_usize(flag: &str, value: u64) -> Result<usize, Failure> {
    usize::try_from(value)
        .map_err(|_| format!("{flag} {value} does not fit in this platform's usize").into())
}
