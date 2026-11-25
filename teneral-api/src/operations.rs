//! What the daemon answers each request with.
//!
//! One handler per operation, each of them a translation and nothing
//! more: the store takes the network as a parameter of every
//! operation, so no handler here can answer from anything the store
//! would not have scoped, and none of them decides what an answer is.
//!
//! Four rules shape every handler below, and three of them are about
//! what an answer says rather than what it contains.
//!
//! **Every answer carries the position it was true at and the network
//! it was about.** A read's position is the position it was read at; a
//! write's is the position it was assigned. A consumer that asked
//! about one network and is handed a body naming another has caught a
//! cache keyed by address instead of by network and address, which is
//! this project's signature defect, and the position is what lets a
//! consumer that is also watching refuse to apply a read older than
//! its streamed state. A refusal carries neither, because a refusal is
//! a position in nothing and must not name a network to a caller who
//! may not see it.
//!
//! **A read of several things is read through one view.** Several
//! independent reads of a changing store agree most of the time, which
//! is the worst available property: an answer assembled from two of
//! them could describe two states that were never true together, and
//! nothing in the result would say so.
//!
//! **A collection is a sequence of answers ended by a marker**, one
//! item per message, which is what keeps the frame bound a bound on
//! one resource rather than a ceiling on how large a network may grow.
//!
//! **The two questions whose answer is a network are resolved first
//! and checked afterwards.** Which network has claimed an interface,
//! and what the daemon would answer a client arriving on one: both
//! resolve an interface to a network before they can answer at all, so
//! the scope check cannot happen in the asking. It happens on the
//! answer, and it refuses with exactly what a caller is told for an
//! interface no network has claimed -- see [`unclaimed`], which is the
//! one place that refusal is built. The lookup on an interface is not
//! one of them: it names the network the caller believes the interface
//! belongs to, and that is the whole point of it.

use std::net::Ipv4Addr;

use teneral_store::{
    BindingStore, Capability as StoreCapability, ClientIdentity, Cursor as StoreCursor,
    Error as StoreError, ReadHandle, Selector as StoreSelector, TokenId, TokenStore,
};

use crate::PROTOCOL_VERSION;
use crate::authz::Authenticated;
use crate::proto::{self, Response, request, response};
use crate::refusal::{self, Refused};
use crate::resource;
use crate::views::{View, Views};

/// What a resource is called in a refusal this layer builds.
///
/// The store's own words, because a caller comparing the refusal a
/// read gave with the one a write gave should not have to work out
/// whether the two mean the same resource.
const NETWORK: &str = "network";
/// What a refusal calls an attachment.
const ATTACHMENT: &str = "attachment";
/// What a refusal calls an address range.
const RANGE: &str = "address range";
/// What a refusal calls a binding.
const BINDING: &str = "binding";
/// What a refusal calls a lease.
const LEASE: &str = "lease";

/// What a handler produces: the answers to write, or the one refusal
/// to write instead of them.
type Answered = Result<Vec<Response>, Refused>;

/// What a handler answers from.
///
/// Borrowed rather than owned, and built per request rather than per
/// connection, so that nothing here can outlive the connection it is
/// serving or be mistaken for state the daemon keeps.
pub(crate) struct Context<'a> {
    /// The store, reached only through its trait.
    pub store: &'a dyn BindingStore,
    /// The credentials, which are not resources in the store.
    pub tokens: &'a dyn TokenStore,
    /// Who the connection is acting as, settled at the handshake.
    pub caller: &'a Authenticated,
    /// The bound on how many views of the store this daemon holds at
    /// once, which every read here goes through and the diagnostics
    /// report.
    pub views: &'a Views,
}

impl Context<'_> {
    /// A view of the store, which every read in one answer is taken
    /// from.
    ///
    /// The one place an operation opens a view, which is what makes
    /// the bound on them a property of this function rather than
    /// something each handler is trusted to ask about. A daemon
    /// already holding as many as it will refuses with the named
    /// refusal and a retry hint; see [`crate::views`].
    pub(crate) async fn view(&self) -> Result<View, Refused> {
        self.views.refusing(self.store).await
    }

    /// The position the store has reached.
    ///
    /// Through the channel that announces movement rather than through
    /// a view: a view costs a thread for as long as it is held, and
    /// the current position is the one thing that channel always
    /// holds. It is what an answer that read nothing from a view
    /// carries.
    fn position(&self) -> proto::Cursor {
        let positions = self.store.subscribe();

        proto::cursor(&StoreCursor::clone(&positions.borrow()))
    }
}

/// Answer one request.
///
/// The capability and the scope have already been checked, by the two
/// functions in [`crate::authz`] that decide both exhaustively. What
/// is left here is the operation.
pub(crate) async fn answer(context: &Context<'_>, request: &request::Kind) -> Vec<Response> {
    match dispatch(context, request).await {
        Ok(responses) => responses,
        Err(refusal) => vec![refused(*refusal)],
    }
}

/// Which handler serves a request.
///
/// Exhaustive, like the two decisions in [`crate::authz`]: an
/// operation added to the schema and not served here does not compile,
/// rather than reaching a caller as an answer that never arrives.
async fn dispatch(context: &Context<'_>, request: &request::Kind) -> Answered {
    match request {
        request::Kind::ListNetworks(_) => list_networks(context).await,
        request::Kind::GetNetwork(request) => get_network(context, request).await,
        request::Kind::CreateNetwork(request) => create_network(context, request).await,
        request::Kind::DeleteNetwork(request) => delete_network(context, request).await,

        request::Kind::ListAttachments(request) => list_attachments(context, request).await,
        request::Kind::GetAttachment(request) => get_attachment(context, request).await,
        request::Kind::AttachInterface(request) => attach_interface(context, request).await,
        request::Kind::DetachInterface(request) => detach_interface(context, request).await,
        request::Kind::InterfaceOwner(request) => interface_owner(context, request).await,

        request::Kind::ListAddressRanges(request) => list_address_ranges(context, request).await,
        request::Kind::GetAddressRange(request) => get_address_range(context, request).await,
        request::Kind::CreateAddressRange(request) => create_address_range(context, request).await,
        request::Kind::UpdateAddressRange(request) => update_address_range(context, request).await,
        request::Kind::DeleteAddressRange(request) => delete_address_range(context, request).await,

        request::Kind::ListBindings(request) => list_bindings(context, request).await,
        request::Kind::GetBinding(request) => get_binding(context, request).await,
        request::Kind::CreateBinding(request) => create_binding(context, request).await,
        request::Kind::UpdateBinding(request) => update_binding(context, request).await,
        request::Kind::DeleteBinding(request) => delete_binding(context, request).await,
        request::Kind::LookupBinding(request) => lookup_binding(context, request).await,
        request::Kind::LookupBindingOnInterface(request) => {
            lookup_binding_on_interface(context, request).await
        }

        request::Kind::ListLeases(request) => list_leases(context, request).await,
        request::Kind::GetLease(request) => get_lease(context, request).await,
        request::Kind::LeasesHeldBy(request) => leases_held_by(context, request).await,
        request::Kind::RevokeLease(request) => revoke_lease(context, request).await,

        request::Kind::Digest(request) => digest(context, request).await,
        request::Kind::Explain(request) => explain(context, request).await,
        request::Kind::Diagnostics(_) => diagnostics(context).await,

        request::Kind::MintToken(request) => mint_token(context, request).await,
        request::Kind::DeleteToken(request) => delete_token(context, request).await,
        request::Kind::ListTokens(_) => list_tokens(context).await,
        request::Kind::MintEpoch(_) => mint_epoch(context).await,

        // A watch is terminal: the connection becomes the stream, so
        // it is taken by [`crate::watch`] before this function is
        // reached and never arrives here. Written out rather than
        // defaulted because the match has no default arm -- an
        // operation added to the schema has to be decided here -- and
        // answered as a failure rather than as a refusal a consumer
        // could act on, there being nothing a consumer could do about
        // a daemon that dispatched its stream as a request.
        request::Kind::Watch(_) => Err(Box::new(refusal::internal(
            "serve a stream as a request and its one answer",
        ))),
    }
}

// ---------------------------------------------------------------------
// Networks.
// ---------------------------------------------------------------------

/// Every network the caller may reach.
///
/// Filtered rather than refused, and the filter is the point: a
/// network's name is its identity, so a listing that named a network
/// outside the caller's scope would say that the network exists --
/// which is the same leak as answering a read about it. Each item
/// names its own network in the envelope; the marker names none,
/// because the collection is about the store rather than about one of
/// them.
async fn list_networks(context: &Context<'_>) -> Answered {
    let view = context.view().await?;
    let position = proto::cursor(view.cursor());

    let networks = view.networks().await.map_err(refuse)?;
    let answers = networks
        .iter()
        .filter(|network| context.caller.may_reach(&network.name))
        .map(|network| {
            answered(
                &position,
                Some(&network.name),
                response::Answer::Resource(resource::of_network(network)),
            )
        })
        .collect();

    Ok(ended(answers, &position, None))
}

/// One network.
async fn get_network(context: &Context<'_>, request: &proto::GetNetworkRequest) -> Answered {
    let view = context.view().await?;
    let position = proto::cursor(view.cursor());

    match view.network(&request.name).await.map_err(refuse)? {
        Some(network) => Ok(vec![answered(
            &position,
            Some(&request.name),
            response::Answer::Resource(resource::of_network(&network)),
        )]),
        None => Err(missing(NETWORK, &request.name)),
    }
}

/// Create a network.
async fn create_network(context: &Context<'_>, request: &proto::CreateNetworkRequest) -> Answered {
    let position = context
        .store
        .create_network(&context.caller.as_actor(), &request.name)
        .await
        .map_err(refuse)?;

    Ok(vec![confirmed(&position, Some(&request.name))])
}

/// Remove a network that is still where the caller read it.
async fn delete_network(context: &Context<'_>, request: &proto::DeleteNetworkRequest) -> Answered {
    let expected = expected(request.expected.as_ref(), NETWORK)?;
    let position = context
        .store
        .delete_network(&context.caller.as_actor(), &request.name, &expected)
        .await
        .map_err(refuse)?;

    Ok(vec![confirmed(&position, Some(&request.name))])
}

// ---------------------------------------------------------------------
// Attachments.
// ---------------------------------------------------------------------

/// Every interface a network has claimed.
async fn list_attachments(
    context: &Context<'_>,
    request: &proto::ListAttachmentsRequest,
) -> Answered {
    let view = context.view().await?;
    let position = proto::cursor(view.cursor());

    let attachments = view.attachments(&request.network).await.map_err(refuse)?;
    let answers = attachments
        .iter()
        .map(|attachment| {
            answered(
                &position,
                Some(&request.network),
                response::Answer::Resource(resource::of_attachment(attachment)),
            )
        })
        .collect();

    Ok(ended(answers, &position, Some(&request.network)))
}

/// One network's claim on one interface.
async fn get_attachment(context: &Context<'_>, request: &proto::GetAttachmentRequest) -> Answered {
    let view = context.view().await?;
    let position = proto::cursor(view.cursor());

    let found = view
        .attachment(&request.network, &request.namespace, &request.ifname)
        .await
        .map_err(refuse)?;

    match found {
        Some(attachment) => Ok(vec![answered(
            &position,
            Some(&request.network),
            response::Answer::Resource(resource::of_attachment(&attachment)),
        )]),
        // An interface another network holds is not found rather than
        // described, because the question was asked inside one network
        // and an answer from another would be the leak.
        None => Err(missing(
            ATTACHMENT,
            &attachment_id(&request.namespace, &request.ifname),
        )),
    }
}

/// Claim an interface for a network.
async fn attach_interface(
    context: &Context<'_>,
    request: &proto::AttachInterfaceRequest,
) -> Answered {
    let position = context
        .store
        .attach_interface(
            &context.caller.as_actor(),
            &request.network,
            &request.namespace,
            &request.ifname,
        )
        .await
        .map_err(refuse)?;

    Ok(vec![confirmed(&position, Some(&request.network))])
}

/// Release a network's claim on an interface.
async fn detach_interface(
    context: &Context<'_>,
    request: &proto::DetachInterfaceRequest,
) -> Answered {
    let expected = expected(request.expected.as_ref(), ATTACHMENT)?;
    let position = context
        .store
        .detach_interface(
            &context.caller.as_actor(),
            &request.network,
            &request.namespace,
            &request.ifname,
            &expected,
        )
        .await
        .map_err(refuse)?;

    Ok(vec![confirmed(&position, Some(&request.network))])
}

/// Which network has claimed an interface.
///
/// The first of the three whose answer is a network. Resolved on one
/// view and then checked against the caller's scope, and the
/// attachment itself read from that same view so that the answer and
/// the resolution are one position apart from nothing.
async fn interface_owner(
    context: &Context<'_>,
    request: &proto::InterfaceOwnerRequest,
) -> Answered {
    let mut view = context.view().await?;
    let position = proto::cursor(view.cursor());

    let network = owner(context, &mut *view, &request.namespace, &request.ifname).await?;
    let network = match network {
        Some(network) => network,
        // Nothing has claimed it, which only a caller that may see
        // every network gets this far to be told.
        None => {
            return Err(unclaimed(&request.namespace, &request.ifname));
        }
    };

    let found = view
        .attachment(&network, &request.namespace, &request.ifname)
        .await
        .map_err(refuse)?;

    match found {
        Some(attachment) => Ok(vec![answered(
            &position,
            Some(&network),
            response::Answer::Resource(resource::of_attachment(&attachment)),
        )]),
        // Resolved to a network at this position and then not there at
        // the same position: the two reads are one view, so this is
        // the store disagreeing with itself rather than a race.
        None => Err(Box::new(refusal::internal(
            "read the interface it had just resolved",
        ))),
    }
}

// ---------------------------------------------------------------------
// Address ranges.
// ---------------------------------------------------------------------

/// Every range a network owns.
async fn list_address_ranges(
    context: &Context<'_>,
    request: &proto::ListAddressRangesRequest,
) -> Answered {
    let view = context.view().await?;
    let position = proto::cursor(view.cursor());

    let ranges = view
        .address_ranges(&request.network)
        .await
        .map_err(refuse)?;
    let answers = ranges
        .iter()
        .map(|range| {
            answered(
                &position,
                Some(&request.network),
                response::Answer::Resource(resource::of_address_range(range)),
            )
        })
        .collect();

    Ok(ended(answers, &position, Some(&request.network)))
}

/// One range.
async fn get_address_range(
    context: &Context<'_>,
    request: &proto::GetAddressRangeRequest,
) -> Answered {
    let view = context.view().await?;
    let position = proto::cursor(view.cursor());

    let found = view
        .address_range(&request.network, &request.name)
        .await
        .map_err(refuse)?;

    match found {
        Some(range) => Ok(vec![answered(
            &position,
            Some(&request.network),
            response::Answer::Resource(resource::of_address_range(&range)),
        )]),
        None => Err(missing(RANGE, &request.name)),
    }
}

/// Give a network a range.
async fn create_address_range(
    context: &Context<'_>,
    request: &proto::CreateAddressRangeRequest,
) -> Answered {
    let position = context
        .store
        .create_address_range(
            &context.caller.as_actor(),
            &request.network,
            &request.name,
            address(request.first),
            address(request.last),
        )
        .await
        .map_err(refuse)?;

    Ok(vec![confirmed(&position, Some(&request.network))])
}

/// Move the ends of a range that is still where the caller read it.
async fn update_address_range(
    context: &Context<'_>,
    request: &proto::UpdateAddressRangeRequest,
) -> Answered {
    let expected = expected(request.expected.as_ref(), RANGE)?;
    let position = context
        .store
        .update_address_range(
            &context.caller.as_actor(),
            &request.network,
            &request.name,
            &expected,
            address(request.first),
            address(request.last),
        )
        .await
        .map_err(refuse)?;

    Ok(vec![confirmed(&position, Some(&request.network))])
}

/// Remove a range that is still where the caller read it.
async fn delete_address_range(
    context: &Context<'_>,
    request: &proto::DeleteAddressRangeRequest,
) -> Answered {
    let expected = expected(request.expected.as_ref(), RANGE)?;
    let position = context
        .store
        .delete_address_range(
            &context.caller.as_actor(),
            &request.network,
            &request.name,
            &expected,
        )
        .await
        .map_err(refuse)?;

    Ok(vec![confirmed(&position, Some(&request.network))])
}

// ---------------------------------------------------------------------
// Bindings.
// ---------------------------------------------------------------------

/// Every binding a network has configured.
async fn list_bindings(context: &Context<'_>, request: &proto::ListBindingsRequest) -> Answered {
    let view = context.view().await?;
    let position = proto::cursor(view.cursor());

    let bindings = view.bindings(&request.network).await.map_err(refuse)?;
    let answers = bindings
        .iter()
        .map(|binding| {
            answered(
                &position,
                Some(&request.network),
                response::Answer::Resource(resource::of_binding(binding)),
            )
        })
        .collect();

    Ok(ended(answers, &position, Some(&request.network)))
}

/// One binding, by the selector that is its identity.
async fn get_binding(context: &Context<'_>, request: &proto::GetBindingRequest) -> Answered {
    let selector = selector(request.selector.as_ref())?;
    let view = context.view().await?;
    let position = proto::cursor(view.cursor());

    let found = view
        .binding(&request.network, &selector)
        .await
        .map_err(refuse)?;

    match found {
        Some(binding) => Ok(vec![answered(
            &position,
            Some(&request.network),
            response::Answer::Resource(resource::of_binding(&binding)),
        )]),
        None => Err(missing(BINDING, &selector.to_string())),
    }
}

/// Configure a network's answer for one client, and say what stood in
/// the way if anything did.
///
/// The assert-and-be-told path. Without the instruction below, an
/// address a lease holds is a conflict naming the lease and its
/// holder; with it, that lease is taken away in the same transaction
/// as the binding is written, so there is no state in which an address
/// was taken from a client and the binding that justified taking it
/// was not written.
async fn create_binding(context: &Context<'_>, request: &proto::CreateBindingRequest) -> Answered {
    let selector = selector(request.selector.as_ref())?;
    let actor = context.caller.as_actor();
    let address = address(request.address);
    let hostname = request.hostname.as_deref();

    let position = match request.revoke_lease.as_ref() {
        // The lease's id and not its last position, because the id
        // changes when and only when the address changes holder: a
        // renewal in between does not invalidate the instruction and a
        // reassignment does.
        Some(holder) => {
            context
                .store
                .create_binding_revoking_lease(
                    &actor,
                    &request.network,
                    &selector,
                    address,
                    hostname,
                    &presented(holder),
                )
                .await
        }
        None => {
            context
                .store
                .create_binding(&actor, &request.network, &selector, address, hostname)
                .await
        }
    }
    .map_err(refuse)?;

    Ok(vec![confirmed(&position, Some(&request.network))])
}

/// Change what a network answers a client, the binding still being
/// where the caller read it.
async fn update_binding(context: &Context<'_>, request: &proto::UpdateBindingRequest) -> Answered {
    let selector = selector(request.selector.as_ref())?;
    let expected = expected(request.expected.as_ref(), BINDING)?;
    let position = context
        .store
        .update_binding(
            &context.caller.as_actor(),
            &request.network,
            &selector,
            &expected,
            address(request.address),
            request.hostname.as_deref(),
        )
        .await
        .map_err(refuse)?;

    Ok(vec![confirmed(&position, Some(&request.network))])
}

/// Remove a binding that is still where the caller read it.
async fn delete_binding(context: &Context<'_>, request: &proto::DeleteBindingRequest) -> Answered {
    let selector = selector(request.selector.as_ref())?;
    let expected = expected(request.expected.as_ref(), BINDING)?;
    let position = context
        .store
        .delete_binding(
            &context.caller.as_actor(),
            &request.network,
            &selector,
            &expected,
        )
        .await
        .map_err(refuse)?;

    Ok(vec![confirmed(&position, Some(&request.network))])
}

/// Which of a network's bindings applies to a client.
///
/// The answer carries the selector it was found under, that being the
/// binding's identity, which is how a consumer is told *which*
/// selector matched: the precedence between kinds is this project's
/// own, so being told is the only way a consumer can check that its
/// understanding agrees with the daemon's.
async fn lookup_binding(context: &Context<'_>, request: &proto::LookupBindingRequest) -> Answered {
    let identity = identity(request.identity.as_ref())?;
    let view = context.view().await?;
    let position = proto::cursor(view.cursor());

    let found = view
        .lookup_binding(&request.network, &identity)
        .await
        .map_err(refuse)?;

    match found {
        Some(binding) => Ok(vec![answered(
            &position,
            Some(&request.network),
            response::Answer::Resource(resource::of_binding(&binding)),
        )]),
        // Nothing configured for this client is a complete answer, and
        // on this wire it is the same refusal as a binding that is not
        // there: both say there is no such resource, and the detail
        // says which was asked.
        None => Err(missing(BINDING, &presented_by(&identity))),
    }
}

/// The same question, asked the way a packet arrives: a client on an
/// interface, inside the network the caller says that interface
/// belongs to.
///
/// An ordinary network-named operation, scoped by the caller's own
/// scope before it is served, and the network in the request is what
/// makes it the one read where the cross-network check can fire on
/// data rather than on a defect. The caller supplies the network and
/// the store resolves the interface's, so the two can disagree -- and
/// a request whose arrival interface resolved to one network while the
/// answer would have come from another is refused and counted. That
/// counter is this project's best alarm, and this is the only thing on
/// the wire that can move it.
async fn lookup_binding_on_interface(
    context: &Context<'_>,
    request: &proto::LookupBindingOnInterfaceRequest,
) -> Answered {
    let identity = identity(request.identity.as_ref())?;
    let view = context.view().await?;
    let position = proto::cursor(view.cursor());

    let found = view
        .lookup_binding_on_interface(
            &request.network,
            &request.namespace,
            &request.ifname,
            &identity,
        )
        .await
        .map_err(|error| disagreed(context, error, &request.namespace, &request.ifname))?;

    match found {
        Some(binding) => Ok(vec![answered(
            &position,
            Some(&request.network),
            response::Answer::Resource(resource::of_binding(&binding)),
        )]),
        None => Err(missing(BINDING, &presented_by(&identity))),
    }
}

// ---------------------------------------------------------------------
// Leases.
//
// Read and revoked, and never created. Derived state is not writable:
// a lease is produced by the daemon answering a client, so a caller
// may read one, watch one and take one away, and cannot invent one.
// That is what keeps the store the single source of truth rather than
// one of two, and the absence of an operation is how it is enforced --
// there is nothing here to refuse.
// ---------------------------------------------------------------------

/// Every lease in a network, expired ones included and saying so.
async fn list_leases(context: &Context<'_>, request: &proto::ListLeasesRequest) -> Answered {
    let view = context.view().await?;
    let position = proto::cursor(view.cursor());

    let leases = view.leases(&request.network).await.map_err(refuse)?;
    let answers = leases
        .iter()
        .map(|lease| {
            answered(
                &position,
                Some(&request.network),
                response::Answer::Resource(resource::of_lease(lease)),
            )
        })
        .collect();

    Ok(ended(answers, &position, Some(&request.network)))
}

/// The lease on one address.
async fn get_lease(context: &Context<'_>, request: &proto::GetLeaseRequest) -> Answered {
    let view = context.view().await?;
    let position = proto::cursor(view.cursor());

    let found = view
        .lease(&request.network, address(request.address))
        .await
        .map_err(refuse)?;

    match found {
        Some(lease) => Ok(vec![answered(
            &position,
            Some(&request.network),
            response::Answer::Resource(resource::of_lease(&lease)),
        )]),
        None => Err(missing(LEASE, &address(request.address).to_string())),
    }
}

/// What this client holds in this network.
///
/// A query of its own rather than a filter over a listing, because
/// this is the question the consumer this API was designed for cannot
/// ask today and the reason it reads a file instead.
async fn leases_held_by(context: &Context<'_>, request: &proto::LeasesHeldByRequest) -> Answered {
    let identity = identity(request.identity.as_ref())?;
    let view = context.view().await?;
    let position = proto::cursor(view.cursor());

    let leases = view
        .leases_held_by(&request.network, &identity)
        .await
        .map_err(refuse)?;
    let answers = leases
        .iter()
        .map(|lease| {
            answered(
                &position,
                Some(&request.network),
                response::Answer::Resource(resource::of_lease(lease)),
            )
        })
        .collect();

    Ok(ended(answers, &position, Some(&request.network)))
}

/// Take a lease away.
async fn revoke_lease(context: &Context<'_>, request: &proto::RevokeLeaseRequest) -> Answered {
    let expected = expected(request.expected.as_ref(), LEASE)?;
    let position = context
        .store
        .revoke_lease(
            &context.caller.as_actor(),
            &request.network,
            address(request.address),
            &expected,
        )
        .await
        .map_err(refuse)?;

    Ok(vec![confirmed(&position, Some(&request.network))])
}

// ---------------------------------------------------------------------
// The diagnostics.
// ---------------------------------------------------------------------

/// One value for a network's configuration.
async fn digest(context: &Context<'_>, request: &proto::DigestRequest) -> Answered {
    let view = context.view().await?;
    let position = proto::cursor(view.cursor());

    let digest = view.digest(&request.network).await.map_err(refuse)?;

    Ok(vec![answered(
        &position,
        Some(&request.network),
        response::Answer::Digest(proto::Digest {
            value: digest.as_str().to_owned(),
        }),
    )])
}

/// What the daemon would answer this client on this interface, with no
/// packet involved.
///
/// The second of the two whose answer is a network, and the one that
/// can answer having resolved to none: a caller that may see every
/// network is told that nothing has claimed the interface, because
/// that is a different fault from a network that has claimed it and
/// configured nothing, and both are the diagnosis this operation
/// exists to give. A caller with a scope is refused instead, for the
/// reason [`unclaimed`] gives.
async fn explain(context: &Context<'_>, request: &proto::ExplainRequest) -> Answered {
    let identity = identity(request.identity.as_ref())?;
    let view = context.view().await?;
    let position = proto::cursor(view.cursor());

    let explanation = view
        .explain(&request.namespace, &request.ifname, &identity)
        .await
        .map_err(refuse)?;

    may_see(
        context,
        explanation.network.as_deref(),
        &request.namespace,
        &request.ifname,
    )?;

    Ok(vec![answered(
        &position,
        explanation.network.as_deref(),
        response::Answer::Explanation(proto::Explanation {
            namespace: explanation.namespace.clone(),
            ifname: explanation.ifname.clone(),
            network: explanation.network.clone(),
            matched: explanation
                .matched
                .as_ref()
                .map(resource::of_binding_message),
            considered: explanation.considered.iter().map(proto::selector).collect(),
        }),
    )])
}

/// What the daemon says about itself.
async fn diagnostics(context: &Context<'_>) -> Answered {
    let identity = context.store.identity().await.map_err(refuse)?;

    Ok(vec![answered(
        &context.position(),
        None,
        response::Answer::Diagnostics(proto::Diagnostics {
            schema_version: context.store.schema_version(),
            protocol_version: PROTOCOL_VERSION,
            epoch_minted_by: identity.minted_by.clone(),
            epoch_minted_at: identity.minted_at,
            live_views: counted(context.store.live_views()),
            max_live_views: counted(context.views.bound()),
            cross_network_answers: context.store.cross_network_answers(),
        }),
    )])
}

// ---------------------------------------------------------------------
// The operator's own operations.
// ---------------------------------------------------------------------

/// Mint a credential, which may grant no more than its minter holds.
///
/// The subset rule is the store's rather than this layer's, and
/// deliberately: an authorisation rule checked only in the caller is
/// one a second caller will skip, and the second caller here is
/// foreseen -- this daemon is not the only thing that will link the
/// store.
async fn mint_token(context: &Context<'_>, request: &proto::MintTokenRequest) -> Answered {
    let scope = match request.scope.as_ref().and_then(proto::scope_from_wire) {
        Some(scope) => scope,
        // A scope that said nothing is refused rather than read as
        // every network: the commonest way to widen a scope by
        // accident is to leave the field empty.
        None => {
            return Err(invalid(
                "a mint says which networks the credential may reach, and the every-network form \
                 is a value a caller writes rather than a field it leaves empty",
            ));
        }
    };

    let capabilities = capabilities(&request.capabilities)?;
    let minted = context
        .tokens
        .mint(
            &context.caller.as_caller(),
            &request.name,
            &scope,
            &capabilities,
        )
        .await
        .map_err(refuse)?;

    Ok(vec![answered(
        &context.position(),
        None,
        response::Answer::MintedToken(proto::MintedToken {
            token: Some(proto::token(&minted.token)),
            secret: minted.secret.clone(),
        }),
    )])
}

/// Remove a credential.
async fn delete_token(context: &Context<'_>, request: &proto::DeleteTokenRequest) -> Answered {
    context
        .tokens
        .delete(
            &context.caller.as_caller(),
            &TokenId::presented(&request.id),
        )
        .await
        .map_err(refuse)?;

    Ok(vec![confirmed_at(&context.position(), None)])
}

/// Every credential inside the caller's own scope.
///
/// Filtered by the store, on the rule a removal uses: a listing that
/// showed a credential scoped to a network the caller may not reach
/// would say that the network exists.
async fn list_tokens(context: &Context<'_>) -> Answered {
    let tokens = context
        .tokens
        .list(&context.caller.as_caller())
        .await
        .map_err(refuse)?;

    let position = context.position();
    let answers = tokens
        .iter()
        .map(|token| {
            answered(
                &position,
                None,
                response::Answer::Token(proto::token(token)),
            )
        })
        .collect();

    Ok(ended(answers, &position, None))
}

/// Replace the store's epoch.
///
/// The most consequential write an operator can make: every position
/// any consumer remembers stops matching, so every one of them is told
/// the epoch changed and reconverges from a snapshot. It is the remedy
/// for a store restored from a file-level backup, which is the one
/// case the epoch cannot catch by itself.
async fn mint_epoch(context: &Context<'_>) -> Answered {
    let position = context
        .store
        .mint_epoch(&context.caller.as_actor())
        .await
        .map_err(refuse)?;
    let position = proto::cursor(&position);

    Ok(vec![answered(
        &position,
        None,
        response::Answer::MintedEpoch(proto::MintedEpoch {
            position: Some(position.clone()),
        }),
    )])
}

// ---------------------------------------------------------------------
// The pieces every handler is built from.
// ---------------------------------------------------------------------

/// An answer, with the position it was true at and the network it was
/// about.
fn answered(position: &proto::Cursor, network: Option<&str>, answer: response::Answer) -> Response {
    Response {
        position: Some(position.clone()),
        network: network.map(str::to_owned),
        answer: Some(answer),
    }
}

/// A write's confirmation, at the position it was assigned.
fn confirmed(position: &StoreCursor, network: Option<&str>) -> Response {
    confirmed_at(&proto::cursor(position), network)
}

/// A write's confirmation, at a position already on the wire.
fn confirmed_at(position: &proto::Cursor, network: Option<&str>) -> Response {
    answered(
        position,
        network,
        response::Answer::Confirmation(proto::Confirmation {}),
    )
}

/// A refusal, which is a position in nothing and names no network.
fn refused(refusal: proto::Refusal) -> Response {
    Response {
        position: None,
        network: None,
        answer: Some(response::Answer::Refusal(refusal)),
    }
}

/// A collection, ended by the marker that says it was the whole of
/// one.
///
/// The count is of what preceded the marker, which is a check a
/// consumer can make cheaply against what it kept -- and the reason a
/// collection ends with a marker at all is that a connection that
/// failed halfway through one would otherwise look exactly like a
/// collection that was that short.
fn ended(
    mut answers: Vec<Response>,
    position: &proto::Cursor,
    network: Option<&str>,
) -> Vec<Response> {
    let count = counted(answers.len());

    answers.push(answered(
        position,
        network,
        response::Answer::CollectionEnd(proto::CollectionEnd { count }),
    ));

    answers
}

/// A count, as the wire carries it.
///
/// Saturating rather than wrapping, because a count that wrapped would
/// be a smaller number than the truth and a consumer comparing it with
/// what it kept would be told they agreed.
fn counted(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// Which network has claimed an interface, at this view's position,
/// and whether the caller may be told.
///
/// Through the view rather than through the store's own
/// store-global read of the same question, so that the answer and
/// whatever else is read to build it are one position apart from
/// nothing: two independent reads of a changing store agree most of
/// the time, which is the property that produces an answer describing
/// two states that were never true together.
/// A unique borrow of the view rather than a shared one, because a
/// view is `Send` and not `Sync`: a shared borrow of one cannot cross
/// an await inside a task, and a connection is a task.
async fn owner(
    context: &Context<'_>,
    view: &mut dyn ReadHandle,
    namespace: &str,
    ifname: &str,
) -> Result<Option<String>, Refused> {
    let owner = view
        .interface_owner(namespace, ifname)
        .await
        .map_err(refuse)?;
    let network = owner.map(|attachment| attachment.network);

    may_see(context, network.as_deref(), namespace, ifname)?;

    Ok(network)
}

/// Refuse unless the caller may be told what an interface resolved to.
///
/// Two facts and one refusal, which is the whole of decision this
/// layer makes about the questions whose answer is a network. A
/// resolved network the caller may not reach is refused, and so is an
/// unresolved one for a caller that has a scope at all -- because a
/// caller told "nothing has claimed that" for one interface and
/// refused for another has learned that the second belongs to a
/// network it may not see, which is the leak with an extra step. A
/// caller whose scope is every network has no network to be kept from,
/// so it is told.
fn may_see(
    context: &Context<'_>,
    network: Option<&str>,
    namespace: &str,
    ifname: &str,
) -> Result<(), Refused> {
    match network {
        Some(network) if context.caller.may_reach(network) => Ok(()),
        Some(_) => Err(unclaimed(namespace, ifname)),
        None if context.caller.reaches_every_network() => Ok(()),
        None => Err(unclaimed(namespace, ifname)),
    }
}

/// What a lookup on an interface failed with.
///
/// This is the one handler where a cross-network failure can be the
/// caller's doing rather than the daemon disagreeing with itself:
/// everywhere else the network a read is scoped by comes from inside
/// the daemon, so the failure means the store could not scope its own
/// answer and the one translation in [`crate::refusal`] is right to
/// call it internal. Here the caller supplies the network and the
/// store resolves the interface's, so the two can disagree about data.
///
/// Which makes the refusal a decision about what the caller may see. A
/// caller whose scope is every network is told which two networks
/// disagreed, there being none it may not see and somebody having to
/// go and look. A caller with a scope gets exactly what it would be
/// told about an interface no network has claimed, from the same
/// function, because a code it could tell apart would say that the
/// interface exists in a network it may not reach -- the same
/// inference that makes answering an unclaimed interface a leak.
///
/// The counter has already moved in both cases. It is the store's, it
/// fires before this is reached, and it must not depend on who asked:
/// an alarm a narrow scope could switch off would be worth nothing.
fn disagreed(context: &Context<'_>, error: StoreError, namespace: &str, ifname: &str) -> Refused {
    match &error {
        StoreError::CrossNetwork {
            kind,
            id,
            asked,
            found,
        } if context.caller.reaches_every_network() => {
            Box::new(refusal::cross_network(kind, id, asked, found))
        }

        StoreError::CrossNetwork { .. } => unclaimed(namespace, ifname),

        _ => refuse(error),
    }
}

/// Nothing this caller may see has claimed that interface.
///
/// One function for two different facts, which is the point. A caller
/// whose scope does not reach the network holding an interface is
/// refused with exactly what it would be told about an interface no
/// network has claimed, because the difference between those two
/// answers is itself the leak: "that one belongs to a network you may
/// not see" says that the network exists. Built here so that the two
/// cannot drift apart in the wording of a detail.
fn unclaimed(namespace: &str, ifname: &str) -> Refused {
    missing(ATTACHMENT, &attachment_id(namespace, ifname))
}

/// There is no such resource.
fn missing(kind: &str, id: &str) -> Refused {
    Box::new(refusal::no_such_resource(kind, id))
}

/// The request cannot be acted on at all.
fn invalid(detail: &str) -> Refused {
    Box::new(refusal::invalid_request(detail))
}

/// A store failure or refusal, as this wire carries it.
///
/// Through the one translation in [`crate::refusal`], so that two
/// handlers cannot come to disagree about whether a compare-and-set
/// failure is a conflict.
fn refuse(error: StoreError) -> Refused {
    Box::new(refusal::for_store_error(&error))
}

/// An interface's identity, as the store renders it.
fn attachment_id(namespace: &str, ifname: &str) -> String {
    format!("{namespace}/{ifname}")
}

/// What a refusal calls a client that presented these attributes.
///
/// The selectors in the precedence order the lookup consulted them in,
/// so that a refusal says what was looked for and not merely that
/// nothing was found.
fn presented_by(identity: &ClientIdentity) -> String {
    let selectors = identity.selectors();

    if selectors.is_empty() {
        "a client presenting nothing".to_owned()
    } else {
        selectors
            .iter()
            .map(StoreSelector::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The position a request carried, or a refusal saying it carried
/// none.
///
/// A compare-and-set is not ceremony: expressed as a read and then a
/// write it is correct only until something gets in between, and two
/// writers resizing one pool from the same stale read is how a network
/// comes to answer from addresses something else was told it owned. So
/// a request that left the position out is refused rather than served
/// as an unconditional write.
fn expected(position: Option<&proto::Cursor>, kind: &str) -> Result<StoreCursor, Refused> {
    match position {
        Some(position) => Ok(presented(position)),
        None => Err(invalid(&format!(
            "changing a {kind} carries the position the caller expects it to be at, and this \
             request carried none"
        ))),
    }
}

/// A position a caller presented.
///
/// An epoch this store does not issue positions in fails every
/// comparison it is used in, which is the answer wanted rather than a
/// thing to check here: a caller holding a position from another
/// store's order is told so by the operation it presented it to.
fn presented(position: &proto::Cursor) -> StoreCursor {
    StoreCursor::presented(&position.epoch, position.sequence)
}

/// An address a request carried.
fn address(value: u32) -> Ipv4Addr {
    proto::address_from_wire(value)
}

/// The selector a request named, or a refusal saying what was wrong
/// with it.
///
/// The value goes through the store's own constructor, so a selector
/// cannot reach a write in a form a lookup would not match: a hardware
/// address is normalised, and the kinds that have no canonical form
/// are stored as given.
fn selector(selector: Option<&proto::Selector>) -> Result<StoreSelector, Refused> {
    let selector = match selector {
        Some(selector) => selector,
        None => {
            return Err(invalid(
                "that operation names the selector a binding is configured against, and this \
                 request named none",
            ));
        }
    };

    let kind = proto::SelectorKind::try_from(selector.kind)
        .ok()
        .and_then(proto::selector_kind_from_wire);

    match kind {
        Some(kind) => StoreSelector::new(kind, &selector.value).map_err(refuse),
        None => Err(invalid(
            "that selector names an attribute this daemon does not know, and a selector kind is \
             not a thing to guess at",
        )),
    }
}

/// The client identity a request named.
///
/// A message nobody set is an identity presenting nothing, which is a
/// real question rather than a malformed one: asking what a completely
/// unknown client would be answered is how an operator checks that the
/// catch-all they configured is the one that catches it.
fn identity(identity: Option<&proto::ClientIdentity>) -> Result<ClientIdentity, Refused> {
    let mut built = ClientIdentity::new();

    let Some(identity) = identity else {
        return Ok(built);
    };

    for presented in &identity.presents {
        let kind = proto::SelectorKind::try_from(presented.kind)
            .ok()
            .and_then(proto::selector_kind_from_wire);

        match kind {
            Some(kind) => built = built.with(kind, &presented.value).map_err(refuse)?,
            None => {
                return Err(invalid(
                    "that identity presents an attribute this daemon does not know, and a \
                     selector kind is not a thing to guess at",
                ));
            }
        }
    }

    Ok(built)
}

/// The capabilities a mint asked for.
fn capabilities(values: &[i32]) -> Result<Vec<StoreCapability>, Refused> {
    let mut capabilities = Vec::with_capacity(values.len());

    for value in values {
        let capability = proto::Capability::try_from(*value)
            .ok()
            .and_then(proto::capability_from_wire);

        match capability {
            Some(capability) => capabilities.push(capability),
            None => {
                return Err(invalid(
                    "that mint asks for a capability this daemon does not know, and a capability \
                     is not a thing to guess at",
                ));
            }
        }
    }

    Ok(capabilities)
}
