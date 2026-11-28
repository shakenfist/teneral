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
    self, AttachInterfaceRequest, Capability, ClientIdentity, CreateAddressRangeRequest,
    CreateBindingRequest, CreateNetworkRequest, DeleteAddressRangeRequest, DeleteBindingRequest,
    DeleteNetworkRequest, DeleteTokenRequest, DetachInterfaceRequest, DiagnosticsRequest,
    DigestRequest, EveryNetwork, ExplainRequest, GetAddressRangeRequest, GetAttachmentRequest,
    GetBindingRequest, GetLeaseRequest, GetNetworkRequest, InterfaceOwnerRequest,
    LeasesHeldByRequest, ListAddressRangesRequest, ListAttachmentsRequest, ListBindingsRequest,
    ListLeasesRequest, ListNetworksRequest, ListTokensRequest, LookupBindingOnInterfaceRequest,
    LookupBindingRequest, MintEpochRequest, MintTokenRequest, NetworkNames, ResourceKind, Response,
    RevokeLeaseRequest, Scope, Selector, UpdateAddressRangeRequest, UpdateBindingRequest,
    UpdateNetworkRequest, WatchRequest, request, response, scope, watch_event,
};

use crate::cli::{
    CapabilityArg, ClientArgs, ClientCommand, Collection, KindArg, PresentsArgs, Query, ReachArgs,
    Resource,
};

type Failure = Box<dyn std::error::Error>;

/// The environment variable a token's secret is read from.
const TOKEN_VARIABLE: &str = "TENERAL_TOKEN";

pub async fn run(command: ClientCommand) -> Result<(), Failure> {
    let (connection, asked) = asked(command)?;

    match asked {
        Asked::One(request) => one(&connection, request).await,
        Asked::Collection(request) => collection(&connection, request).await,
        Asked::Stream(request) => watch(&connection, request).await,
    }
}

/// What a subcommand asks for, with the asking left out.
///
/// The three shapes an answer arrives in, named so that the mapping
/// from a command line to an operation can be read -- and checked --
/// without a daemon to ask. That check is the test at the end of this
/// file: an operation the schema carries and no subcommand reaches is
/// a hole nothing else in the repository would report.
enum Asked {
    /// One request, answered by one response.
    One(request::Kind),

    /// One request, answered by a sequence of responses ended by a
    /// marker.
    Collection(request::Kind),

    /// A watch, which is terminal. It travels as the request the
    /// client library wraps rather than as a `request::Kind`, because
    /// asking for a stream consumes the connection and nothing may be
    /// sent on it afterwards.
    Stream(WatchRequest),
}

/// A subcommand read as the connection it wants and the operation it
/// asks for.
///
/// Every argument is parsed here, before anything is opened, so that a
/// malformed address or position is reported without a socket having
/// been touched.
fn asked(command: ClientCommand) -> Result<(ClientArgs, Asked), Failure> {
    Ok(match command {
        ClientCommand::Get {
            connection,
            resource,
        } => (connection, Asked::One(get(resource)?)),
        ClientCommand::List {
            connection,
            collection,
        } => (connection, Asked::Collection(list(collection))),
        ClientCommand::Lookup { connection, query } => (connection, lookup(query)?),
        ClientCommand::Explain {
            connection,
            namespace,
            ifname,
            identity,
        } => (
            connection,
            Asked::One(request::Kind::Explain(ExplainRequest {
                namespace,
                ifname,
                identity: presented(&identity)?,
            })),
        ),
        ClientCommand::Digest {
            connection,
            network,
        } => (
            connection,
            Asked::One(request::Kind::Digest(DigestRequest { network })),
        ),
        ClientCommand::Diagnostics { connection } => (
            connection,
            Asked::One(request::Kind::Diagnostics(DiagnosticsRequest {})),
        ),
        ClientCommand::CreateNetwork { connection, name } => (
            connection,
            Asked::One(request::Kind::CreateNetwork(CreateNetworkRequest { name })),
        ),
        ClientCommand::UpdateNetwork {
            connection,
            name,
            lease_seconds,
            position,
        } => (
            connection,
            Asked::One(request::Kind::UpdateNetwork(UpdateNetworkRequest {
                name,
                expected: Some(cursor_from(&position.expected)?),
                lease_seconds,
            })),
        ),
        ClientCommand::DeleteNetwork {
            connection,
            name,
            position,
        } => (
            connection,
            Asked::One(request::Kind::DeleteNetwork(DeleteNetworkRequest {
                name,
                expected: Some(cursor_from(&position.expected)?),
            })),
        ),
        ClientCommand::AttachInterface {
            connection,
            network,
            namespace,
            ifname,
        } => (
            connection,
            Asked::One(request::Kind::AttachInterface(AttachInterfaceRequest {
                network,
                namespace,
                ifname,
            })),
        ),
        ClientCommand::DetachInterface {
            connection,
            network,
            namespace,
            ifname,
            position,
        } => (
            connection,
            Asked::One(request::Kind::DetachInterface(DetachInterfaceRequest {
                network,
                namespace,
                ifname,
                expected: Some(cursor_from(&position.expected)?),
            })),
        ),
        ClientCommand::CreateAddressRange {
            connection,
            network,
            name,
            first,
            last,
        } => (
            connection,
            Asked::One(request::Kind::CreateAddressRange(
                CreateAddressRangeRequest {
                    network,
                    name,
                    first: address(&first)?,
                    last: address(&last)?,
                },
            )),
        ),
        ClientCommand::UpdateAddressRange {
            connection,
            network,
            name,
            first,
            last,
            position,
        } => (
            connection,
            Asked::One(request::Kind::UpdateAddressRange(
                UpdateAddressRangeRequest {
                    network,
                    name,
                    expected: Some(cursor_from(&position.expected)?),
                    first: address(&first)?,
                    last: address(&last)?,
                },
            )),
        ),
        ClientCommand::DeleteAddressRange {
            connection,
            network,
            name,
            position,
        } => (
            connection,
            Asked::One(request::Kind::DeleteAddressRange(
                DeleteAddressRangeRequest {
                    network,
                    name,
                    expected: Some(cursor_from(&position.expected)?),
                },
            )),
        ),
        ClientCommand::CreateBinding {
            connection,
            network,
            kind,
            value,
            address: held,
            naming,
            revoke_lease,
        } => (
            connection,
            Asked::One(request::Kind::CreateBinding(CreateBindingRequest {
                network,
                selector: Some(selector(&kind, &value)?),
                address: address(&held)?,
                hostname: naming.hostname,
                revoke_lease: revoke_lease.as_deref().map(cursor_from).transpose()?,
            })),
        ),
        ClientCommand::UpdateBinding {
            connection,
            network,
            kind,
            value,
            address: held,
            naming,
            position,
        } => (
            connection,
            Asked::One(request::Kind::UpdateBinding(UpdateBindingRequest {
                network,
                selector: Some(selector(&kind, &value)?),
                expected: Some(cursor_from(&position.expected)?),
                address: address(&held)?,
                hostname: naming.hostname,
            })),
        ),
        ClientCommand::DeleteBinding {
            connection,
            network,
            kind,
            value,
            position,
        } => (
            connection,
            Asked::One(request::Kind::DeleteBinding(DeleteBindingRequest {
                network,
                selector: Some(selector(&kind, &value)?),
                expected: Some(cursor_from(&position.expected)?),
            })),
        ),
        ClientCommand::RevokeLease {
            connection,
            network,
            address: held,
            position,
        } => (
            connection,
            Asked::One(request::Kind::RevokeLease(RevokeLeaseRequest {
                network,
                address: address(&held)?,
                expected: Some(cursor_from(&position.expected)?),
            })),
        ),
        ClientCommand::MintToken {
            connection,
            name,
            reach,
            capabilities,
        } => (
            connection,
            Asked::One(request::Kind::MintToken(MintTokenRequest {
                name,
                scope: Some(scope_from(&reach)),
                capabilities: capabilities
                    .into_iter()
                    .map(|capability| capability_from(capability) as i32)
                    .collect(),
            })),
        ),
        ClientCommand::DeleteToken { connection, id } => (
            connection,
            Asked::One(request::Kind::DeleteToken(DeleteTokenRequest { id })),
        ),
        ClientCommand::MintEpoch { connection } => (
            connection,
            Asked::One(request::Kind::MintEpoch(MintEpochRequest {})),
        ),
        ClientCommand::Watch {
            connection,
            reach,
            kinds,
            after,
        } => {
            let after = after.as_deref().map(cursor_from).transpose()?;

            (
                connection,
                Asked::Stream(WatchRequest {
                    scope: Some(scope_from(&reach)),
                    kinds: kinds
                        .into_iter()
                        .map(|kind| kind_from(kind) as i32)
                        .collect(),
                    after,
                }),
            )
        }
    })
}

/// One request, and its answer printed.
async fn one(args: &ClientArgs, request: request::Kind) -> Result<(), Failure> {
    let mut connection = connect(args).await?;
    let response = connection.request(request).await?;

    print_response(&response)
}

/// One request, and the whole of the collection it answers with.
///
/// The marker at the end is printed with the rest rather than dropped:
/// the count it carries is what distinguishes a collection that was
/// that short from a connection that failed halfway through one, and a
/// person reading the output is the one making that comparison.
async fn collection(args: &ClientArgs, request: request::Kind) -> Result<(), Failure> {
    let mut connection = connect(args).await?;

    for response in connection.request_collection(request).await? {
        print_response(&response)?;
    }

    Ok(())
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
        Resource::Lease {
            network,
            address: held,
        } => request::Kind::GetLease(GetLeaseRequest {
            network,
            address: address(&held)?,
        }),
    })
}

/// The listings, none of which takes anything to parse: a collection
/// is named by its kind and at most by the network it is under.
fn list(collection: Collection) -> request::Kind {
    match collection {
        Collection::Networks => request::Kind::ListNetworks(ListNetworksRequest {}),
        Collection::Attachments { network } => {
            request::Kind::ListAttachments(ListAttachmentsRequest { network })
        }
        Collection::AddressRanges { network } => {
            request::Kind::ListAddressRanges(ListAddressRangesRequest { network })
        }
        Collection::Bindings { network } => {
            request::Kind::ListBindings(ListBindingsRequest { network })
        }
        Collection::Leases { network } => request::Kind::ListLeases(ListLeasesRequest { network }),
        Collection::Tokens => request::Kind::ListTokens(ListTokensRequest {}),
    }
}

/// The queries, which answer with a shape rather than with a kind:
/// three of them name one resource and the last names every lease a
/// client holds, so this returns what is asked for rather than the
/// request alone.
fn lookup(query: Query) -> Result<Asked, Failure> {
    Ok(match query {
        Query::InterfaceOwner { namespace, ifname } => {
            Asked::One(request::Kind::InterfaceOwner(InterfaceOwnerRequest {
                namespace,
                ifname,
            }))
        }
        Query::Binding { network, identity } => {
            Asked::One(request::Kind::LookupBinding(LookupBindingRequest {
                network,
                identity: presented(&identity)?,
            }))
        }
        Query::BindingOnInterface {
            namespace,
            ifname,
            network,
            identity,
        } => Asked::One(request::Kind::LookupBindingOnInterface(
            LookupBindingOnInterfaceRequest {
                namespace,
                ifname,
                identity: presented(&identity)?,
                network,
            },
        )),
        Query::LeasesHeldBy { network, identity } => {
            Asked::Collection(request::Kind::LeasesHeldBy(LeasesHeldByRequest {
                network,
                identity: presented(&identity)?,
            }))
        }
    })
}

/// Everything a client presents, as the identity a lookup matches
/// against. The order it was written in is kept: precedence is the
/// store's to decide, so a client that reordered them would be
/// deciding it here.
fn presented(identity: &PresentsArgs) -> Result<Option<ClientIdentity>, Failure> {
    let presents = identity
        .presents
        .iter()
        .map(|text| selector_from_pair(text))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Some(ClientIdentity { presents }))
}

/// An address as a person writes one, as the wire carries it.
fn address(text: &str) -> Result<u32, Failure> {
    let address: Ipv4Addr = text
        .parse()
        .map_err(|error| format!("{text:?} is not an IPv4 address: {error}"))?;

    Ok(proto::address(address))
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

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::cli::{Cli, Command};

    /// The schema, read at compile time.
    ///
    /// It is the only copy of the operation list, so the test reads it
    /// rather than a list kept beside it: a second list would agree
    /// with the schema on the day it was written and never afterwards.
    const SCHEMA: &str = include_str!("../../teneral-api/proto/teneral.v1.proto");

    /// A command line reaching each operation, by the schema's own name
    /// for it, in the order the schema lists them.
    ///
    /// Every line here is parsed and turned into a request below, so a
    /// row is a claim that can be wrong in two ways and no others: the
    /// subcommand does not exist, or it builds a different operation.
    const REACHED: &[(&str, &[&str])] = &[
        ("list_networks", &["teneral", "list", "networks"]),
        ("get_network", &["teneral", "get", "network", "n1"]),
        ("create_network", &["teneral", "create-network", "n1"]),
        (
            "delete_network",
            &["teneral", "delete-network", "n1", "--expected", "e:1"],
        ),
        (
            "update_network",
            &[
                "teneral",
                "update-network",
                "n1",
                "3600",
                "--expected",
                "e:1",
            ],
        ),
        (
            "list_attachments",
            &["teneral", "list", "attachments", "n1"],
        ),
        (
            "get_attachment",
            &["teneral", "get", "attachment", "n1", "/", "br-n1"],
        ),
        (
            "attach_interface",
            &["teneral", "attach-interface", "n1", "/", "br-n1"],
        ),
        (
            "detach_interface",
            &[
                "teneral",
                "detach-interface",
                "n1",
                "/",
                "br-n1",
                "--expected",
                "e:1",
            ],
        ),
        (
            "interface_owner",
            &["teneral", "lookup", "interface-owner", "/", "br-n1"],
        ),
        (
            "list_address_ranges",
            &["teneral", "list", "address-ranges", "n1"],
        ),
        (
            "get_address_range",
            &["teneral", "get", "address-range", "n1", "pool"],
        ),
        (
            "create_address_range",
            &[
                "teneral",
                "create-address-range",
                "n1",
                "pool",
                "10.0.0.10",
                "10.0.0.20",
            ],
        ),
        (
            "update_address_range",
            &[
                "teneral",
                "update-address-range",
                "n1",
                "pool",
                "10.0.0.10",
                "10.0.0.30",
                "--expected",
                "e:1",
            ],
        ),
        (
            "delete_address_range",
            &[
                "teneral",
                "delete-address-range",
                "n1",
                "pool",
                "--expected",
                "e:1",
            ],
        ),
        ("list_bindings", &["teneral", "list", "bindings", "n1"]),
        (
            "get_binding",
            &[
                "teneral",
                "get",
                "binding",
                "n1",
                "mac",
                "02:00:00:00:00:01",
            ],
        ),
        (
            "create_binding",
            &[
                "teneral",
                "create-binding",
                "n1",
                "mac",
                "02:00:00:00:00:01",
                "10.0.0.5",
            ],
        ),
        (
            "update_binding",
            &[
                "teneral",
                "update-binding",
                "n1",
                "mac",
                "02:00:00:00:00:01",
                "10.0.0.6",
                "--expected",
                "e:1",
            ],
        ),
        (
            "delete_binding",
            &[
                "teneral",
                "delete-binding",
                "n1",
                "mac",
                "02:00:00:00:00:01",
                "--expected",
                "e:1",
            ],
        ),
        (
            "lookup_binding",
            &[
                "teneral",
                "lookup",
                "binding",
                "n1",
                "--present",
                "mac=02:00:00:00:00:01",
            ],
        ),
        (
            "lookup_binding_on_interface",
            &[
                "teneral",
                "lookup",
                "binding-on-interface",
                "/",
                "br-n1",
                "n1",
                "--present",
                "mac=02:00:00:00:00:01",
            ],
        ),
        ("list_leases", &["teneral", "list", "leases", "n1"]),
        ("get_lease", &["teneral", "get", "lease", "n1", "10.0.0.5"]),
        (
            "leases_held_by",
            &[
                "teneral",
                "lookup",
                "leases-held-by",
                "n1",
                "--present",
                "mac=02:00:00:00:00:01",
            ],
        ),
        (
            "revoke_lease",
            &[
                "teneral",
                "revoke-lease",
                "n1",
                "10.0.0.5",
                "--expected",
                "e:1",
            ],
        ),
        ("digest", &["teneral", "digest", "n1"]),
        (
            "explain",
            &[
                "teneral",
                "explain",
                "/",
                "br-n1",
                "--present",
                "mac=02:00:00:00:00:01",
            ],
        ),
        ("diagnostics", &["teneral", "diagnostics"]),
        (
            "mint_token",
            &[
                "teneral",
                "mint-token",
                "reader",
                "--every-network",
                "--capability",
                "read",
            ],
        ),
        ("delete_token", &["teneral", "delete-token", "t1"]),
        ("list_tokens", &["teneral", "list", "tokens"]),
        ("mint_epoch", &["teneral", "mint-epoch"]),
        (
            "watch",
            &["teneral", "watch", "--every-network", "--kind", "lease"],
        ),
    ];

    /// The schema's name for an operation.
    ///
    /// Exhaustive with no default arm, for the same reason the server's
    /// capability decision is: an operation added to the schema stops
    /// this compiling, which is where the author of the next one learns
    /// that the debugging client has to reach it too.
    fn operation_name(kind: &request::Kind) -> &'static str {
        match kind {
            request::Kind::ListNetworks(_) => "list_networks",
            request::Kind::GetNetwork(_) => "get_network",
            request::Kind::CreateNetwork(_) => "create_network",
            request::Kind::DeleteNetwork(_) => "delete_network",
            request::Kind::UpdateNetwork(_) => "update_network",
            request::Kind::ListAttachments(_) => "list_attachments",
            request::Kind::GetAttachment(_) => "get_attachment",
            request::Kind::AttachInterface(_) => "attach_interface",
            request::Kind::DetachInterface(_) => "detach_interface",
            request::Kind::InterfaceOwner(_) => "interface_owner",
            request::Kind::ListAddressRanges(_) => "list_address_ranges",
            request::Kind::GetAddressRange(_) => "get_address_range",
            request::Kind::CreateAddressRange(_) => "create_address_range",
            request::Kind::UpdateAddressRange(_) => "update_address_range",
            request::Kind::DeleteAddressRange(_) => "delete_address_range",
            request::Kind::ListBindings(_) => "list_bindings",
            request::Kind::GetBinding(_) => "get_binding",
            request::Kind::CreateBinding(_) => "create_binding",
            request::Kind::UpdateBinding(_) => "update_binding",
            request::Kind::DeleteBinding(_) => "delete_binding",
            request::Kind::LookupBinding(_) => "lookup_binding",
            request::Kind::LookupBindingOnInterface(_) => "lookup_binding_on_interface",
            request::Kind::ListLeases(_) => "list_leases",
            request::Kind::GetLease(_) => "get_lease",
            request::Kind::LeasesHeldBy(_) => "leases_held_by",
            request::Kind::RevokeLease(_) => "revoke_lease",
            request::Kind::Digest(_) => "digest",
            request::Kind::Explain(_) => "explain",
            request::Kind::Diagnostics(_) => "diagnostics",
            request::Kind::MintToken(_) => "mint_token",
            request::Kind::DeleteToken(_) => "delete_token",
            request::Kind::ListTokens(_) => "list_tokens",
            request::Kind::MintEpoch(_) => "mint_epoch",
            request::Kind::Watch(_) => "watch",
        }
    }

    /// Every operation the schema carries, in the order it lists them.
    ///
    /// A few lines of parsing rather than a protobuf descriptor, which
    /// this crate has no other reason to generate or depend on: what is
    /// wanted is the field names of one `oneof`.
    fn operations_in_schema() -> Vec<String> {
        let request = SCHEMA
            .split_once("\nmessage Request {")
            .expect("the schema carries no Request message")
            .1;
        let oneof = request
            .split_once("oneof kind {")
            .expect("Request carries no kind oneof")
            .1;

        let mut operations = Vec::new();
        for line in oneof.lines() {
            let line = line.trim();
            if line == "}" {
                break;
            }

            // A field is "Type name = number;"; everything else in the
            // block is a comment or a blank line.
            if let Some(field) = line.strip_suffix(';')
                && let Some((declaration, _)) = field.split_once(" = ")
                && let Some((_, name)) = declaration.split_once(' ')
            {
                operations.push(name.to_owned());
            }
        }

        operations
    }

    /// The subcommand a command line parses to.
    fn client_command(line: &[&str]) -> ClientCommand {
        let cli = Cli::try_parse_from(line.iter().copied())
            .unwrap_or_else(|error| panic!("{line:?} did not parse: {error}"));

        match cli.command {
            Command::Serve(_) => panic!("{line:?} is the daemon rather than a client subcommand"),
            Command::Client(command) => command,
        }
    }

    /// Which operation a command line asks for.
    fn asked_for(line: &[&str]) -> String {
        let (_, asked) = asked(client_command(line))
            .unwrap_or_else(|error| panic!("{line:?} built no request: {error}"));

        match asked {
            Asked::One(kind) | Asked::Collection(kind) => operation_name(&kind).to_owned(),
            // The one operation no `request::Kind` is built for here,
            // the client library wrapping a watch because a stream is
            // terminal. Named rather than skipped, so that the
            // schema's `watch` is covered like the other thirty-three.
            Asked::Stream(_) => "watch".to_owned(),
        }
    }

    #[test]
    fn every_operation_in_the_schema_is_reachable_from_the_command_line() {
        let operations = operations_in_schema();

        assert_eq!(
            operations.len(),
            34,
            "the schema's operations parsed as {operations:?}"
        );

        for operation in &operations {
            let line = REACHED
                .iter()
                .find(|(named, _)| named == operation)
                .unwrap_or_else(|| panic!("no subcommand reaches {operation}"))
                .1;

            assert_eq!(
                &asked_for(line),
                operation,
                "{line:?} asked for the wrong operation"
            );
        }
    }

    #[test]
    fn no_command_line_claims_an_operation_the_schema_does_not_carry() {
        let operations = operations_in_schema();

        for (operation, line) in REACHED {
            assert!(
                operations.iter().any(|named| named == operation),
                "{line:?} claims {operation}, which the schema does not carry"
            );
        }
    }

    #[test]
    fn a_watch_is_asked_for_as_a_stream_rather_than_as_a_request() {
        let (_, asked) = asked(client_command(&[
            "teneral",
            "watch",
            "--every-network",
            "--kind",
            "lease",
        ]))
        .expect("the watch built no request");

        assert!(
            matches!(asked, Asked::Stream(_)),
            "a watch has to consume the connection rather than travel as one request"
        );
    }

    #[test]
    fn a_listing_is_asked_for_as_a_collection_rather_than_as_one_answer() {
        for line in [
            &["teneral", "list", "networks"][..],
            &["teneral", "list", "leases", "n1"][..],
            &[
                "teneral",
                "lookup",
                "leases-held-by",
                "n1",
                "--present",
                "mac=02:00:00:00:00:01",
            ][..],
        ] {
            let (_, asked) = asked(client_command(line)).expect("built no request");

            assert!(
                matches!(asked, Asked::Collection(_)),
                "{line:?} would be read as one answer and leave the marker on the connection"
            );
        }
    }

    #[test]
    fn a_position_that_is_not_epoch_and_sequence_is_refused_before_anything_is_opened() {
        let command =
            client_command(&["teneral", "delete-network", "n1", "--expected", "fourteen"]);

        assert!(asked(command).is_err());
    }
}
