//! What a connection to the control plane is allowed to be.
//!
//! The handshake, the credential, and the refusals a connection can be
//! met with before it has asked for anything. What a connection may
//! then ask for is the subject of the resource suite beside this one;
//! the harness both share is in `support`.
//!
//! Each test stops the listener it started. The socket goes with it.

mod support;

use std::time::Duration;

use support::{Daemon, TempDirectory};
use teneral_api::client::Connection;
use teneral_api::proto::{self, Hello, HelloResult, RefusalCode, hello_result, request};
use teneral_api::{Authenticated, Error, Listener, PROTOCOL_VERSION, ServerConfig, wire};
use teneral_store::{Capability, TokenScope};
use tokio::net::UnixStream;

/// The refusal a handshake was answered with, or a panic naming what
/// arrived instead.
fn refusal_of(outcome: Result<Connection, Error>) -> proto::Refusal {
    match outcome {
        Ok(_) => panic!("the connection was served when it should have been refused"),
        Err(error) => match error.refusal() {
            Some(refusal) => refusal.clone(),
            None => panic!("the connection failed rather than being refused: {error}"),
        },
    }
}

#[tokio::test]
async fn a_caller_that_is_not_the_owning_peer_and_presents_nothing_is_refused() {
    let directory = TempDirectory::new("not-the-owning-peer");

    // Serving as a uid this test is not running as, so the connection
    // it makes is an ordinary consumer's as far as the kernel is
    // concerned.
    let daemon = Daemon::start(&directory, teneral_api::owning_uid().wrapping_add(1)).await;

    let refusal = refusal_of(Connection::connect(daemon.socket(), None).await);

    assert_eq!(refusal.code, RefusalCode::Unauthenticated as i32);
    assert!(
        refusal.detail.contains("no credential"),
        "the refusal says what was missing: {}",
        refusal.detail
    );

    daemon.stop().await;
}

#[tokio::test]
async fn the_owning_peer_is_served_without_a_credential() {
    let directory = TempDirectory::new("owning-peer");
    let daemon = Daemon::start(&directory, teneral_api::owning_uid()).await;

    let connection = Connection::connect(daemon.socket(), None)
        .await
        .expect("the owning peer is served without a credential");
    let welcome = connection.welcome();

    assert_eq!(welcome.protocol_version, PROTOCOL_VERSION);
    assert_eq!(welcome.schema_version, teneral_store::SCHEMA_VERSION);

    // The position it was welcomed at, with the store's epoch in it.
    // The epoch travels inside the position rather than beside it, so
    // this is also the check that it is carried at all.
    let position = welcome
        .position
        .as_ref()
        .expect("a welcome names a position");
    assert!(!position.epoch.is_empty());

    // Every capability and every network, which is the honest
    // rendering of a caller that has no scope of its own: the first
    // credential has to be mintable by somebody, and this is who.
    assert_eq!(welcome.capabilities.len(), Capability::ALL.len());
    let scope = welcome.scope.as_ref().expect("a welcome names a scope");
    assert!(matches!(
        scope.reach,
        Some(proto::scope::Reach::EveryNetwork(_))
    ));

    match welcome
        .identified_as
        .as_ref()
        .and_then(|identification| identification.identity.as_ref())
    {
        Some(proto::identification::Identity::OwningPeer(peer)) => {
            assert_eq!(peer.uid, teneral_api::owning_uid());
        }
        other => panic!("the owning peer was identified as {other:?}"),
    }

    drop(connection);
    daemon.stop().await;
}

#[tokio::test]
async fn a_credential_the_daemon_does_not_hold_is_refused() {
    let directory = TempDirectory::new("unknown-credential");

    // As the owning peer's own uid, so that the refusal is about the
    // credential and not about the peer: a connection presenting a
    // secret is verified whoever makes it.
    let daemon = Daemon::start(&directory, teneral_api::owning_uid()).await;

    let refusal = refusal_of(Connection::connect(daemon.socket(), Some("not-a-secret")).await);

    assert_eq!(refusal.code, RefusalCode::Unauthenticated as i32);

    daemon.stop().await;
}

#[tokio::test]
async fn a_minted_credential_is_welcomed_with_the_scope_it_was_given() {
    let directory = TempDirectory::new("minted-credential");
    let daemon = Daemon::start(&directory, teneral_api::owning_uid()).await;

    let minted = daemon
        .mint(
            "alpha-reader",
            TokenScope::Networks(vec!["alpha".to_owned()]),
            &[Capability::Read],
        )
        .await;

    let connection = Connection::connect(daemon.socket(), Some(&minted.secret))
        .await
        .expect("a minted credential is served");
    let welcome = connection.welcome();

    // The scope it was granted rather than the scope it asked for,
    // which is the thing worth repeating back: a consumer comparing
    // these with what it believes it holds catches a wrong credential
    // before it acts on an answer.
    let scope = welcome.scope.as_ref().expect("a welcome names a scope");
    match &scope.reach {
        Some(proto::scope::Reach::Networks(names)) => {
            assert_eq!(names.names, vec!["alpha".to_owned()]);
        }
        other => panic!("a credential scoped to one network was welcomed with {other:?}"),
    }

    assert_eq!(
        welcome.capabilities,
        vec![proto::Capability::Read as i32],
        "a welcome names what the credential may do"
    );

    match welcome
        .identified_as
        .as_ref()
        .and_then(|identification| identification.identity.as_ref())
    {
        Some(proto::identification::Identity::Token(token)) => {
            assert_eq!(token.id, minted.token.id.as_str());
            assert_eq!(token.name, "alpha-reader");
        }
        other => panic!("a credential was identified as {other:?}"),
    }

    drop(connection);
    daemon.stop().await;
}

#[tokio::test]
async fn an_operator_operation_is_refused_to_a_credential_without_the_admin_capability() {
    let directory = TempDirectory::new("admin-refused");
    let daemon = Daemon::start(&directory, teneral_api::owning_uid()).await;

    // Everything except admin, which is the case worth testing: a
    // credential with no capabilities at all would be refused even by
    // a check that asked for the wrong one.
    let minted = daemon
        .mint(
            "nearly-everything",
            TokenScope::Networks(vec!["alpha".to_owned()]),
            &[Capability::Read, Capability::Write, Capability::Watch],
        )
        .await;

    // The decision is in one function, so it is asserted there as well
    // as over the socket: the function is where an operation added
    // without a capability decision fails to compile, and the socket
    // is where a handler that forgot to call it would show.
    let caller = Authenticated::Token(Box::new(minted.token.clone()));
    assert!(
        caller
            .require_capability(&request::Kind::MintToken(Default::default()))
            .is_err()
    );

    // Through the client for the handshake, and by hand for the
    // requests, because the client in this crate has no way to send
    // one yet -- which is the point of this step and not a gap in the
    // test.
    let connection = Connection::connect(daemon.socket(), Some(&minted.secret))
        .await
        .expect("a minted credential is served");
    let (mut stream, _) = connection.into_parts();

    for kind in [
        // The diagnostics are among these because they are the
        // operator's own: every value they carry is store-wide, and
        // one of them names the credential that last replaced the
        // store's epoch.
        request::Kind::Diagnostics(Default::default()),
        request::Kind::MintToken(Default::default()),
        request::Kind::DeleteToken(Default::default()),
        request::Kind::ListTokens(Default::default()),
        request::Kind::MintEpoch(Default::default()),
    ] {
        wire::write_message(
            &mut stream,
            &proto::Request {
                kind: Some(kind.clone()),
            },
        )
        .await
        .expect("a request is written");

        let response: proto::Response = wire::read_message(&mut stream)
            .await
            .expect("a request is answered");

        match response.answer {
            Some(proto::response::Answer::Refusal(refusal)) => {
                assert_eq!(
                    refusal.code,
                    RefusalCode::MissingCapability as i32,
                    "an operator's operation was not refused for the capability: {refusal:?}"
                );
                assert_eq!(
                    refusal.required_capability,
                    Some(proto::Capability::Admin as i32)
                );
            }
            other => panic!("an operator's operation was answered with {other:?}"),
        }
    }

    drop(stream);
    daemon.stop().await;
}

#[tokio::test]
async fn a_hello_for_a_version_this_daemon_does_not_speak_is_refused_naming_both() {
    let directory = TempDirectory::new("version-mismatch");
    let daemon = Daemon::start(&directory, teneral_api::owning_uid()).await;

    // Written on to the socket by hand, because the client in this
    // crate only ever presents the version it was built with -- which
    // is the right behaviour for a client and the wrong one for this
    // test.
    let mut stream = UnixStream::connect(daemon.socket())
        .await
        .expect("the socket accepts a connection");

    let hello = Hello {
        protocol_version: PROTOCOL_VERSION + 1,
        token_secret: None,
    };
    wire::write_message(&mut stream, &hello)
        .await
        .expect("a hello is written");

    let result: HelloResult = wire::read_message(&mut stream)
        .await
        .expect("a hello is answered");

    match result.outcome {
        Some(hello_result::Outcome::Refusal(refusal)) => {
            assert_eq!(refusal.code, RefusalCode::VersionMismatch as i32);

            let versions = refusal.versions.expect("a version refusal names the pair");
            assert_eq!(versions.presented, PROTOCOL_VERSION + 1);
            assert_eq!(versions.supported, vec![PROTOCOL_VERSION]);

            // The sentence says both, because an operator reads the
            // sentence and not the fields.
            assert!(refusal.detail.contains(&PROTOCOL_VERSION.to_string()));
            assert!(refusal.detail.contains(&(PROTOCOL_VERSION + 1).to_string()));
        }
        other => panic!("a version this daemon does not speak was answered with {other:?}"),
    }

    drop(stream);
    daemon.stop().await;
}

#[tokio::test]
async fn a_frame_over_the_bound_closes_the_connection() {
    let directory = TempDirectory::new("frame-too-large");
    let daemon = Daemon::start(&directory, teneral_api::owning_uid()).await;

    let mut stream = UnixStream::connect(daemon.socket())
        .await
        .expect("the socket accepts a connection");

    // A length prefix over the bound and no body at all. A daemon that
    // allocated first and judged afterwards would be waiting for a
    // body this test never sends; one that judges the prefix refuses
    // and closes, which is what is asserted below.
    let claimed = wire::MAX_MESSAGE_BYTES + 1;
    let mut prefix = Vec::new();
    let mut remaining = claimed;
    loop {
        let seven = u8::try_from(remaining & 0x7f).expect("seven bits fit in a byte");
        remaining >>= 7;
        if remaining == 0 {
            prefix.push(seven);
            break;
        }
        prefix.push(seven | 0x80);
    }

    tokio::io::AsyncWriteExt::write_all(&mut stream, &prefix)
        .await
        .expect("a length prefix is written");

    let result: teneral_api::Result<HelloResult> = wire::read_message(&mut stream).await;

    match result {
        Ok(HelloResult {
            outcome: Some(hello_result::Outcome::Refusal(refusal)),
        }) => {
            assert_eq!(refusal.code, RefusalCode::MessageTooLarge as i32);

            // And the connection is finished with: there is nothing
            // left on the stream to resynchronise on, the body having
            // deliberately not been read.
            let next: teneral_api::Result<HelloResult> = wire::read_message(&mut stream).await;
            assert!(
                matches!(next, Err(Error::Closed { .. })),
                "the connection stayed open after a frame over the bound: {next:?}"
            );
        }
        other => panic!("a frame over the bound was answered with {other:?}"),
    }

    drop(stream);
    daemon.stop().await;
}

#[tokio::test]
async fn a_live_daemon_on_the_path_stops_a_second_one() {
    let directory = TempDirectory::new("already-serving");
    let daemon = Daemon::start(&directory, teneral_api::owning_uid()).await;

    let mut config = ServerConfig::new(daemon.socket());
    config.probe_timeout = Duration::from_millis(250);

    match Listener::bind(config).await {
        Err(Error::InUse { path }) => assert_eq!(path, daemon.socket()),
        Err(other) => panic!("a second daemon failed for the wrong reason: {other}"),
        Ok(_) => panic!("a second daemon bound a socket that was already being served"),
    }

    // And the first one is still serving, which is the half that
    // matters: refusing to start must not have disturbed it.
    let connection = Connection::connect(daemon.socket(), None)
        .await
        .expect("the daemon that was already there is still answering");
    drop(connection);

    daemon.stop().await;
}

#[tokio::test]
async fn a_socket_file_with_nothing_behind_it_is_removed_rather_than_inherited() {
    let directory = TempDirectory::new("stale-socket");

    // What a daemon that was killed leaves behind. Dropping a bound
    // socket closes it and leaves its file, which is exactly the
    // state: the path is there and nothing is listening on it.
    let socket = directory.socket();
    drop(std::os::unix::net::UnixListener::bind(&socket).expect("a socket to leave behind"));
    assert!(socket.exists(), "the file outlives the socket");

    let daemon = Daemon::start(&directory, teneral_api::owning_uid()).await;

    let connection = Connection::connect(daemon.socket(), None)
        .await
        .expect("a daemon starts on a path whose socket file nothing was behind");
    drop(connection);

    daemon.stop().await;
}

#[tokio::test]
async fn the_socket_is_created_at_mode_0660() {
    let directory = TempDirectory::new("socket-mode");
    let daemon = Daemon::start(&directory, teneral_api::owning_uid()).await;

    let mode = std::os::unix::fs::MetadataExt::mode(
        &std::fs::metadata(daemon.socket()).expect("the socket exists"),
    );

    assert_eq!(
        mode & 0o7777,
        teneral_api::server::SOCKET_MODE,
        "the mode is what decides who may connect at all"
    );

    daemon.stop().await;
}
