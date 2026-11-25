# The control API

The control API is how anything other than the daemon itself reads
or changes what teneral serves. Protobuf messages travel over a Unix
domain socket. The schema is
[`teneral-api/proto/teneral.v1.proto`](../teneral-api/proto/teneral.v1.proto),
and it is the whole of the wire: the daemon's server and the client a
consumer links are both generated from it, so a field one side writes
and the other reads cannot drift apart. This page is what the schema
cannot say -- what each operation requires of its caller, what a
watch promises, and what an operator does with the few operations
that are theirs.

It lives in the `teneral-api` crate, which holds the schema, the
server (over a `dyn BindingStore`, so it serves whatever store it is
handed), the client and the framing both share. The `teneral`
binary is a thin wrapper: it parses flags, opens the store, serves
the socket and stops cleanly. A consumer in another repository links
`teneral-api` and speaks to a daemon through its client; nothing in
this repository serves a browser, and nothing here listens on TCP.

## Contents

- [The transport](#the-transport)
- [The handshake](#the-handshake)
- [The socket, and who may connect](#the-socket-and-who-may-connect)
- [Capabilities and tokens](#capabilities-and-tokens)
- [What every answer says](#what-every-answer-says)
- [Refusals](#refusals)
- [The operations](#the-operations)
- [The watch stream](#the-watch-stream)
- [Minting an epoch, and restoring a backup](#minting-an-epoch-and-restoring-a-backup)
- [The diagnostics](#the-diagnostics)
- [The daemon and its flags](#the-daemon-and-its-flags)
- [What is not here](#what-is-not-here)

## The transport

Messages are length-delimited in both directions on a `SOCK_STREAM`
socket: a varint byte count, then that many bytes of encoded
message. That is prost's own length-delimited form rather than a
header of this project's invention, so any protobuf library can
speak it.

**A message may be at most 1 MiB (`MAX_MESSAGE_BYTES`), and the bound
is checked against the length prefix before any body is read.** The
input is local and authenticated, which is a reason to expect it to
be well formed and not a reason to allocate whatever a peer's bytes
asked for: one process serves every network, so an out-of-memory
kill is an outage for all of them. A frame over the bound is refused
with `MESSAGE_TOO_LARGE` and the connection is closed -- not
continued, because a stream whose body was deliberately not read has
nothing left to resynchronise on. A length prefix that has not ended
within five bytes cannot encode a length that would be accepted, and
is treated as a framing error: the connection is closed without a
refusal, there being nothing to say it in.

After the handshake, a connection is a sequence of request and
response exchanges, in order, with **no multiplexing and no request
ids**. The handshake is paid once per connection rather than once
per read, which matters to an interface that renders a page out of
five reads, and in-order exchanges need no correlation machinery. A
request is a `Request` carrying exactly one operation; its answer is
one or more `Response` messages.

**A collection is a sequence of responses, one item each, ended by a
`CollectionEnd` carrying a count.** It is not one message holding a
list, and there is no pagination because there is nothing to page:
that keeps the 1 MiB bound a bound on one resource instead of a
ceiling on how large a network may grow, and it means a collection
read and a watch snapshot are one code path. A consumer has the whole
of a collection only when it has seen the marker; anything that goes
wrong before it is a collection it should discard.

**A `Watch` request is terminal.** The connection becomes a stream of
events until either end closes it, and no request follows. That
removes the question of what a request means while a stream is
running.

Clients and servers refuse what they do not understand rather than
guess. A request with no operation set -- what a client built from a
newer schema may send -- is refused as `UNKNOWN_OPERATION`.

## The handshake

A connection opens with the client's `Hello`, answered by a
`HelloResult` that is either a `Welcome` or a `Refusal`. They are a
pair of their own rather than a request kind, so that "a Hello is
the first frame and only the first frame" is a property of the types.

`Hello` carries the protocol version (currently 1) and, optionally,
the secret of a token. A version this daemon does not speak is
refused as `VERSION_MISMATCH`, and the refusal carries both what was
presented and every version the daemon serves, so a mismatch is a
sentence an operator can read rather than a decode failure partway
through a stream. The version moves when a change would make an older
client wrong; adding a field or an operation does not move it, since
an old client ignores a field it does not know and a daemon that does
not serve an operation refuses it as unknown.

`Welcome` carries what the caller needs to know before it acts:

| Field | What it is |
|-------|------------|
| `protocol_version` | The version this connection speaks. There is no negotiation, only agreement or a refusal. |
| `position` | The store's cursor when the connection opened. Its epoch is the store's epoch, carried once so there are not two copies that could disagree. |
| `schema_version` | The store's schema version, for a consumer that wants to refuse a store it was not written for. |
| `scope` and `capabilities` | The networks this caller may reach and what it may do, **resolved** -- not what it asked for. A client that compares them with what it believes it holds catches a wrong credential before acting on an answer rather than after. |
| `identified_as` | Either the owning peer (with its uid) or a token (with its id and name, never its secret). Told plainly because a consumer that believes it presented a token and was in fact served as the owning peer is running with more reach than it thinks. |

The token, if any, is verified once, here, and the identity is fixed
for the life of the connection. It is not re-derived per request: a
connection whose identity could change halfway through would make
every later refusal a statement about when it happened to be checked.

## The socket, and who may connect

There is **one socket**, created with mode `0660`. Which operations a
caller may use is decided by what it holds, not by which socket it
opened.

The mode is what decides who may open the file at all, and the group
on the directory is the deployment's lever: giving a file to a group
needs a privilege the daemon should not spend, so a unit file that
sets up the runtime directory and its group is where that belongs. The
daemon creates the socket under a temporary name, sets the mode,
reads it back, and only then moves it into place, so there is never a
moment when the path exists with a mode nobody chose. If the mode
cannot be set the daemon refuses to start: a socket with an unknown
permission on it is no basis for serving anything.

**A token is required, with exactly one exception: the owning peer.**
A process whose uid, as the kernel reports it for the connection, is
the uid the daemon runs as may omit the token and is served with
every capability and every network. The exception exists because the
first token has to be minted by somebody. Without it, the first
token would have to come from a secret printed at startup or written
to disk, and the whole design is arranged so that there is no initial
password, no token file and no first-run secret to leak: the only
path that needs no credential is the one the socket's permissions
already gate. It applies only to *omitting* a token. A token that is
presented is verified whoever presents it, and a wrong one is refused
rather than quietly downgraded to the owning peer.

Anyone else who connects without a token is refused as
`UNAUTHENTICATED`, and a token the daemon does not hold is refused the
same way. That code is distinct from `MISSING_CAPABILITY` on purpose:
one is a credential to mint and the other a credential to replace.

**What the one-socket arrangement gives up.** With a second,
stricter socket for the operator's operations, a client process could
not reach the minting path at all -- the filesystem would refuse to
open the file. With one socket the client reaches the path and is
turned away by a comparison, so the strongest boundary in the design
is a check in the code rather than a permission the kernel enforces.
A defect in that comparison is a token that can mint tokens. Three
things hold it, and a change that weakens any of them should be
treated as one that weakens the boundary:

1. Which capability a request needs is decided by one exhaustive
   `match` (`required_capability`) with no default arm, which every
   request passes through. An operation added to the schema without a
   decision about who may call it does not compile.
2. The subset rule below, which the store enforces rather than
   trusting the server to.
3. A test per operator operation asserting that a token without
   `admin` is refused.

**Startup refusals.** The daemon will not start if something already
answers a `Hello` on the socket path: taking over a live daemon's
path would leave it holding the store and answering nobody. A
socket file that nothing answers on is the remains of a daemon that
was killed, and is removed and recreated. A path that accepts a
connection and does not answer within the probe timeout (two
seconds) counts as in use, so a slow daemon is not mistaken for a
dead one.

## Capabilities and tokens

There are four capabilities.

| Capability | What it is for |
|------------|----------------|
| `read` | Every operation that answers from the store without changing it: gets, lists, the two lookups, `interface_owner`, `explain` and `digest`. A lookup is a read of the configuration, however much work it does. |
| `write` | Every operation that changes a resource, including `revoke_lease`, because revoking a lease moves an address away from a client. |
| `watch` | The watch stream. Its own capability because a stream is a standing read of everything in a scope rather than one answer: a consumer that should see a network's current state need not be one that should be told every change in it. |
| `admin` | The operator's own operations: `diagnostics`, `mint_token`, `delete_token`, `list_tokens` and `mint_epoch`. |

`diagnostics` is on that list although it changes nothing. Every
value in it is store-wide rather than per-network -- how many views
are open, how many answers were refused for having come from the
wrong network, and which credential last replaced the store's epoch --
and the last one names a credential to whoever asks. What a consumer
legitimately needs to know about the daemon it is talking to arrives
without asking, in the `Welcome`.

A token has an id, a name, a scope, a set of capabilities, and a
secret. The secret is 32 bytes of randomness, shown **once**, in the
answer to the mint, and never again: the store keeps only its SHA-256
hash, and there is no field anywhere on the wire that could carry it
back out. No slow password hash is used because there is no password
to guess; the secret is as large as a key. A consumer that loses it
is minted another.

A scope is either a list of network names or the explicit
`every_network` message. The every-network form is a value a caller
has to write, never an empty field, because the commonest way to widen
a scope by accident is to leave one blank. A scope or a watch that
says nothing is refused as `INVALID_REQUEST`, not read as "every
network". The names in a scope need not exist yet: a scope is a
statement about names, and the first token is minted before any
network has been created.

**A credential narrows and never widens.** The first half of that is
scope: a token cannot reach a network outside its own. The second half
exists because `admin` is a capability: **a minted token's scope and
capabilities must be a subset of its minter's**, and the store enforces
that itself, so a second caller of the store cannot skip it. Without it
a token holding `admin` on one network could mint itself an
every-network token, which is the whole boundary defeated in one call.
The owning peer has no scope, so it may mint anything; every token may
mint only downwards. The same scoping applies to the other two token
operations: a caller may remove, and sees in a listing, only tokens
whose scope lies inside its own, because a listing that showed a token
scoped to a network the caller may not reach would say that network
exists.

Every write records who made it -- the token's id, the owning peer's
uid, or a named part of the daemon -- both in the event log and on the
resource row (`written_by`). It survives on the row after the event has
been evicted from the retained history, and on a token's removal: "who
moved this binding" is a question about the past, and a token removed
this morning is still the answer to it.

## What every answer says

Each `Response` carries, outside the answer itself:

- **The position it was true at.** For a read, the cursor it was read
  at; for a write, the cursor the write was assigned. A consumer that
  is also watching uses it to refuse to apply a read older than its
  streamed state, which is the race that otherwise reverts a correct
  view to a stale one.
- **The network it is about**, repeated even when the question named
  one. A consumer that asked about one network and is handed a body
  naming another has caught a cache keyed by address instead of by
  network and address, in the daemon, in its own client or somewhere
  between them.

A refusal carries neither. It is a position in nothing, and it must
not name a network to a caller who may not see it.

A read of several things is read through one consistent view of the
store, so an answer assembled from several reads cannot describe two
states that were never true together.

Every resource also carries the cursor it was last written at and who
wrote it. An IPv4 address is a 32-bit number, most significant octet
first, rather than text: the conversion is total in both directions,
so no address on this wire can fail to parse.

A request that changes an existing resource -- every update, delete,
detach and `revoke_lease` -- carries the cursor the caller **expects the
resource to be at**, and one that omits it is refused as
`INVALID_REQUEST` rather than served as an unconditional write. A
compare-and-set is not ceremony: expressed as a read followed by a
write it is correct only until something gets in between, and two
writers resizing one pool from the same stale read is how a network
comes to answer from addresses something else was told it owned. If the
resource has moved on, the answer is `CHANGED`, with both positions.

## Refusals

A refusal is a code, then whichever fields that code makes meaningful,
then a sentence for a person. **Nothing should branch on the sentence.**
One rule governs all of them: a refusal never names a resource outside
the caller's scope.

| Code | Meaning | Fields it fills |
|------|---------|-----------------|
| `MISSING_CAPABILITY` | The credential does not hold the capability the request needs. | `required_capability` |
| `OUTSIDE_SCOPE` | The request named a network outside the caller's scope. Answered before the network is looked up, so it is the same whether or not the network exists. | |
| `NO_SUCH_RESOURCE` | There is no such resource. A lookup that matches nothing answers this too, the detail saying which was asked. | `resource_kind`, `resource_id` |
| `CONFLICT` | Something is already there. It is reported and never resolved. | `held_by` names what stands in the way |
| `CHANGED` | The resource moved on between the read and the write. The repair is to read it again and decide. | `expected_position`, `found_position` |
| `TOO_MANY_VIEWS` | The daemon is holding as many concurrent views as it will. A refusal to retry, not an error to report. | `retry_after_seconds` |
| `UNKNOWN_RESOURCE_KIND` | A watch named a kind this daemon does not serve. | |
| `VERSION_MISMATCH` | The protocol version is not one this daemon speaks. | `versions` |
| `UNAUTHENTICATED` | No credential and not the owning peer, or a credential the daemon does not hold. | |
| `INVALID_REQUEST` | Not a request the daemon can act on at all: a name it will not store, an unknown selector or capability, a scope that said nothing, a missing expected position. | |
| `NO_ADDRESS` | The network had no address to give. Distinct from a conflict because nothing is in the way. | |
| `UNKNOWN_OPERATION` | No operation set, or one this build does not answer. | |
| `MESSAGE_TOO_LARGE` | The frame's length prefix is over the bound. The connection closes after it. | |
| `INTERNAL` | The daemon failed rather than refused. The detail says what it was doing and deliberately not what it found, because what it found can name a network the caller may not see. | |

**`CONFLICT` and `CHANGED` are different faults.** In a conflict
something else holds what the caller wanted, so the repair is to take
it or give up; in a change nothing holds it, the caller's picture is
stale, and the repair is to look again.

**Refusing without naming.** Two questions have a network as their
*answer* -- `interface_owner` and `explain` -- so there is no network
in the asking to check. The server resolves the answer first and then
checks it against the caller's scope. If it is outside, the refusal is
exactly the one a caller gets for an interface that no network has
claimed. A caller told "nothing has claimed that" for one interface and
"that belongs to a network you may not see" for another has learned the
second exists, which is the leak with an extra step. A caller whose
scope is every network has no network to be kept from and is told the
truth, including that nothing has claimed the interface.

## The operations

Each operation is a message in the `Request` oneof, one message per
operation even where it would fit in a field, so that an operation can
grow a parameter without touching its neighbours. Field numbers are
permanent and gaps are held, with a comment, for what a resource is
expected to grow.

**Scope.** "Named network" below means the request carries the network
(or, for the network operations, the network's name), and the server
refuses it as `OUTSIDE_SCOPE` before it is run if the caller's scope
does not reach it. That check is one exhaustive function
(`named_network`), for the same reason the capability decision is: an
operation added without a decision about its network does not compile.

### Networks

| Operation | Capability | Scope rule |
|-----------|------------|------------|
| `list_networks` | read | Filtered: the collection holds only the networks the caller may reach. |
| `get_network` | read | Named network. |
| `create_network` | write | Named network: a scoped token may create only a network its scope names. |
| `delete_network` | write | Named network. Needs the expected cursor. |

### Attachments

An attachment is an interface, in a namespace, belonging to exactly one
network. The namespace is part of its identity, since an interface name
is unique only within one. Claiming an interface that another network
has already claimed is refused as a conflict naming the holder; that
constraint is where a cross-network leak would be configured rather than
coded.

| Operation | Capability | Scope rule |
|-----------|------------|------------|
| `list_attachments` | read | Named network. |
| `get_attachment` | read | Named network. |
| `attach_interface` | write | Named network. |
| `detach_interface` | write | Named network. Needs the expected cursor. |
| `interface_owner` | read | Names no network; the answer is one. Resolved, then checked, as above. |

### Address ranges

A contiguous span, both ends inclusive. A network may own several or
none; one with none allocates nothing and only answers from what was
configured.

| Operation | Capability | Scope rule |
|-----------|------------|------------|
| `list_address_ranges` | read | Named network. |
| `get_address_range` | read | Named network. |
| `create_address_range` | write | Named network. |
| `update_address_range` | write | Named network. Needs the expected cursor. |
| `delete_address_range` | write | Named network. Needs the expected cursor. |

### Bindings

A binding is what an operator configured for one client identity in one
network. Its selector -- a kind and a value -- is its identity inside
the network, which is why a lookup can say which selector matched
without any extra field: the matched selector is the binding's
identity. A selector kind is one of `client_id`, `duid_iaid`, `mac`,
`user_class` and `vendor_class`; the same hardware address in two
networks is two unrelated clients, so a selector is never an identity
of its own.

| Operation | Capability | Scope rule |
|-----------|------------|------------|
| `list_bindings` | read | Named network. |
| `get_binding` | read | Named network. |
| `create_binding` | write | Named network. |
| `update_binding` | write | Named network. Needs the expected cursor. The selector cannot change: a binding whose selector moved is a different binding. |
| `delete_binding` | write | Named network. Needs the expected cursor. |
| `lookup_binding` | read | Named network. Which binding applies to a client presenting this identity. |
| `lookup_binding_on_interface` | read | Named network, which the caller supplies out of whatever context it answers in. |

**`create_binding` is the assert-and-be-told path.** A consumer that
chooses its own addresses either has the binding confirmed or is told
what holds the address: a `CONFLICT` whose `held_by` names the lease and
its holder. Setting `revoke_lease` -- to the lease's id, which changes
when and only when the address changes holder, so a renewal in between
does not invalidate the instruction and a reassignment does -- takes
that lease away in the same transaction as the binding is written.
There is no state in which the address was taken from a client and the
binding that justified taking it was never written. Changing an
existing binding onto an address something else holds has no such form:
it is refused with the holder named, which is a legitimate answer.

**A lookup's answer names the selector that matched.** The precedence
between selector kinds is this project's own choice rather than a
protocol's, so the only way a consumer can check that its understanding
agrees with the daemon's is by being told. A client presenting several
selectors is a `ClientIdentity`, a list so that the precedence is
applied in one place; a kind given twice is refused.

**`lookup_binding_on_interface` names the network on purpose.** The
caller supplies the network it believes the interface belongs to, and
the store resolves the interface's own, so the two can disagree. A
request where they do is refused and counted, and that counter is the
one in the diagnostics that should always be zero. A message that named
no network could not express the disagreement, and the guard could then
fire only on a defect inside the store. A caller with a scope is
refused as it would be for an unclaimed interface; a caller whose scope
is every network is told which two networks disagreed, as `INTERNAL`,
because somebody has to go and look.

### Leases

A lease is an address a client is using, as against one an operator
configured for it. It is keyed by network and address, because within
one network an address has at most one holder at a time and a client may
legitimately hold two. **Derived state is not writable**: a lease is
produced by the daemon answering a client, so a caller may read one,
watch one and revoke one, and cannot invent one. The absence of an
operation is how that is enforced.

| Operation | Capability | Scope rule |
|-----------|------------|------------|
| `list_leases` | read | Named network. Expired leases are included, and say so. |
| `get_lease` | read | Named network. |
| `leases_held_by` | read | Named network. What a client holds, as a query of its own rather than a filter over a list. |
| `revoke_lease` | write | Named network. Needs the expected cursor. |

A lease carries its absolute `expires_at`, and an `expired` flag that is
that fact evaluated at the instant the answer was read; a consumer that
holds the answer for longer should use the instant.

### Diagnostics

| Operation | Capability | Scope rule |
|-----------|------------|------------|
| `digest` | read | Named network. |
| `explain` | read | Names no network; the answer is one. Resolved, then checked. |
| `diagnostics` | admin | Store-wide. |

`explain` converts the project's signature failure -- a silently wrong
answer, noticed hours later inside somebody's guest -- into a question an
operator or a test can ask directly. It answers what the daemon would say
to this identity on this interface and how it got there: the network the
interface resolved to (absent if none has claimed it), the binding that
would answer, and the selectors it considered, in the order it considered
them. Every part is a distinct diagnosis: no network means nobody has
claimed the interface; a network with no match means the client would
fall through to whatever the front-end does with a client it does not
know; and the considered list turns "this matched" into "these lost, in
this order", the only complete check of a precedence this project chose.
It is the front-end's own lookup path with the packet replaced by its
parameters, not a second implementation.

`digest` is one opaque value per network that changes when the network's
*configuration* changes -- not when a client takes a lease, or it would
answer "has anything happened" instead of "do we agree". A consumer holding
a mirror compares one value rather than diffing everything, and a mismatch
means resync rather than investigate.

### Credentials and the epoch

All of these need `admin` (or the owning peer).

| Operation | Scope rule |
|-----------|------------|
| `mint_token` | The new token's scope and capabilities must be a subset of the caller's, else refused as `OUTSIDE_SCOPE`. Answered with the token and its secret, once. |
| `delete_token` | Only a token whose scope lies inside the caller's. |
| `list_tokens` | Filtered to tokens inside the caller's scope. Never carries a secret. |
| `mint_epoch` | Store-wide. See below. |

### Watch

| Operation | Capability | Scope rule |
|-----------|------------|------------|
| `watch` | watch | Carries its own scope; see below. Terminal. |

There is deliberately no operation for a zone, a record or a content
blob: the store does not hold them, and a message for one would be a
guess at an interface somebody else will design.

## The watch stream

A consumer needs to learn that the store changed and to stay right
about it. Everything below is about what happens when a consumer has
been away, because that is where a push interface quietly becomes wrong:
a consumer that resumes and silently misses the event in which an
address changed holds a wrong answer forever, and nothing tells it. So a
gap is never silent. It is a *resync*, named, **in band as data on the
stream rather than as a connection error** -- a connection error is
the thing every client retries blindly, and the blind retry is what turns
a gap into permanent divergence.

### The request

A `WatchRequest` has a scope, a set of kinds, and optionally a cursor to
resume after.

- **The scope must be written.** A watch with no scope is refused. A
  scope of named networks must name at least one, and every name must be
  inside the caller's own scope. A watch of every network is refused to a
  caller that does not itself reach every network, rather than narrowed to
  the networks it can reach: a consumer that asked for everything and
  received one network's worth would believe there was nothing else to hear.
- **The kinds must be written.** An empty list is refused. A kind is one of
  `network`, `attachment`, `address_range`, `binding` and `lease`, and a kind
  the daemon does not know is refused as `UNKNOWN_RESOURCE_KIND` rather than
  dropped, so a consumer written against a fuller vocabulary hears that it
  is talking to an older daemon instead of watching a stream that never
  carries what it wanted.
- **`after` absent means a new consumer.** Present, it is a position the
  consumer remembers.

Everything on the stream is a `WatchEvent`, inside a `Response`.

### A new consumer: snapshot, then in sync, then the stream

A consumer with no position receives, in order:

1. **A snapshot**: one event per resource in its scope and kinds, with
   origin `SNAPSHOT`, read on **one consistent view** so a scope covering
   several networks is one state and not several that agree most of the
   time. Within it, events run in position order, which puts a network
   before the resources configured in it.
2. **An `InSync` marker** carrying the position the snapshot was read at.
   This is not a nicety. Without it a consumer cannot tell "I have seen the
   whole world" from "I have seen some of it", so it cannot safely do the
   one thing it most wants to after connecting, which is to delete local
   state for resources the daemon did not mention.
3. **The stream**, starting exactly from that position, with origin `CHANGE`.

Each event is `ResourceEvent`: the cursor, the network (always, including
on a stream watching one), the kind, the resource's identity inside its
network, the origin, the resource **as it now stands**, and who made the
change. A deletion is the same shape with no state. It is the whole
post-state rather than a delta, which makes applying an event idempotent
and so makes at-least-once delivery enough: exactly-once is not on offer
from anything, and designing for it is how a system acquires a defect it
cannot reproduce. A `CHANGE` carries the instant it was recorded; a
snapshot's events do not, because a resource row records the position it
was last written at and not the instant, and a snapshot is a statement
about now. Order by position, which both carry.

**The state in an event is the same message a read returns.** A read of
a resource and the event describing it are produced by one converter per
kind, and there is a test per kind asserting that the two are equal after
encoding. Two spellings of one resource is the defect this design would
otherwise breed, and it would be invisible until a consumer applied a
stream event over a read.

### A returning consumer: resume, or a named resync

A consumer that presents a position gets one of four answers. The store
produces them and the control plane translates them without inventing any.

| Answer | What it means | What the consumer receives |
|--------|---------------|----------------------------|
| **Resume** | The position is one this store issued, in this epoch, and everything after it is still held. | Every event after it in scope, **including intermediate states** -- a lease created and released while the consumer was away produces both events, because a consumer holding the earlier state would otherwise never see it end -- then `InSync`, then the stream. |
| **`EPOCH_CHANGED`** | The position belongs to another order. This is a different store (a database recreated, or an epoch minted). | A `Resync` carrying the position presented and the store's epoch, then a full snapshot and `InSync`. |
| **`EVICTED`** | The epoch matches but the events the consumer needs have been truncated. It was away longer than the history is retained. | A `Resync` carrying the position presented and the highest position that has been truncated, then a snapshot and `InSync`. |
| **`NEVER_ISSUED`** | The epoch matches and the position is *ahead* of anything this store ever assigned. | A `Resync` carrying the position presented and the latest the store has issued, then a snapshot and `InSync`. |

The three are three values and not one, because they are read differently.
`EPOCH_CHANGED` is a different store. `EVICTED` is a consumer that was
away too long, and is the daemon working as designed. `NEVER_ISSUED` is a
store that has gone backwards, which is the only one of the three that
means somebody should go and look at the daemon. Reporting it as eviction
would be wrong: nothing was discarded; nothing was ever there.

A `Resync` can also arrive in the middle of a stream, if the daemon's own
reader of the history loses its place or sees the epoch change under it.
What follows is the same: a snapshot, `InSync`, and the stream from there. A
consumer that treats a resync as fatal is a correct consumer; one that
silently carries on cannot exist, because it arrives as data that has to be
parsed to keep reading. A consumer that resyncs should discard what it
holds for its scope before applying the snapshot, which is exactly what the
`InSync` marker makes safe.

### The heartbeat

An idle stream is sent a `Heartbeat` every 15 seconds by default
(`--heartbeat-seconds`). It does two jobs, and the second is the one that
is not obvious. A consumer can tell "nothing has happened" from "my
connection is a black hole", which in a system whose failures are silent is
worth the bytes. And the position it carries lets a consumer advance its
resume point through a quiet period, so a stream idle for hours still
resumes rather than resyncing; without it, a quiet network is the case
most likely to fall out of the retained history.

**The position is the highest the daemon has delivered to that stream, not
the store's current position.** The store's latest can include an event the
stream has not been handed yet, and a consumer that resumed from it would
never be told about that event -- a silent gap produced by the very
mechanism meant to prevent one. Events outside the stream's scope count as
delivered: an event the stream was never going to be given is one it loses
nothing by resuming past.

### A reader that falls behind

The daemon holds a buffer of 1024 events for the streams it feeds. A
stream that falls behind it is **disconnected, not skipped**: its last event
is a `Disconnecting` with reason `LAGGED`, a sentence, and the position it was
delivered up to, and then the connection closes. Events carry authoritative
state, so dropping some leaves a consumer holding an address that has moved,
and a count of how many unspecified things were dropped gives it no way
back. Disconnection is the one failure every client already handles, and the
position makes recovery exact: reconnect, present it, and get a resume or a
named resync. When the daemon is stopping, the final event is
`Disconnecting` with `SHUTTING_DOWN`; a closed socket alone is
indistinguishable from a network that went away, and the two want different
behaviour from a consumer.

### Why a stream cannot miss an event at its join

One task per daemon reads the store's history once for every movement and
broadcasts it, and each stream filters that for its own scope; the point is
that a read of the history costs the store a thread, so a stream that polled
would cost one per poll per consumer. A new stream subscribes to the
broadcast **before** it takes its snapshot or catch-up read, and then
discards any broadcast event whose position it has already delivered. The
reverse order is the natural way to write it and it is wrong: subscribe
then read leaves an *overlap* (events in both), which comparing two positions
removes, while read then subscribe leaves a *gap* (events in neither), which
nothing removes and nothing reports.

### What the client library does and does not do

`teneral-api`'s client opens a connection, performs the handshake, makes
requests (`request`, and `request_collection` for a collection read through
to its marker) and turns a connection into a `Stream` whose `next` returns
events. **It does not reconnect.** A consumer keeps the last position it
was delivered, and reconnects and presents it itself; a refused watch
arrives from `next` as an error, and the final `Disconnecting` event is an
ordinary event followed by the connection closing.

## Minting an epoch, and restoring a backup

A cursor is an epoch and a sequence. The sequence is the position; the epoch
says whose sequence numbers these are, so a consumer presenting a remembered
position is told to carry on from it rather than handed a different history's
events under numbers it recognises. The epoch is generated when the database
is created, and it lives in the file.

That is why a restore is a problem the epoch cannot solve alone. A file-level
restore from a backup brings the epoch back unchanged, together with a
sequence counter wound back to when the backup was taken:

- A consumer whose position is *ahead* of the restore point presents a
  position the store never issued, and is caught by `NEVER_ISSUED`.
- A consumer whose position is *at or below* the restore point presents a
  position the restored store has issued, so it **resumes**, and is then served
  different events under numbers it has already seen. This is caught by nothing
  at all, and it is the failure mode this whole project is arranged against: a
  silently wrong answer.

`mint_epoch` is the remedy, and it is an operator's action rather than a
mechanism. It replaces the store's epoch and answers with the first cursor of
the new order, so every position from the old order fails to match and every
consumer is told `EPOCH_CHANGED`. It needs `admin` or the owning peer.
Minting *is* "everybody resync", expressed as data rather than as an
instruction nobody can enforce. The sequence counter does **not** restart, so a
position from the old order can never be issued again and is never confusable
with one from the new.

**After restoring a store from a backup:**

1. Start the daemon on the restored file while the consumers cannot reach it
   -- stopped, or kept off the socket -- because every consumer that connects
   before the next step is exactly the silent case above.
2. Mint a new epoch, as the owning peer or with an `admin` token:
   `teneral mint-epoch --socket <path>`.
3. Start the consumers. Each presents its remembered position, is told the
   epoch changed, and takes a snapshot.

**The daemon does not try to detect that it has been handed a restored file.**
This is a decision, not an omission. Every detector needs durable state
outside the database -- a remembered high-water mark -- and the restore that
rewinds the database is a restore of the data directory that remembered it, so
the detector comes back rewound too. A detector that fails exactly when it is
needed is worse than none, because it will be trusted. What exists instead:
the daemon logs the epoch, who minted it and when, the schema version and the
latest position at startup; `diagnostics` reports the same, including whether
the epoch was ever replaced and by whom; and `NEVER_ISSUED` is a distinct
answer that says a store has gone backwards. Reading the startup line after a
restore is the cheap check that does survive.

## The diagnostics

Beyond `digest` and `explain` above, the `diagnostics` operation (admin)
reports:

| Field | What it is |
|-------|------------|
| `schema_version`, `protocol_version` | Of the store's schema, and of this wire. |
| `epoch_minted_by`, `epoch_minted_at` | Who replaced the store's epoch and when, or the recorded word for an epoch nobody has replaced (with no instant). How an operator tells a store that has been minted since a restore from one that has not. |
| `live_views` and `max_live_views` | Two numbers rather than one because they count different things; see below. |
| `cross_network_answers` | **The counter that should always be zero.** |

The epoch and the current position arrive in the envelope the answer travels
in, as every answer's position does.

`cross_network_answers` counts answers the store refused for having come from
the wrong network. By construction it cannot move: the store checks the network
of every row it returns. That is the point -- a counter that is always zero in a
correct system is a better alarm than any dashboard, because one non-zero
reading is unambiguous and needs no baseline to interpret. If it is ever
non-zero, treat it as a defect in teneral, not as a misconfiguration. The one
thing that can move it from the wire is `lookup_binding_on_interface`, naming a
network that disagrees with the interface's owner: it moves for any caller, and
what the caller is *told* depends on its scope.

`max_live_views` is the bound this daemon enforces on the views it opened. A view
is a read transaction on a thread of its own for as long as it is held, so the
number alive at once wants bounding, and the number is a statement about how many
concurrent consumers there are supposed to be, which only this layer knows. A
request that would exceed it is refused as `TOO_MANY_VIEWS` with a retry hint of
one second. The bound is enforced in one place -- the one function in the crate
that opens a view -- so that no operation can open one without holding a permit
for as long as it lives. The watch's own reader waits for a permit rather than
being refused, since a reader that gave up would leave every consumer silently
behind; views are opened, used and dropped inside a single operation, so the wait
is short. `live_views` is the store's own count across everything that opened a
view in the process, and it is reported beside the bound: a bound enforced on one
number and reported on another is the arrangement that notices the day they
disagree, which will matter once something else shares the process.

## The daemon and its flags

`teneral serve` runs the daemon. There is no configuration file and there will
not be one; every number is a flag with a default, and `teneral serve --help`
is the reference.

| Flag | Default | What it trades |
|------|---------|----------------|
| `--store` | `/var/lib/teneral/store.sqlite` | The one piece of durable state. Its directory must exist: a daemon pointed at a directory that is not there has been given the wrong path, and silently creating a database at the mistaken one would hand every consumer a fresh epoch. |
| `--socket` | `/run/teneral/teneral.sock` | The one listener, mode `0660`. The group belongs to the deployment. |
| `--history-events` | 50000 | The operative bound on retained history. Raising it lets a consumer be away longer before it must resync, and costs disk. At ten events a second, 50000 resumes after about eighty minutes away. |
| `--history-age` | 21600 (six hours) | A ceiling on how old retained history may get, in seconds. It is a ceiling and not a window: the count above is the operative bound. Tighter than the store's own default of a week, so that a quiet store trims its log and a quiet deployment reaches the resync path at all instead of leaving it exercised only by the test suite. Evicting history a consumer could have resumed from is a cost rather than a goal; raise it where consumers are routinely away for hours. |
| `--max-live-views` | 64 | How many concurrent reads. Each costs the store a thread. Lower protects the machine; higher serves more simultaneous consumers. |
| `--max-connections` | 256 | A connection is a task, so this is what stops an accept loop becoming unbounded memory. A daemon at the bound stops taking connections off the queue rather than accepting and idling them. |
| `--heartbeat-seconds` | 15 | How soon a consumer notices a dead connection, against how much a quiet stream chatters. |

All numeric flags must be at least 1. The daemon stops cleanly on `SIGINT` and
`SIGTERM`: it stops accepting, tells every open stream it is stopping, waits up
to five seconds for connections to finish with the store, and then closes the
store. It logs one line per message to standard error and uses no logging
framework; capturing and filtering that is left to whatever runs the daemon.
There is also **no sweeper**: nothing in the daemon removes an expired lease, and
`list_leases` includes expired ones and says so.

The client subcommands -- `get`, `explain`, `diagnostics`, `mint-token`,
`mint-epoch` and `watch` -- are each one call over the client and a printing of
the answer, and hold no logic the server should hold. A token's secret is read
from the first line of standard input with `--token-stdin`, or from the
`TENERAL_TOKEN` environment variable; with neither, the connection is made as the
owning peer. There is deliberately no flag that takes the secret itself, because
an argument is visible to every user on the machine in the process table.

## What is not here

- **Zones, static records and projected records**, and **content blobs**. The
  store does not hold them. Reading a blob will be a capability of its own,
  because a blob holds credentials, and because capabilities are rows of text
  rather than a schema, adding one is an insert and not a migration.
- **Mapping an authenticated human to a scope.** Tokens, their scopes and who may
  mint one are here. Which networks an identity arriving from an authenticating
  proxy may touch is not decided yet.
- **A TCP listener, TLS and anything a browser touches directly.** Something has
  to translate for a browser, and it is the home IPAM's own binary, in its own
  repository, linking this crate's client.
- **Metrics, rate limits and quotas.**
- **Writing a lease.** The lease-writing store methods have no caller on this
  wire; leases are written by the front-ends that answer clients.
