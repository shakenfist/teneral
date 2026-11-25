//! The one listener.
//!
//! # One socket, and who may connect to it
//!
//! There is a single Unix socket at mode 0660. The mode is what
//! decides who may connect at all, and the group is the deployment's
//! lever: setting group ownership needs a privilege the daemon should
//! not spend, so it belongs to whatever starts the daemon rather than
//! to the daemon.
//!
//! A credential is required, verified once at the handshake, with
//! exactly one exception. A peer whose uid is the one the daemon runs
//! as may act without one, as the owning peer, read from the kernel
//! rather than claimed. That exception is not a convenience: the first
//! credential has to be minted by somebody, and the only caller that
//! can be trusted without one is the caller the socket's permissions
//! already trust. Remove it and the first credential has to come from
//! a secret printed at startup or written to disk, which is the one
//! thing this design rules out -- there is no configuration file here,
//! so there is nowhere for a secret to live and nothing to read it
//! from.
//!
//! The operator's own operations are gated by a capability on a
//! credential rather than by a second socket, which means the
//! strongest boundary in the design is a comparison in this process
//! rather than a permission the kernel enforces. What holds it is in
//! [`crate::authz`].
//!
//! # Refusing to start
//!
//! Three ways, and each is a refusal rather than a recovery.
//!
//! Something already answering on the socket path stops this daemon:
//! replacing a live daemon's socket would leave it holding the store
//! and answering nobody, which is worse than not starting. A socket
//! file with nothing behind it is removed and recreated, because that
//! is what a daemon that was killed leaves and inheriting it is not
//! possible anyway.
//!
//! And a socket whose mode cannot be set is a socket with an unknown
//! permission on it, which is no basis for serving anything. The mode
//! is read back rather than assumed, because a file system is allowed
//! to decline.

use std::future::Future;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use teneral_store::{BindingStore, Cursor, Token, TokenStore};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;

use crate::authz::Authenticated;
use crate::error::{Error, Result};
use crate::proto::{self, Hello, HelloResult, Welcome, hello_result};
use crate::refusal;
use crate::{PROTOCOL_VERSION, wire};

/// The mode the socket is created with.
///
/// The owner and the group may connect and nobody else. The group is
/// how a deployment lets a consumer in, and it is set by whatever owns
/// the directory the socket lives in rather than by this process.
pub const SOCKET_MODE: u32 = 0o660;

/// The uid this process runs as, which is the owning peer's whole
/// identity.
///
/// A connection is the owning peer when the uid the kernel reports for
/// it equals this. Both halves come from the kernel: this one from the
/// process, and the peer's from the socket, so neither is a claim a
/// client makes.
pub fn owning_uid() -> u32 {
    // SAFETY: `getuid` takes no arguments, reads no memory this
    // program owns, and has no error return -- POSIX requires it to
    // succeed. There is nothing here for a wrapper crate to make
    // safer.
    unsafe { libc::getuid() }
}

/// What the listener needs to know.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// Where the socket goes.
    pub socket_path: PathBuf,

    /// The uid the owning peer connects as. A field rather than a call
    /// to [`owning_uid`] at each comparison, so that a test can serve
    /// as a uid it is not running as -- which is the only way to check
    /// that a caller which is *not* the owning peer is refused.
    pub owning_uid: u32,

    /// How many connections may be open at once. A connection is a
    /// task, so this is the bound that stops an accept loop becoming
    /// unbounded memory.
    pub max_connections: usize,

    /// How long to wait for an answer when deciding whether something
    /// else is already serving the socket path.
    ///
    /// Short, because the thing being waited for is a local process
    /// answering a handshake, and a daemon that hangs at startup is
    /// worse than one that refuses: a path that accepts a connection
    /// and does not answer is treated as in use, so the timeout
    /// expiring is not a licence to take it over.
    pub probe_timeout: Duration,
}

impl ServerConfig {
    /// A configuration for a socket at `socket_path`, with the
    /// defaults.
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
            owning_uid: owning_uid(),
            max_connections: 256,
            probe_timeout: Duration::from_secs(2),
        }
    }
}

/// The socket, bound and ready to serve.
///
/// Binding and serving are separate so that a daemon can fail to start
/// before it has a store open and before it has told anybody it is
/// running, and so that a test can hold a bound socket without
/// serving on it.
pub struct Listener {
    /// The listener itself.
    listener: UnixListener,
    /// What it was configured with.
    config: ServerConfig,
}

impl Listener {
    /// Bind the socket, refusing rather than taking over.
    pub async fn bind(config: ServerConfig) -> Result<Self> {
        let path = config.socket_path.clone();

        if path.exists() {
            if crate::client::is_answering(&path, config.probe_timeout).await {
                return Err(Error::InUse { path });
            }

            // Nothing is behind it, so it is the remains of a daemon
            // that was killed. Removed rather than inherited: a bound
            // socket cannot be picked up from a path, only recreated.
            std::fs::remove_file(&path)
                .map_err(|error| Error::io("removing a socket file nothing was behind", error))?;
        }

        let listener = bind_at_mode(&path, SOCKET_MODE)?;

        Ok(Self { listener, config })
    }

    /// Where the socket is.
    pub fn path(&self) -> &Path {
        &self.config.socket_path
    }

    /// Serve until `shutdown` completes.
    ///
    /// Returns when the accept loop stops, which is either because
    /// `shutdown` completed or because accepting failed. A connection
    /// already accepted is a task of its own and ends when its peer
    /// goes or when the runtime does.
    pub async fn serve<S>(
        self,
        store: Arc<dyn BindingStore>,
        tokens: Arc<dyn TokenStore>,
        shutdown: S,
    ) -> Result<()>
    where
        S: Future<Output = ()>,
    {
        let connections = Arc::new(Semaphore::new(self.config.max_connections));
        let owning_uid = self.config.owning_uid;
        let mut shutdown = std::pin::pin!(shutdown);

        loop {
            // The permit before the accept, so that a daemon at its
            // bound stops taking connections off the queue rather than
            // accepting them and then holding them open with nothing
            // serving them.
            let permit = tokio::select! {
                biased;
                () = &mut shutdown => break,
                permit = Arc::clone(&connections).acquire_owned() => match permit {
                    Ok(permit) => permit,
                    // The semaphore is only closed by this function,
                    // which does not, so this is unreachable rather
                    // than handled -- and stopping is the only honest
                    // thing to do with a bound that has gone away.
                    Err(_) => break,
                },
            };

            let accepted = tokio::select! {
                biased;
                () = &mut shutdown => break,
                accepted = self.listener.accept() => accepted,
            };

            let (stream, _) =
                accepted.map_err(|error| Error::io("accepting a connection", error))?;

            let store = Arc::clone(&store);
            let tokens = Arc::clone(&tokens);
            tokio::spawn(async move {
                // The permit is moved into the task and dropped with
                // it, so the bound counts connections that are open
                // rather than connections that were accepted.
                let _permit = permit;

                serve_connection(stream, store, tokens, owning_uid).await;
            });
        }

        Ok(())
    }
}

impl Drop for Listener {
    /// Remove the socket file.
    ///
    /// The other half of the rule that a socket file with nothing
    /// behind it is removed rather than inherited: a daemon that shuts
    /// down cleanly should not leave the next one a file to probe. A
    /// failure is not reported, because by this point there is nobody
    /// left to report it to and the next start recovers anyway.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.config.socket_path);
    }
}

/// Bind a socket at `path` with exactly `mode`.
///
/// Bound at a neighbouring name and renamed into place, which is not
/// fussiness. A socket created at the final path exists, briefly, with
/// whatever mode the process umask produced, and "briefly" is enough
/// for a connection on a machine where somebody is waiting for it. The
/// rename is atomic, so the path either does not exist or carries a
/// socket whose mode was set before anything could reach it.
fn bind_at_mode(path: &Path, mode: u32) -> Result<UnixListener> {
    let staged = staging_path(path)?;

    // A leftover from a start that failed between binding and
    // renaming. It is this process's own name, so removing it takes
    // nothing from anybody.
    let _ = std::fs::remove_file(&staged);

    let listener = UnixListener::bind(&staged)
        .map_err(|error| Error::io(format!("binding {}", staged.display()), error))?;

    let outcome = set_mode(&staged, mode).and_then(|()| {
        std::fs::rename(&staged, path).map_err(|error| {
            Error::io(
                format!("moving the socket into place at {}", path.display()),
                error,
            )
        })
    });

    if let Err(error) = outcome {
        // Either the mode did not take or the move did, so the socket
        // this function bound is not one anything should be able to
        // find.
        drop(listener);
        let _ = std::fs::remove_file(&staged);
        return Err(error);
    }

    Ok(listener)
}

/// Where the socket is bound before it is moved into place.
///
/// Beside the final path, because a rename is only atomic within one
/// directory, and named after this process so that two daemons racing
/// to start cannot stage over each other.
fn staging_path(path: &Path) -> Result<PathBuf> {
    let directory = path.parent().ok_or_else(|| {
        Error::io(
            format!("creating a socket at {}", path.display()),
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the path names no directory to create it in",
            ),
        )
    })?;

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            Error::io(
                format!("creating a socket at {}", path.display()),
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the path does not end in a name a socket can be created under",
                ),
            )
        })?;

    Ok(directory.join(format!(".{name}.{}.staging", std::process::id())))
}

/// Set `path`'s mode, and check that it took.
///
/// Read back rather than assumed: a file system is allowed to decline
/// a mode, and serving on a socket whose permission is not the one
/// asked for is serving on an unknown permission.
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|error| Error::io(format!("setting the mode of {}", path.display()), error))?;

    let found = std::fs::metadata(path)
        .map_err(|error| {
            Error::io(
                format!("reading back the mode of {}", path.display()),
                error,
            )
        })?
        .permissions()
        .mode();

    // The file type bits come back with the mode, so only the
    // permission bits are compared.
    let found = found & 0o7777;
    if found == mode {
        Ok(())
    } else {
        Err(Error::Mode {
            path: path.to_owned(),
            wanted: mode,
            found,
        })
    }
}

/// Serve one connection, from its handshake to its end.
///
/// Nothing here is reported upwards. A connection that fails is one
/// client's problem and the daemon goes on serving every other
/// network; what a failure costs is a line in a log, which is the
/// binary's to arrange.
async fn serve_connection(
    mut stream: UnixStream,
    store: Arc<dyn BindingStore>,
    tokens: Arc<dyn TokenStore>,
    owning_uid: u32,
) {
    let peer_uid = match stream.peer_cred() {
        Ok(credential) => credential.uid(),
        // The kernel declines to say who connected, so there is no
        // identity to serve and nothing to say to a peer that may not
        // exist any more.
        Err(_) => return,
    };

    let caller = match handshake(&mut stream, &*store, &*tokens, owning_uid, peer_uid).await {
        Ok(Some(caller)) => caller,
        // Refused, and the refusal has been sent. The connection ends
        // here deliberately: a caller whose credential was refused has
        // nothing to retry on the same connection.
        Ok(None) => return,
        Err(_) => return,
    };

    serve_requests(&mut stream, &caller).await;
}

/// Settle who the connection is acting as, or refuse it.
///
/// `Ok(None)` means the connection was refused and told why.
async fn handshake(
    stream: &mut UnixStream,
    store: &dyn BindingStore,
    tokens: &dyn TokenStore,
    owning_uid: u32,
    peer_uid: u32,
) -> Result<Option<Authenticated>> {
    let hello: Hello = match wire::read_message(stream).await {
        Ok(hello) => hello,
        Err(Error::TooLarge { length }) => {
            refuse_hello(stream, refusal::message_too_large(length)).await?;
            return Ok(None);
        }
        Err(error) => return Err(error),
    };

    if hello.protocol_version != PROTOCOL_VERSION {
        refuse_hello(stream, refusal::version_mismatch(hello.protocol_version)).await?;
        return Ok(None);
    }

    let caller = match hello.token_secret.as_deref() {
        // A credential is verified whoever presents it: the owning
        // peer's exception is for omitting one, not for being
        // identified without one.
        Some(secret) => match tokens.authenticate(secret).await {
            Ok(Some(token)) => Authenticated::Token(Box::new(token)),
            Ok(None) => {
                refuse_hello(
                    stream,
                    refusal::unauthenticated(
                        "the credential presented is not one this daemon holds",
                    ),
                )
                .await?;
                return Ok(None);
            }
            Err(error) => {
                refuse_hello(stream, refusal::for_store_error(&error)).await?;
                return Ok(None);
            }
        },

        None if peer_uid == owning_uid => Authenticated::OwningPeer(peer_uid),

        None => {
            refuse_hello(
                stream,
                refusal::unauthenticated(
                    "this connection presented no credential and is not the peer the daemon runs \
                     as",
                ),
            )
            .await?;
            return Ok(None);
        }
    };

    let welcome = welcome_for(store, &caller);
    wire::write_message(
        stream,
        &HelloResult {
            outcome: Some(hello_result::Outcome::Welcome(welcome)),
        },
    )
    .await?;

    Ok(Some(caller))
}

/// What the daemon tells a caller about itself and about the caller.
fn welcome_for(store: &dyn BindingStore, caller: &Authenticated) -> Welcome {
    // The position through the channel that announces movement rather
    // than through a view of the store: a view is a thread holding a
    // read transaction, and the current position is the one thing that
    // channel always holds.
    let positions = store.subscribe();
    let position = Cursor::clone(&positions.borrow());

    Welcome {
        protocol_version: PROTOCOL_VERSION,
        position: Some(proto::cursor(&position)),
        schema_version: store.schema_version(),
        scope: Some(proto::scope(&caller.scope())),
        capabilities: caller
            .capabilities()
            .into_iter()
            .map(|capability| proto::capability(capability) as i32)
            .collect(),
        identified_as: Some(identification(caller)),
    }
}

/// How the caller was identified, which is a different question from
/// what it may do.
fn identification(caller: &Authenticated) -> proto::Identification {
    let identity = match caller {
        Authenticated::OwningPeer(uid) => {
            proto::identification::Identity::OwningPeer(proto::OwningPeer { uid: *uid })
        }
        Authenticated::Token(token) => {
            proto::identification::Identity::Token(token_identity(token))
        }
    };

    proto::Identification {
        identity: Some(identity),
    }
}

/// A credential as the handshake names it, without its secret.
fn token_identity(token: &Token) -> proto::TokenIdentity {
    proto::TokenIdentity {
        id: token.id.as_str().to_owned(),
        name: token.name.clone(),
    }
}

/// Answer a Hello with a refusal.
async fn refuse_hello(stream: &mut UnixStream, refusal: proto::Refusal) -> Result<()> {
    wire::write_message(
        stream,
        &HelloResult {
            outcome: Some(hello_result::Outcome::Refusal(refusal)),
        },
    )
    .await
}

/// Read requests and answer them until the connection ends.
///
/// No resource operation is served on this socket yet. What is in
/// place is the part every operation will go through: the framing, the
/// bound on a frame, and the authorisation decision -- so a request
/// without the capability its operation needs is refused here, and a
/// request this build has no answer for is refused as an operation
/// this daemon does not serve rather than left unanswered.
async fn serve_requests(stream: &mut UnixStream, caller: &Authenticated) {
    loop {
        let request: proto::Request = match wire::read_message(stream).await {
            Ok(request) => request,
            Err(Error::TooLarge { length }) => {
                // The body was never read, so the stream has nothing
                // left to resynchronise on: the refusal goes out and
                // the connection closes.
                let _ = refuse_request(stream, refusal::message_too_large(length)).await;
                return;
            }
            Err(_) => return,
        };

        let refusal = match request.kind.as_ref() {
            Some(kind) => match caller.require_capability(kind) {
                Ok(()) => refusal::unknown_operation(),
                Err(refusal) => *refusal,
            },
            None => refusal::unknown_operation(),
        };

        if refuse_request(stream, refusal).await.is_err() {
            return;
        }
    }
}

/// Answer a request with a refusal.
async fn refuse_request(stream: &mut UnixStream, refusal: proto::Refusal) -> Result<()> {
    wire::write_message(
        stream,
        &proto::Response {
            // A refusal is not a position in anything, and it names no
            // network: a caller refused for a network it may not reach
            // must not be told the network exists.
            position: None,
            network: None,
            answer: Some(proto::response::Answer::Refusal(refusal)),
        },
    )
    .await
}
