//! The client subcommands.
//!
//! Each is one call through the control plane's client and a printing
//! of what came back. Nothing here decides anything the daemon should
//! decide: it does not check a scope, a capability or a bound before
//! asking, because a client that did would be a second copy of the
//! rule and the one place it could disagree with the first.

use std::io::BufRead;
use std::net::Ipv4Addr;

use teneral_api::client::Connection;
use teneral_api::proto::{
    self, Capability, ClientIdentity, DiagnosticsRequest, EveryNetwork, ExplainRequest,
    GetAddressRangeRequest, GetAttachmentRequest, GetBindingRequest, GetLeaseRequest,
    GetNetworkRequest, MintEpochRequest, MintTokenRequest, NetworkNames, ResourceKind, Response,
    Scope, Selector, WatchRequest, request, response, scope, watch_event,
};

use crate::cli::{CapabilityArg, ClientArgs, ClientCommand, KindArg, ReachArgs, Resource};

type Failure = Box<dyn std::error::Error>;

/// The environment variable a token's secret is read from.
const TOKEN_VARIABLE: &str = "TENERAL_TOKEN";

pub async fn run(command: ClientCommand) -> Result<(), Failure> {
    match command {
        ClientCommand::Get {
            connection,
            resource,
        } => one(&connection, get(resource)?).await,
        ClientCommand::Explain {
            connection,
            namespace,
            ifname,
            presents,
        } => {
            let presents = presents
                .iter()
                .map(|text| selector_from_pair(text))
                .collect::<Result<Vec<_>, _>>()?;

            one(
                &connection,
                request::Kind::Explain(ExplainRequest {
                    namespace,
                    ifname,
                    identity: Some(ClientIdentity { presents }),
                }),
            )
            .await
        }
        ClientCommand::Diagnostics { connection } => {
            one(
                &connection,
                request::Kind::Diagnostics(DiagnosticsRequest {}),
            )
            .await
        }
        ClientCommand::MintToken {
            connection,
            name,
            reach,
            capabilities,
        } => {
            one(
                &connection,
                request::Kind::MintToken(MintTokenRequest {
                    name,
                    scope: Some(scope_from(&reach)),
                    capabilities: capabilities
                        .into_iter()
                        .map(|capability| capability_from(capability) as i32)
                        .collect(),
                }),
            )
            .await
        }
        ClientCommand::MintEpoch { connection } => {
            one(&connection, request::Kind::MintEpoch(MintEpochRequest {})).await
        }
        ClientCommand::Watch {
            connection,
            reach,
            kinds,
            after,
        } => {
            let after = after.as_deref().map(cursor_from).transpose()?;

            watch(
                &connection,
                WatchRequest {
                    scope: Some(scope_from(&reach)),
                    kinds: kinds
                        .into_iter()
                        .map(|kind| kind_from(kind) as i32)
                        .collect(),
                    after,
                },
            )
            .await
        }
    }
}

/// One request, and its answer printed.
async fn one(args: &ClientArgs, request: request::Kind) -> Result<(), Failure> {
    let mut connection = connect(args).await?;
    let response = connection.request(request).await?;

    print_response(&response)
}

/// A watch: print every event as it arrives.
async fn watch(args: &ClientArgs, request: WatchRequest) -> Result<(), Failure> {
    let connection = connect(args).await?;
    let mut stream = connection.watch(request).await?;

    loop {
        let event = stream.next().await?;
        println!("{event:#?}");

        // The daemon ending the stream is the one event after which
        // nothing more comes, and it says why in the event.
        if matches!(event.event, Some(watch_event::Event::Disconnecting(_))) {
            return Ok(());
        }
    }
}

/// Print an answer, and turn a refusal into a failure so that a
/// script can tell.
fn print_response(response: &Response) -> Result<(), Failure> {
    println!("{response:#?}");

    match &response.answer {
        Some(response::Answer::Refusal(refusal)) => {
            Err(format!("the daemon refused the request: {}", refusal.detail).into())
        }
        _ => Ok(()),
    }
}

async fn connect(args: &ClientArgs) -> Result<Connection, Failure> {
    let secret = token_secret(args)?;

    Ok(Connection::connect(&args.socket, secret.as_deref()).await?)
}

/// The token's secret, from standard input or the environment and
/// never from an argument.
///
/// An argument is part of the process's command line, which any user
/// on the machine can read from the process table for as long as the
/// process runs, and which a shell's history keeps afterwards. A
/// credential given that way is published to everybody who could
/// otherwise not have it, so this program does not accept one.
fn token_secret(args: &ClientArgs) -> Result<Option<String>, Failure> {
    if args.token_stdin {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|error| format!("reading the token from standard input: {error}"))?;

        let secret = line.trim();
        if secret.is_empty() {
            return Err("standard input held no token".into());
        }

        return Ok(Some(secret.to_owned()));
    }

    Ok(std::env::var(TOKEN_VARIABLE)
        .ok()
        .filter(|secret| !secret.is_empty()))
}

fn get(resource: Resource) -> Result<request::Kind, Failure> {
    Ok(match resource {
        Resource::Network { name } => request::Kind::GetNetwork(GetNetworkRequest { name }),
        Resource::Attachment {
            network,
            namespace,
            ifname,
        } => request::Kind::GetAttachment(GetAttachmentRequest {
            network,
            namespace,
            ifname,
        }),
        Resource::AddressRange { network, name } => {
            request::Kind::GetAddressRange(GetAddressRangeRequest { network, name })
        }
        Resource::Binding {
            network,
            kind,
            value,
        } => request::Kind::GetBinding(GetBindingRequest {
            network,
            selector: Some(selector(&kind, &value)?),
        }),
        Resource::Lease { network, address } => {
            let address: Ipv4Addr = address
                .parse()
                .map_err(|error| format!("{address:?} is not an IPv4 address: {error}"))?;

            request::Kind::GetLease(GetLeaseRequest {
                network,
                address: proto::address(address),
            })
        }
    })
}

fn selector(kind: &str, value: &str) -> Result<Selector, Failure> {
    let kind = proto::selector_kind_named(kind).ok_or_else(|| {
        format!(
            "{kind:?} is not a selector kind; use client-id, duid-iaid, mac, user-class or \
             vendor-class"
        )
    })?;

    Ok(Selector {
        kind: proto::selector_kind(kind) as i32,
        value: value.to_owned(),
    })
}

fn selector_from_pair(text: &str) -> Result<Selector, Failure> {
    let (kind, value) = text
        .split_once('=')
        .ok_or_else(|| format!("{text:?} is not KIND=VALUE"))?;

    selector(kind, value)
}

fn scope_from(reach: &ReachArgs) -> Scope {
    Scope {
        reach: Some(if reach.every_network {
            scope::Reach::EveryNetwork(EveryNetwork {})
        } else {
            scope::Reach::Networks(NetworkNames {
                names: reach.networks.clone(),
            })
        }),
    }
}

fn capability_from(capability: CapabilityArg) -> Capability {
    match capability {
        CapabilityArg::Read => Capability::Read,
        CapabilityArg::Write => Capability::Write,
        CapabilityArg::Watch => Capability::Watch,
        CapabilityArg::Admin => Capability::Admin,
    }
}

fn kind_from(kind: KindArg) -> ResourceKind {
    match kind {
        KindArg::Network => ResourceKind::Network,
        KindArg::Attachment => ResourceKind::Attachment,
        KindArg::AddressRange => ResourceKind::AddressRange,
        KindArg::Binding => ResourceKind::Binding,
        KindArg::Lease => ResourceKind::Lease,
    }
}

/// A position as a person writes one: the epoch, a colon, the
/// sequence. The epoch is split from the right, so that whatever an
/// epoch is made of, the sequence is the last field.
fn cursor_from(text: &str) -> Result<proto::Cursor, Failure> {
    let (epoch, sequence) = text
        .rsplit_once(':')
        .ok_or_else(|| format!("{text:?} is not EPOCH:SEQUENCE"))?;
    let sequence = sequence
        .parse()
        .map_err(|error| format!("{sequence:?} is not a sequence number: {error}"))?;

    Ok(proto::Cursor {
        epoch: epoch.to_owned(),
        sequence,
    })
}
