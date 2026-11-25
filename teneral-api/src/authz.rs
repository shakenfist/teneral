//! Who the caller is, and whether it may ask what it asked.
//!
//! Two things live here and they answer different questions. [`
//! Authenticated`] is who the connection is acting as, settled once at
//! the handshake and unchanged for the life of the connection.
//! [`required_capability`] is which capability a request needs, and it
//! is one exhaustive function that every handler calls.
//!
//! The exhaustive match is load-bearing rather than tidy. The
//! operator's operations are gated by a capability on a credential
//! instead of by a second socket whose mode the kernel would have
//! enforced, so the strongest boundary in this design is a comparison
//! in this file. With two sockets a consumer could not reach the
//! minting path at all, the file system refusing to open the file;
//! with one it reaches the path and is turned away by a check, and a
//! defect in that check is a credential that can mint credentials.
//!
//! Three things hold it, and this file is one of them: an operation
//! added without a decision about who may call it does not compile,
//! because the match below has no default arm. The second is that a
//! credential may only ever grant what it holds itself, which the
//! store enforces rather than trusting this layer to. The third is a
//! test per admin operation.

use teneral_store::{Actor, Caller, Capability as StoreCapability, Token, TokenScope};

use crate::proto::{self, Capability, request};
use crate::refusal::{self, Refused};

/// Who a connection is acting as.
///
/// Settled once, at the handshake, from what the kernel reports about
/// the peer and from the credential it presented. Not re-derived per
/// request: a connection whose identity could change halfway through
/// would make every refusal below a statement about whenever it
/// happened to be checked.
#[derive(Clone, Debug)]
pub enum Authenticated {
    /// A local process connecting as the uid the daemon runs as, read
    /// from the kernel rather than claimed.
    ///
    /// It has no scope and holds every capability, which is what makes
    /// the first credential mintable at all: there is no initial
    /// password, no token file and no first-run secret on disk,
    /// because the one path that needs no credential is the one the
    /// socket's permissions already gate.
    OwningPeer(u32),

    /// A consumer that presented a credential, which narrows and never
    /// widens.
    Token(Box<Token>),
}

impl Authenticated {
    /// The actor every write on this connection is recorded against.
    pub fn as_actor(&self) -> Actor {
        match self {
            Authenticated::OwningPeer(uid) => Actor::Peer(*uid),
            Authenticated::Token(token) => Actor::Token(token.id.clone()),
        }
    }

    /// How the store's credential table knows this caller.
    pub fn as_caller(&self) -> Caller {
        match self {
            Authenticated::OwningPeer(uid) => Caller::OwningPeer(*uid),
            Authenticated::Token(token) => Caller::Token(token.id.clone()),
        }
    }

    /// The networks this caller may reach.
    pub fn scope(&self) -> TokenScope {
        match self {
            // The owning peer has no scope of its own, and every
            // network is the honest rendering of that: it is the caller
            // the socket's permissions already trust, and the first
            // network does not exist yet when it connects.
            Authenticated::OwningPeer(_) => TokenScope::EveryNetwork,
            Authenticated::Token(token) => token.scope.clone(),
        }
    }

    /// What this caller may do, in the store's own order so that a
    /// listing reads the same way every time.
    pub fn capabilities(&self) -> Vec<StoreCapability> {
        match self {
            Authenticated::OwningPeer(_) => StoreCapability::ALL.to_vec(),
            Authenticated::Token(token) => token.capabilities.clone(),
        }
    }

    /// Whether this caller holds `capability`.
    pub fn holds(&self, capability: StoreCapability) -> bool {
        match self {
            Authenticated::OwningPeer(_) => true,
            Authenticated::Token(token) => token.holds(capability),
        }
    }

    /// Whether this caller may reach `network` at all.
    ///
    /// Through the store's own predicate for a credential, rather than
    /// comparing its fields here: a second implementation of "inside
    /// my own scope" in the layer that serves the socket is the one
    /// least able to notice it had got it wrong.
    pub fn may_reach(&self, network: &str) -> bool {
        match self {
            Authenticated::OwningPeer(_) => true,
            Authenticated::Token(token) => token.may_reach(network),
        }
    }

    /// Refuse unless this caller may reach `network`.
    ///
    /// The one place that refusal is built for a named network, so
    /// that no handler has to remember that the refusal must not name
    /// the network it refused.
    pub fn require_reach(&self, network: &str) -> Result<(), Refused> {
        if self.may_reach(network) {
            Ok(())
        } else {
            Err(Box::new(refusal::outside_scope()))
        }
    }

    /// Refuse unless this caller may reach the network `request` names.
    ///
    /// Every handler's second line, and it is one call rather than one
    /// per handler because the list of operations that name no network
    /// is a decision and not an omission: [`named_network`] is where
    /// that decision is made, exhaustively, and this is what acts on
    /// it.
    pub fn require_scope(&self, request: &request::Kind) -> Result<(), Refused> {
        match named_network(request) {
            Some(network) => self.require_reach(network),
            None => Ok(()),
        }
    }

    /// Whether this caller's scope is the every-network form.
    ///
    /// The question the two reads whose answer is a network have to
    /// ask. For such a caller there is no network outside its scope
    /// for an answer to leak, so it is told the whole truth -- that no
    /// network has claimed an interface, where a scoped caller has to
    /// be refused, because being told that much would distinguish an
    /// unclaimed interface from one claimed by a network it may not
    /// see.
    pub fn reaches_every_network(&self) -> bool {
        matches!(self.scope(), TokenScope::EveryNetwork)
    }

    /// Refuse unless this caller may make `request`.
    ///
    /// Every handler's first line. It consults the one function that
    /// decides which capability an operation needs, so a handler
    /// cannot answer a question about authorisation for itself.
    pub fn require_capability(&self, request: &request::Kind) -> Result<(), Refused> {
        let required = required_capability(request);

        if self.holds(required) {
            Ok(())
        } else {
            Err(Box::new(refusal::missing_capability(wire_capability(
                required,
            ))))
        }
    }
}

/// Which capability a request needs.
///
/// Exhaustive, with no default arm, and that is the point: a request
/// added to the schema without a decision about who may make it does
/// not compile. There is no `Option` here either -- every operation
/// needs exactly one capability, and an operation that needed none
/// would be one this socket serves to anybody who can open it.
///
/// The groupings are the ones the capabilities were named for. A read
/// is anything that answers from the store without changing it, which
/// includes the diagnostics and the two lookups: a lookup is a read of
/// the configuration, however much work it does. A write is anything
/// that changes a resource, including taking a lease away, because a
/// revocation moves an address away from a client. Watching is its own
/// capability because a stream is a standing read of everything in a
/// scope rather than one answer, and a consumer that should see a
/// network's current state is not necessarily one that should be told
/// every change in it. Admin is the operator's own operations: the
/// credentials, and the store's epoch.
pub fn required_capability(request: &request::Kind) -> StoreCapability {
    match request {
        request::Kind::ListNetworks(_)
        | request::Kind::GetNetwork(_)
        | request::Kind::ListAttachments(_)
        | request::Kind::GetAttachment(_)
        | request::Kind::InterfaceOwner(_)
        | request::Kind::ListAddressRanges(_)
        | request::Kind::GetAddressRange(_)
        | request::Kind::ListBindings(_)
        | request::Kind::GetBinding(_)
        | request::Kind::LookupBinding(_)
        | request::Kind::LookupBindingOnInterface(_)
        | request::Kind::ListLeases(_)
        | request::Kind::GetLease(_)
        | request::Kind::LeasesHeldBy(_)
        | request::Kind::Digest(_)
        | request::Kind::Explain(_) => StoreCapability::Read,

        request::Kind::CreateNetwork(_)
        | request::Kind::DeleteNetwork(_)
        | request::Kind::AttachInterface(_)
        | request::Kind::DetachInterface(_)
        | request::Kind::CreateAddressRange(_)
        | request::Kind::UpdateAddressRange(_)
        | request::Kind::DeleteAddressRange(_)
        | request::Kind::CreateBinding(_)
        | request::Kind::UpdateBinding(_)
        | request::Kind::DeleteBinding(_)
        | request::Kind::RevokeLease(_) => StoreCapability::Write,

        request::Kind::Watch(_) => StoreCapability::Watch,

        // The diagnostics sit with the operator's operations rather
        // than with the reads, although nothing in them changes
        // anything. Every value they carry is store-wide instead of
        // per-network -- how many views are open, how many answers
        // were refused for having come from the wrong network, and the
        // credential that last replaced the store's epoch, which names
        // a credential to whoever asks. What a consumer legitimately
        // needs about the daemon it is talking to arrives without
        // asking, in the welcome: the position, the schema version and
        // its own scope.
        request::Kind::Diagnostics(_)
        | request::Kind::MintToken(_)
        | request::Kind::DeleteToken(_)
        | request::Kind::ListTokens(_)
        | request::Kind::MintEpoch(_) => StoreCapability::Admin,
    }
}

/// The network a request names, or `None` for one that names none.
///
/// Exhaustive for the same reason [`required_capability`] is: an
/// operation added to the schema without a decision about which
/// network it is scoped by does not compile. The scope check reads
/// this, so the arm an operation lands in *is* whether its network is
/// checked before it runs.
///
/// A `None` here is not a licence. It means one of three things, and
/// each is handled somewhere else: the answer is itself a network, so
/// the check happens after it is resolved and refuses without naming
/// it; the answer spans the store, so it is filtered to what the
/// caller may reach; or the operation is the operator's own, where the
/// store enforces that a credential grants no more than it holds.
pub fn named_network(request: &request::Kind) -> Option<&str> {
    match request {
        request::Kind::GetNetwork(request) => Some(&request.name),
        request::Kind::CreateNetwork(request) => Some(&request.name),
        request::Kind::DeleteNetwork(request) => Some(&request.name),

        request::Kind::ListAttachments(request) => Some(&request.network),
        request::Kind::GetAttachment(request) => Some(&request.network),
        request::Kind::AttachInterface(request) => Some(&request.network),
        request::Kind::DetachInterface(request) => Some(&request.network),

        request::Kind::ListAddressRanges(request) => Some(&request.network),
        request::Kind::GetAddressRange(request) => Some(&request.network),
        request::Kind::CreateAddressRange(request) => Some(&request.network),
        request::Kind::UpdateAddressRange(request) => Some(&request.network),
        request::Kind::DeleteAddressRange(request) => Some(&request.network),

        request::Kind::ListBindings(request) => Some(&request.network),
        request::Kind::GetBinding(request) => Some(&request.network),
        request::Kind::CreateBinding(request) => Some(&request.network),
        request::Kind::UpdateBinding(request) => Some(&request.network),
        request::Kind::DeleteBinding(request) => Some(&request.network),
        request::Kind::LookupBinding(request) => Some(&request.network),
        request::Kind::LookupBindingOnInterface(request) => Some(&request.network),

        request::Kind::ListLeases(request) => Some(&request.network),
        request::Kind::GetLease(request) => Some(&request.network),
        request::Kind::LeasesHeldBy(request) => Some(&request.network),
        request::Kind::RevokeLease(request) => Some(&request.network),

        request::Kind::Digest(request) => Some(&request.network),

        // The two questions whose answer is a network, so there is no
        // network in the asking to check. The lookup on an interface
        // is not one of them: it names the network the caller believes
        // the interface belongs to, which is what lets the store
        // refuse and count the case where the two disagree.
        request::Kind::InterfaceOwner(_) | request::Kind::Explain(_) => None,

        // The listing whose answer is the set of networks, filtered to
        // the ones the caller may reach.
        request::Kind::ListNetworks(_) => None,

        // The operator's own, and the stream, which carries a scope of
        // its own rather than a network.
        request::Kind::Diagnostics(_)
        | request::Kind::MintToken(_)
        | request::Kind::DeleteToken(_)
        | request::Kind::ListTokens(_)
        | request::Kind::MintEpoch(_)
        | request::Kind::Watch(_) => None,
    }
}

/// A capability as the wire names it.
///
/// A thin wrapper so that this module's refusals do not each reach
/// into the conversions.
fn wire_capability(capability: StoreCapability) -> Capability {
    proto::capability(capability)
}

#[cfg(test)]
mod tests {
    use super::{Authenticated, named_network, required_capability};
    use crate::proto::request;
    use teneral_store::Capability as StoreCapability;

    /// Every request kind, built empty, so that a test about
    /// authorisation enumerates the operations rather than a sample of
    /// them. A kind added to the schema and left out here is caught by
    /// the count asserted below.
    ///
    /// The credentials these are checked against are minted by a real
    /// store, in the suite that has one: there is no constructor for a
    /// credential's id outside the store, deliberately, because every
    /// id is supposed to come from a mint.
    fn every_request_kind() -> Vec<request::Kind> {
        vec![
            request::Kind::ListNetworks(Default::default()),
            request::Kind::GetNetwork(Default::default()),
            request::Kind::CreateNetwork(Default::default()),
            request::Kind::DeleteNetwork(Default::default()),
            request::Kind::ListAttachments(Default::default()),
            request::Kind::GetAttachment(Default::default()),
            request::Kind::AttachInterface(Default::default()),
            request::Kind::DetachInterface(Default::default()),
            request::Kind::InterfaceOwner(Default::default()),
            request::Kind::ListAddressRanges(Default::default()),
            request::Kind::GetAddressRange(Default::default()),
            request::Kind::CreateAddressRange(Default::default()),
            request::Kind::UpdateAddressRange(Default::default()),
            request::Kind::DeleteAddressRange(Default::default()),
            request::Kind::ListBindings(Default::default()),
            request::Kind::GetBinding(Default::default()),
            request::Kind::CreateBinding(Default::default()),
            request::Kind::UpdateBinding(Default::default()),
            request::Kind::DeleteBinding(Default::default()),
            request::Kind::LookupBinding(Default::default()),
            request::Kind::LookupBindingOnInterface(Default::default()),
            request::Kind::ListLeases(Default::default()),
            request::Kind::GetLease(Default::default()),
            request::Kind::LeasesHeldBy(Default::default()),
            request::Kind::RevokeLease(Default::default()),
            request::Kind::Digest(Default::default()),
            request::Kind::Explain(Default::default()),
            request::Kind::Diagnostics(Default::default()),
            request::Kind::MintToken(Default::default()),
            request::Kind::DeleteToken(Default::default()),
            request::Kind::ListTokens(Default::default()),
            request::Kind::MintEpoch(Default::default()),
            request::Kind::Watch(Default::default()),
        ]
    }

    /// The operator's own operations, which are what the admin
    /// capability stands in for now that they are not behind a socket
    /// of their own.
    fn admin_request_kinds() -> Vec<request::Kind> {
        vec![
            request::Kind::Diagnostics(Default::default()),
            request::Kind::MintToken(Default::default()),
            request::Kind::DeleteToken(Default::default()),
            request::Kind::ListTokens(Default::default()),
            request::Kind::MintEpoch(Default::default()),
        ]
    }

    #[test]
    fn every_operation_the_schema_carries_is_enumerated_here() {
        // The count is the check. An operation added to the schema and
        // not to the list above would leave the tests below quietly
        // covering less than they claim to, and nothing else would
        // say so: the compiler insists on the match in this module
        // being exhaustive, and cannot insist on a list in a test.
        assert_eq!(every_request_kind().len(), 33);
    }

    #[test]
    fn each_operator_operation_needs_the_admin_capability() {
        // The decision that stands in for the file mode a second
        // socket would have enforced. Asserted at the function rather
        // than by calling the operations, because the function is
        // where the decision is made and a handler that skipped it
        // would be caught by the handler's own test instead.
        for kind in admin_request_kinds() {
            assert_eq!(required_capability(&kind), StoreCapability::Admin);
        }
    }

    #[test]
    fn nothing_that_changes_the_store_is_served_to_a_reader() {
        // The three groups, counted rather than described, so that an
        // operation quietly mapped to read is visible here as a number
        // that moved.
        let mut counts = (0, 0, 0, 0);
        for kind in every_request_kind() {
            match required_capability(&kind) {
                StoreCapability::Read => counts.0 += 1,
                StoreCapability::Write => counts.1 += 1,
                StoreCapability::Watch => counts.2 += 1,
                StoreCapability::Admin => counts.3 += 1,
            }
        }

        assert_eq!(counts, (16, 11, 1, 5));
    }

    #[test]
    fn every_operation_either_names_a_network_or_is_one_of_the_few_that_cannot() {
        // The scope check acts on this, so an operation that stopped
        // naming its network would stop being checked against the
        // caller's scope before it ran -- silently, and in the layer
        // whose whole job is to keep one network's answers out of
        // another's. Counted rather than described, because the number
        // moving is the thing to look at.
        let (named, unnamed) =
            every_request_kind()
                .iter()
                .fold((0, 0), |counts, kind| match named_network(kind) {
                    Some(_) => (counts.0 + 1, counts.1),
                    None => (counts.0, counts.1 + 1),
                });

        // The nine that name none: the listing of networks, the two
        // questions whose answer is a network, the five operator
        // operations, and the stream.
        assert_eq!((named, unnamed), (24, 9));
    }

    /// What an operation does to a lease.
    ///
    /// An exhaustive match with no default arm, over the list above,
    /// and it is here rather than in a suite because the list is here.
    /// What it asserts is an *absence*: there is no arm that creates a
    /// lease, and an operation added to the schema that did would have
    /// to be classified here before this compiled.
    fn against_a_lease(request: &request::Kind) -> &'static str {
        match request {
            request::Kind::ListLeases(_)
            | request::Kind::GetLease(_)
            | request::Kind::LeasesHeldBy(_) => "reads a lease",

            request::Kind::RevokeLease(_) => "takes a lease away",

            request::Kind::ListNetworks(_)
            | request::Kind::GetNetwork(_)
            | request::Kind::CreateNetwork(_)
            | request::Kind::DeleteNetwork(_)
            | request::Kind::ListAttachments(_)
            | request::Kind::GetAttachment(_)
            | request::Kind::AttachInterface(_)
            | request::Kind::DetachInterface(_)
            | request::Kind::InterfaceOwner(_)
            | request::Kind::ListAddressRanges(_)
            | request::Kind::GetAddressRange(_)
            | request::Kind::CreateAddressRange(_)
            | request::Kind::UpdateAddressRange(_)
            | request::Kind::DeleteAddressRange(_)
            | request::Kind::ListBindings(_)
            | request::Kind::GetBinding(_)
            | request::Kind::CreateBinding(_)
            | request::Kind::UpdateBinding(_)
            | request::Kind::DeleteBinding(_)
            | request::Kind::LookupBinding(_)
            | request::Kind::LookupBindingOnInterface(_)
            | request::Kind::Digest(_)
            | request::Kind::Explain(_)
            | request::Kind::Diagnostics(_)
            | request::Kind::MintToken(_)
            | request::Kind::DeleteToken(_)
            | request::Kind::ListTokens(_)
            | request::Kind::MintEpoch(_)
            | request::Kind::Watch(_) => "nothing to do with a lease",
        }
    }

    #[test]
    fn a_lease_cannot_be_created_over_this_wire() {
        // Derived state is not writable. A lease is produced by the
        // daemon answering a client, so a caller may read one, watch
        // one and take one away, and cannot invent one -- which is
        // what keeps the store the single source of truth rather than
        // one of two. The enforcement is that no operation for it
        // exists, so what is checked here is that none has appeared:
        // the classification above is exhaustive, and the counts are
        // what notice a lease operation added quietly to the four
        // this wire has.
        let mut counts = (0, 0, 0);
        for kind in every_request_kind() {
            match against_a_lease(&kind) {
                "reads a lease" => counts.0 += 1,
                "takes a lease away" => counts.1 += 1,
                _ => counts.2 += 1,
            }
        }

        assert_eq!(counts, (3, 1, 29));
    }

    #[test]
    fn the_owning_peer_may_make_every_request() {
        // It has no scope and no capabilities of its own, because it is
        // the caller the socket's permissions already trust and the
        // first credential has to come from somewhere.
        let caller = Authenticated::OwningPeer(0);

        for kind in every_request_kind() {
            assert!(
                caller.require_capability(&kind).is_ok(),
                "the owning peer was refused an operation"
            );
        }

        assert!(caller.require_reach("any-network-at-all").is_ok());
    }
}
