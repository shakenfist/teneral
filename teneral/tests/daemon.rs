//! The daemon as an operator meets it: the real binary, a real socket
//! in a directory the test owns, and the real client.
//!
//! Every test that starts a daemon stops it, whatever the test's
//! outcome, and everything that waits does so with a bound. A daemon
//! left running after a failed assertion would hold its socket and its
//! store for the rest of the machine's uptime.

use std::fs;
use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use teneral_api::client::Connection;
use teneral_api::proto::{self, request, response};

const BINARY: &str = env!("CARGO_BIN_EXE_teneral");

/// The longest any wait here lasts.
const BOUND: Duration = Duration::from_secs(20);

static NEXT: AtomicU32 = AtomicU32::new(0);

/// Held while a descriptor is being created or while a child is being
/// started. A child inherits every descriptor open in the process for
/// the moment between `fork` and `exec`, so a socket one test has
/// just closed can be kept alive by another test's child, and "nothing
/// is listening" is then briefly untrue.
static SPAWNING: Mutex<()> = Mutex::new(());

fn spawning() -> std::sync::MutexGuard<'static, ()> {
    SPAWNING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A directory one test owns, removed when it ends.
struct Directory(PathBuf);

impl Directory {
    fn new(label: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "teneral-daemon-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("a temporary directory for a test");

        Self(path)
    }

    fn socket(&self) -> PathBuf {
        self.0.join("teneral.sock")
    }

    fn store(&self) -> PathBuf {
        self.0.join("store.sqlite")
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A daemon process, killed if the test did not stop it.
struct Daemon {
    child: Child,
    errors: PathBuf,
}

impl Daemon {
    /// Start one, without waiting for it to be ready.
    fn spawn(directory: &Directory, label: &str, store: &Path, socket: &Path) -> Self {
        Self::spawn_with(directory, label, store, socket, &[])
    }

    /// Start one with extra flags, without waiting for it to be ready.
    fn spawn_with(
        directory: &Directory,
        label: &str,
        store: &Path,
        socket: &Path,
        extra: &[&str],
    ) -> Self {
        let errors = directory.0.join(format!("{label}.stderr"));
        let _spawning = spawning();
        let child = Command::new(BINARY)
            .arg("serve")
            .arg("--store")
            .arg(store)
            .arg("--socket")
            .arg(socket)
            .args(extra)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(&errors).expect("a file for the daemon's standard error"))
            .spawn()
            .expect("the daemon binary to start");

        Self { child, errors }
    }

    /// Start one and wait until it answers.
    fn start(directory: &Directory) -> Self {
        let daemon = Self::spawn(directory, "daemon", &directory.store(), &directory.socket());
        daemon.wait_until_serving();
        wait_until_answering(&directory.socket());

        daemon
    }

    /// Wait until it says it is serving, which is after it has bound
    /// the socket and opened the store. The socket accepting
    /// connections is earlier than that, so it is not the signal.
    fn wait_until_serving(&self) {
        let deadline = Instant::now() + BOUND;
        while !self.stderr().contains("serving on") {
            assert!(
                Instant::now() < deadline,
                "the daemon was not serving within {BOUND:?}; it said:\n{}",
                self.stderr()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn stderr(&self) -> String {
        fs::read_to_string(&self.errors).unwrap_or_default()
    }

    /// Wait for it to exit, up to the bound.
    fn exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + BOUND;
        loop {
            if let Some(status) = self.child.try_wait().expect("the daemon to be waitable") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the daemon did not exit within {BOUND:?}; it said:\n{}",
                self.stderr()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Ask it to stop the way systemd does, and wait for it.
    fn terminate(&mut self) -> ExitStatus {
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("kill -TERM {}", self.child.id()))
            .status()
            .expect("sh to run kill");
        assert!(status.success());

        self.exit()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Wait until something accepts a connection on `socket`.
fn wait_until_answering(socket: &Path) {
    let deadline = Instant::now() + BOUND;
    while UnixStream::connect(socket).is_err() {
        assert!(
            Instant::now() < deadline,
            "nothing answered on {} within {BOUND:?}",
            socket.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Run the binary to completion with `arguments`, returning its exit
/// status and everything it printed.
fn run(arguments: &[&str]) -> (ExitStatus, String, String) {
    let spawned = spawning();
    let mut child = Command::new(BINARY)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary to start");
    drop(spawned);

    let mut stdout = child.stdout.take().expect("a piped stdout");
    let mut stderr = child.stderr.take().expect("a piped stderr");
    let out = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let err = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    let deadline = Instant::now() + BOUND;
    let status = loop {
        if let Some(status) = child.try_wait().expect("the binary to be waitable") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{arguments:?} did not finish within {BOUND:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    (
        status,
        out.join().expect("stdout reader"),
        err.join().expect("stderr reader"),
    )
}

fn mac(value: &str) -> proto::Selector {
    proto::Selector {
        kind: proto::SelectorKind::Mac as i32,
        value: value.to_owned(),
    }
}

#[test]
fn refuses_a_store_path_it_cannot_open() {
    let directory = Directory::new("badstore");
    let missing = directory.0.join("not-there");
    let store = missing.join("store.sqlite");
    let socket = directory.socket();

    let mut daemon = Daemon::spawn(&directory, "daemon", &store, &socket);
    let status = daemon.exit();

    assert!(!status.success(), "{}", daemon.stderr());
    assert!(
        daemon.stderr().contains("does not exist"),
        "{}",
        daemon.stderr()
    );
    // Refused rather than created somewhere unintended.
    assert!(!missing.exists());
    assert!(!socket.exists());

    // A path that is a directory, which SQLite would also refuse, is
    // named as what it is.
    let mut daemon = Daemon::spawn(&directory, "directory", &directory.0, &socket);
    assert!(!daemon.exit().success());
    assert!(
        daemon.stderr().contains("is a directory"),
        "{}",
        daemon.stderr()
    );
    assert!(!socket.exists());
}

#[test]
fn a_live_daemon_stops_a_second_starting_on_its_socket() {
    let directory = Directory::new("second");
    let first = Daemon::start(&directory);

    let other_store = directory.0.join("other.sqlite");
    let mut second = Daemon::spawn(&directory, "second", &other_store, &directory.socket());
    let status = second.exit();

    assert!(!status.success());
    assert!(
        second.stderr().contains("already answering"),
        "{}",
        second.stderr()
    );
    // It did not get as far as opening a store, and it did not take
    // the socket over: the first is still serving.
    assert!(!other_store.exists());
    wait_until_answering(&directory.socket());
    drop(first);
}

#[test]
fn a_stale_socket_file_does_not_stop_a_daemon_starting() {
    let directory = Directory::new("stale");

    // What a killed daemon leaves: a socket file nothing is behind.
    drop(UnixListener::bind(directory.socket()).expect("a socket to leave behind"));
    assert!(directory.socket().exists());
    let probe = UnixStream::connect(directory.socket());
    assert!(
        probe.is_err(),
        "connected to a socket nobody listens on: {probe:?}"
    );

    let daemon = Daemon::start(&directory);
    assert!(
        daemon.stderr().contains("serving on"),
        "{}",
        daemon.stderr()
    );
}

#[test]
fn startup_says_which_store_it_is_and_shutdown_is_clean() {
    let directory = Directory::new("startup");
    let mut daemon = Daemon::start(&directory);

    let said = daemon.stderr();
    for wanted in ["epoch", "never minted", "schema version", "latest position"] {
        assert!(said.contains(wanted), "{wanted:?} missing from:\n{said}");
    }

    let status = daemon.terminate();
    assert!(status.success(), "{}", daemon.stderr());
    // The socket file is removed on the way out, so the next start has
    // nothing to probe.
    assert!(!directory.socket().exists());
    assert!(daemon.stderr().contains("stopping on SIGTERM"));
}

#[tokio::test]
async fn two_networks_holding_one_address_each_answer_for_themselves() {
    let directory = Directory::new("roundtrip");
    let daemon = Daemon::start(&directory);
    let socket = directory.socket();

    // The test runs as the daemon's own user, so it is the owning peer
    // and needs no token.
    let mut connection = Connection::connect(&socket, None)
        .await
        .expect("a connection as the owning peer");

    for (network, client) in [
        ("alpha", "00:11:22:33:44:55"),
        ("beta", "66:77:88:99:aa:bb"),
    ] {
        let created = connection
            .request(request::Kind::CreateNetwork(proto::CreateNetworkRequest {
                name: network.to_owned(),
            }))
            .await
            .expect("creating a network");
        assert!(
            !matches!(created.answer, Some(response::Answer::Refusal(_))),
            "{created:?}"
        );

        // The same address in both, for different clients.
        let bound = connection
            .request(request::Kind::CreateBinding(proto::CreateBindingRequest {
                network: network.to_owned(),
                selector: Some(mac(client)),
                address: proto::address("10.0.0.5".parse().expect("an address")),
                hostname: None,
                revoke_lease: None,
            }))
            .await
            .expect("creating a binding");
        assert!(
            !matches!(bound.answer, Some(response::Answer::Refusal(_))),
            "{bound:?}"
        );
    }

    for (network, client) in [
        ("alpha", "00:11:22:33:44:55"),
        ("beta", "66:77:88:99:aa:bb"),
    ] {
        let answer = connection
            .request(request::Kind::GetBinding(proto::GetBindingRequest {
                network: network.to_owned(),
                selector: Some(mac(client)),
            }))
            .await
            .expect("reading a binding");

        assert_eq!(answer.network.as_deref(), Some(network), "{answer:?}");
        match answer.answer {
            Some(response::Answer::Resource(proto::Resource {
                state: Some(proto::resource::State::Binding(binding)),
            })) => {
                assert_eq!(binding.network, network);
                assert_eq!(
                    binding.address,
                    proto::address("10.0.0.5".parse().expect("an address"))
                );
            }
            other => panic!("{network}: {other:?}"),
        }
    }

    // One network's client asked about in the other is not found, and
    // is refused without being answered from the wrong network.
    let crossed = connection
        .request(request::Kind::GetBinding(proto::GetBindingRequest {
            network: "beta".to_owned(),
            selector: Some(mac("00:11:22:33:44:55")),
        }))
        .await
        .expect("reading a binding");
    assert!(
        matches!(crossed.answer, Some(response::Answer::Refusal(_))),
        "{crossed:?}"
    );

    // And through the subcommand an operator would type.
    let socket_text = socket.to_str().expect("a utf-8 path");
    let (status, out, err) = run(&[
        "get",
        "--socket",
        socket_text,
        "binding",
        "alpha",
        "mac",
        "00:11:22:33:44:55",
    ]);
    assert!(status.success(), "{err}");
    assert!(
        out.contains("\"alpha\"") && !out.contains("\"beta\""),
        "{out}"
    );

    let (status, out, err) = run(&["diagnostics", "--socket", socket_text]);
    assert!(status.success(), "{err}");
    assert!(out.contains("schema_version"), "{out}");

    drop(connection);
    drop(daemon);
}

#[test]
fn help_names_every_flag_with_its_default() {
    let (status, help, _) = run(&["serve", "--help"]);
    assert!(status.success());

    // (flag, default), against the table the daemon is specified by.
    let flags = [
        ("--store", "/var/lib/teneral/store.sqlite"),
        ("--socket", "/run/teneral/teneral.sock"),
        ("--history-events", "50000"),
        ("--history-age", "21600"),
        ("--max-live-views", "64"),
        ("--max-connections", "256"),
        ("--heartbeat-seconds", "15"),
    ];
    for (flag, default) in flags {
        assert!(help.contains(flag), "{flag} missing from:\n{help}");
        assert!(
            help.contains(&format!("[default: {default}]")),
            "{flag}'s default {default} missing from:\n{help}"
        );
    }

    // Every subcommand is named, and a client's help says where its
    // secret comes from.
    let (_, top, _) = run(&["--help"]);
    for command in [
        "serve",
        "get",
        "explain",
        "diagnostics",
        "mint-token",
        "mint-epoch",
        "watch",
    ] {
        assert!(top.contains(command), "{command} missing from:\n{top}");
    }
    let (_, client, _) = run(&["diagnostics", "--help"]);
    assert!(
        client.contains("--token-stdin") && client.contains("TENERAL_TOKEN"),
        "{client}"
    );
}

/// A daemon asked to serve DHCP on an interface no network has claimed
/// does not come up. The alternative is worse than not starting: it
/// would hold the protocol's port on that link and answer nothing,
/// while every client on it waited out its timeouts against a server
/// that was listening.
///
/// This is the one half of the DHCP wiring a test here can reach. The
/// refusal lands before any socket is opened, so it needs none of the
/// privilege the sockets do -- and the sockets themselves cannot be
/// opened by this suite at all, which is why the datapath is measured
/// in a lab with a real client instead.
#[test]
fn refuses_to_serve_dhcp_on_an_interface_no_network_has_claimed() {
    let directory = Directory::new("dhcpunclaimed");
    let mut daemon = Daemon::spawn_with(
        &directory,
        "daemon",
        &directory.store(),
        &directory.socket(),
        &["--dhcp-interface", "teneral0"],
    );

    let status = daemon.exit();

    assert!(!status.success(), "{}", daemon.stderr());
    let said = daemon.stderr();
    for wanted in ["teneral0", "no network has claimed", "root"] {
        assert!(said.contains(wanted), "{wanted:?} missing from:\n{said}");
    }
    // It refused rather than half-started: the control socket it had
    // already bound is gone again.
    assert!(!directory.socket().exists());
}

/// Two sockets on one port and one interface is a conflict the kernel
/// reports as a port already in use, naming neither the flag nor the
/// interface, so the flag is checked before a socket is opened.
#[test]
fn refuses_an_interface_named_twice_before_it_opens_anything() {
    let directory = Directory::new("dhcptwice");
    let mut daemon = Daemon::spawn_with(
        &directory,
        "daemon",
        &directory.store(),
        &directory.socket(),
        &[
            "--dhcp-interface",
            "teneral0",
            "--dhcp-interface",
            "teneral0",
        ],
    );

    let status = daemon.exit();

    assert!(!status.success(), "{}", daemon.stderr());
    let said = daemon.stderr();
    for wanted in ["--dhcp-interface teneral0", "more than once"] {
        assert!(said.contains(wanted), "{wanted:?} missing from:\n{said}");
    }
    assert!(!directory.socket().exists());
}
