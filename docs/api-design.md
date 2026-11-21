# The control API

**Nothing here is built.** This note settles the shape of the
control plane and recommends one transport, so that the binding
store and the front-ends can be written against a surface whose
hard questions have already been asked. It fixes the resource
model, the identity of every resource, the watch primitive's
behaviour across a reconnect, and the transport. It deliberately
does not fix a wire format, a field numbering or a complete list
of operations; those belong to the phase that writes them.

## The short version

- **One transport: HTTP with JSON bodies, served on a Unix
  domain socket, with watch delivered as a long-lived
  event-stream response.** No TCP listener in the daemon.
- The sentence that decides it: *a browser does not need TCP, it
  needs HTTP -- an ordinary reverse proxy bridges a Unix socket
  to a browser for nothing, but only if what is on the socket is
  HTTP.*
- This keeps the Unix socket the project already intended and
  discards the encoding it intended to put on it. Protobuf on
  that socket is what forces a translating proxy between the
  daemon and the one consumer whose user interface is a browser,
  and a translating proxy is a second implementation of the
  rules that keep one network's answers out of another's.
- The watch primitive delivers a snapshot to a new client, a
  resume to a reconnecting client whose cursor is still held,
  and an explicit, in-band `resync` to one whose cursor is not.
  It never silently starts streaming from the present, and it
  never silently drops events: a client that cannot keep up is
  disconnected so that its cursor still means something.

## Prior art, read as an interface

Technitium DNS Server is the closest existing thing:
authoritative and recursive DNS, multi-network DHCP, and a web
console driven by the same documented HTTP API an operator can
drive. It was studied here as an interface and not used as a
source -- it is GPL-3.0 and this project is Apache-2.0, so no
sentence, table, operation name or parameter name of its
documentation has been reproduced or paraphrased closely, and
the only thing taken from it is the list of questions a combined
DNS and DHCP surface has to answer.

Four of those questions it answers differently from this note,
and the disagreements are worth recording because they are where
teneral's premises bite:

- **It has no event mechanism.** Every consumer polls, and the
  only push-shaped thing is pull-based metrics scraping. Not
  polling is most of why this API exists, so the watch primitive
  below is designed rather than inherited.
- **Its DHCP is a subsystem beside its DNS**, so a lease
  creating a DNS record is a switch on the DHCP side. Here a
  name is a *fact about a binding* rather than a record some
  other subsystem writes, so the forward and reverse records for
  a client are a projection of the store and not separately
  created objects.
- **It has no tenancy.** A lease is identified inside one global
  address space. That is the one thing this project cannot
  borrow at any price: overlapping address space is the premise,
  not an edge case.
- **Its leases are listed globally and filtered by the caller.**
  Asking "what address does this client hold" means enumerating
  and searching. The whole reason Shaken Fist reads a file today
  is that nothing will answer that question directly, so here it
  is a direct lookup.

## What the consumers actually need

### The home IPAM, whose interface is a browser

This is the first consumer and the demanding one. It wants to
read and write everything: networks, ranges, reservations,
leases, names, upstream forwarders. It wants to show a live
list, so it wants a push channel rather than a poll loop. Its
user interface lives in its own repository and consumes this API
like any other client, which is what stops it acquiring a
privileged path into the store.

Concretely, a browser needs:

- a listener it can reach, which cannot be a Unix socket
  directly;
- TLS, because a bearer credential on a home LAN otherwise
  travels in clear text;
- same-origin service, or a configured cross-origin allowance
  that fails closed when unset;
- a push channel the platform implements natively, so that
  reconnect and backoff are not hand-written in the UI;
- a credential the browser can hold without exposing it to
  every script on the page.

Those five are the whole of the argument against a Unix socket,
and all five are satisfied by the recommendation below without
the daemon owning a TCP port, a certificate or a session
concept.

### Shaken Fist, which does something subtler than it looks

The existing consumer is worth reading carefully, because the
obvious summary of it is wrong.

`DnsMasq` writes a dnsmasq configuration whose DHCP range is in
`static` mode, and writes every instance's address into a hosts
file from its own IPAM
(`shakenfist/managed_executables/dnsmasq.py`,
`_enumerate_leases` and the `dhcphosts.tmpl` template). So it
does not learn an address from DHCP: it *chooses* the address
and tells the server, which is then only allowed to hand out
what it was given.
Each network's server runs against the inner end of a veth pair
inside that network's own namespace
(`shakenfist/network/bridged_vxlan_network.py`, which moves the
inner veth into the namespace and gives it the router address),
and DNS is usually switched off entirely.

The lease file is still read, and this is the part that matters:
`_remove_invalid_leases` walks it looking for an unexpired lease
held by a MAC that is no longer authorised, and if it finds one
it kills the server, deletes the lease file and starts again.
That is not discovery. That is **reconciliation** -- a
consistency check against a view of the world that the consumer
cannot query, implemented by parsing a file the server happens
to write, with a process kill as the only available repair.

Two more things follow from the same gap. `is_serving_dhcp()`
has to send a real DHCPINFORM into the namespace to find out
whether the server is answering, because liveness is not a
question that can be asked. And `remove_lease()` shells out to a
helper that sends a release packet, because dropping a lease is
not an operation either.

So the thing being designed away is broader than one file read.
It is the absence of any way to ask the server what it believes,
to be told when that changes, or to correct it without killing
it. Four capabilities replace all of it: assert a binding and be
told whether the assertion conflicted, revoke a lease, compare
the server's view with yours cheaply, and be told when it
changes. The watch primitive is one of the four and the other
three are why it is not sufficient on its own.

## The rules the shape follows

Five rules, each of which exists because of a specific way this
project fails.

**Nothing is addressable without naming its network.** Two
networks may legitimately hold the same address and the same
name, so an address, a MAC or a hostname is not an identity --
it is an identity's second half. A lease, a binding or a record
that can be reached by a URL with no network in it is a
cross-network bug with a stable link to it. The network comes
first in every path and in every event.

**A wildcard is typed, never defaulted.** Omitting a scope is an
error, not a request for everything. A content blob that applies
to every client in a network says so in a word the caller had to
write; a watch that covers every network says so the same way.
The reason is that the commonest way to widen a scope by
accident is to leave a field empty.

**Every answer says what it was true of, and when.** A response
repeats the network in its body rather than relying on the
request, so a consumer can detect that it was answered from the
wrong scope. Every response also carries the store cursor at
which it was read, so a consumer holding streamed state can tell
a stale read from a fresh one and refuse to apply it backwards.

**Derived state is not writable.** Leases and the records
projected from bindings are produced by the daemon. A client may
read them, watch them and revoke a lease, but it cannot invent
one. This is what keeps the store the single source of truth
rather than one of two.

**A conflict is reported, never resolved.** Two writers
asserting the same address in one network, an interface claimed
by a second network, a binding whose address falls outside every
range in its network: each is refused with the conflicting party
named. Nothing is silently merged, because the whole failure
mode here is a silently wrong answer.

## The resource model

| Resource | Identity | Written by |
|----------|----------|------------|
| Network | a caller-supplied name, unique in the daemon | operator |
| Attachment | `(network, namespace, interface name)` | operator |
| Address range | `(network, range name)` | operator |
| Selector | not a resource: a `(kind, value)` pair, meaningful only inside a network | -- |
| Binding | `(network, selector)` | operator |
| Lease | `(network, address)`, carrying a lease id that changes whenever the address changes holder | the DHCP front-end; an operator may revoke |
| Zone | `(network, suffix)` | operator |
| Static record | `(network, name, type, rdata)` | operator |
| Projected record | `(network, name, type)` -- a view of a binding or a lease | nobody |
| Content blob | `(network, selector or the word for "any client", key)` | operator |
| Token | a token id, daemon-global, carrying the networks it may touch | operator |

A few of those need the reasoning spelled out.

**A network's identity is a name the caller chose.** Not the
subnet, which two networks may share; not the interface, which
can be renamed; not an identifier the daemon invents, because
Shaken Fist already has a stable identifier for the thing and a
second one means a mapping table that will eventually disagree
with itself. The daemon treats the name as opaque, so a UUID and
the word `guest` are equally valid. A network is also not a
subnet: it owns zero or more address ranges as child resources,
which is how a static-only network (Shaken Fist's case: no
range, so nothing is ever allocated, only matched) and a home
network with a dynamic pool are the same resource with different
children rather than two modes.

**An attachment carries its namespace in its identity.** An
interface name is unique only within a network namespace, and
both of the namespace-using consumers put every network in one
of its own. Identity without the namespace would collide the
moment a second consumer named its interfaces by role rather
than by network. The other half of this resource is a
constraint rather than a field: **an interface belongs to
exactly one network, and claiming an attached interface for a
second network is refused.** That refusal is the single most
important validation in the API, because it is the one place
where a cross-network leak would be *configured* rather than
coded, and it is the invariant that lets the datapath treat a
packet's arrival interface as a complete answer to which network
it belongs to.

**A selector is not independently addressable.** Client identity
is the union of what a client presents -- MAC, client-id,
DUID/IAID, user-class, vendor-class -- and a selector names one
of those and its value. The same MAC in two networks is two
unrelated things, legitimately, so a selector has no meaning
outside a network and no URL of its own. A binding is reached as
a selector *under* a network, which makes the nesting carry the
scoping: roughly
`/networks/{network}/bindings/mac/{value}`.

**A lease is keyed by address, not by client.** Within one
network an address has at most one holder at a time, which makes
it a true key; a client may briefly hold two addresses during a
renumber, which makes the client not one. Because an address
outlives any particular lease on it, each lease also carries an
opaque lease id that changes whenever the address changes
holder, so a consumer can tell a renewal from a reassignment
without comparing timestamps. Looking a lease up by client is a
query within a network rather than a path, and it is a first
class query rather than a client-side filter over a list --
answering "what does this client hold" is the thing the existing
consumer cannot do today.

**Precedence between selectors is the API's, documented, and
not per-network.** When a client presents several attributes and
several bindings could match, the order of preference is fixed
by this API, published, and returned alongside every lookup
result. It is not configurable per network, because a
per-network precedence makes it impossible to reason about what
two networks will do with the same client. No RFC specifies such
an order, so this is teneral's own choice rather than anything
observed elsewhere, and the API's documentation is its only
source -- which is exactly why a consumer must be told which
selector matched rather than inferring it.

**Content blobs are scoped tightest.** A blob is keyed by
network, by selector or the explicit word for "any client in
this network", and by a path-like key. The eventual metadata
front-end serves credentials from these, so a cross-network read
here is a credential leak rather than a wrong address, and read
access to blobs is a capability distinct from read access to
bindings. A user interface that lists leases has no business
reading cloud-init user data.

**Names are facts about bindings.** A binding or lease with a
hostname produces the forward and reverse records for it in its
network's zone; those records are projections and cannot be
written. Names that are not a client -- the router's own A
record, an alias to something off-network -- are static records,
whose identity includes the record data because one name
legitimately holds several. There is no global name lookup in
this API: asking what `printer.lan` resolves to without naming a
network is an error rather than a search, because the honest
answer is "in which network", and a front-end answers from the
zone of the network the query arrived on.

## How a consumer learns an address

Three paths, each with a different job, and a consumer that uses
the wrong one builds a race.

**Assert, and be told.** The first path is the one the existing
consumer actually wants and cannot have. Write a binding with an
explicit address; the response either confirms it or reports a
conflict naming the lease and the holder that stands in the way,
and the write can carry an explicit instruction to revoke that
lease as part of the same operation. Those two together --
conflict reported, revoke available atomically -- remove the
kill-the-server-and-delete-the-lease-file repair entirely,
because the condition it repairs is now refused at the point of
writing instead of discovered later by parsing a file.

**Ask.** The second path is a read: within a network, the lease
held by this selector, or the binding that applies to it. This
is what a consumer that allocates addresses elsewhere uses to
confirm that the daemon agrees with it, and it is what a user
interface uses to fill in a page. Reads carry the cursor they
were taken at, so a consumer that is also watching can order
them against its stream.

**Be told.** The third path is the watch stream, which carries
every lease and binding transition in a watched scope. This is
the one a consumer uses when the address is chosen by the daemon
rather than by the consumer -- the home case, where a client
appears and gets a pool address -- and it is the one that
replaces polling a file to notice that something changed.

The distinction worth keeping in mind is that the existing
consumer's problem was never *discovery*. It knew the address;
it had no way to confirm that the server agreed, no way to hear
when the server's view diverged, and no way to correct it
without a restart. Watch alone would not have fixed that, which
is why the first path above exists and why the digest in the
detection section does too.

## The watch primitive

Everything about this is about what happens when a client has
been away, because that is where a push API quietly becomes
wrong. A consumer that resumes a stream and silently misses the
event in which an address changed holds a wrong answer forever,
and nothing will tell it.

### Shape

A watch is a long-lived response on an ordinary request. The
request names:

- **The scope**, which is one or more networks, or the explicit
  word for every network. A watch with no scope is an error.
- **The resource kinds** wanted: leases, bindings, records,
  attachments, blobs. Unknown kinds are rejected rather than
  ignored, because a client that asked for something the server
  does not know is a version mismatch worth hearing about.
- **A cursor**, optionally, from a previous connection.

Every event carries its network, always, including on a
single-network stream. A handler that learned to infer the
network from the subscription is a handler that will be wrong
the day someone widens the subscription, and the cost of
repeating it is a few bytes.

Every event carries **the full post-state of one resource**
rather than a delta, together with its identity and its cursor.
A deletion is the same shape with the state marked absent. This
makes every event idempotent to apply, which is what makes
at-least-once delivery sufficient; exactly-once delivery is not
on offer from anything, and designing for it is how a system
acquires a bug it cannot reproduce.

### The cursor

A cursor is a pair: an **epoch** and a **sequence**. The
sequence is a monotonically increasing number assigned to every
mutation of the store, in one total order. The epoch changes
whenever the sequence cannot be trusted to continue -- the store
was restored from a backup, the event history was truncated past
what a client might hold, or the daemon's identity of the store
changed. An epoch is what stops a restored daemon from handing
out sequence numbers a client has already seen attached to
different events.

Order is total across the whole store, but **a consumer must
not read causality between networks from it**. Two networks'
events are interleaved by the cursor and nothing more; the
cursor exists to resume a stream, not to prove that something in
network A happened because of something in network B. Within one
network, order is meaningful and is the order the store applied
the changes in.

### A new client: snapshot, then stream

A watch presented without a cursor gets the current state of its
scope as a sequence of events, one per resource, marked as
existing rather than as having just changed. The snapshot is
read at a single cursor -- it is a consistent view, not a walk
that races mutations -- and it ends with an explicit **in-sync
marker carrying that cursor**. The live stream resumes at
exactly that cursor, so there is no gap to lose an event in and
no overlap to deduplicate.

The in-sync marker is not a nicety. Without it, a consumer
cannot tell "I have seen the whole world" from "I have seen some
of it", so it cannot safely do the one thing it most wants to do
after connecting, which is to delete local state for resources
the server did not mention.

### A returning client: resume, or an explicit resync

A watch presented with a cursor gets one of two things, and the
server says which.

**Resume.** If the epoch matches and the sequence is still
inside the retained history, the server sends every event after
that cursor, in order, and then an in-sync marker. The
consumer's state is continuous and no snapshot is needed. Events
that occurred while it was away are delivered as they occurred,
including intermediate states: a lease that was created and then
released while the client was absent produces both events, not
neither. Collapsing them would be cheaper and would make the
stream unusable as a sequence, because a consumer that was
holding the earlier state would never see it end.

**Resync.** If the epoch does not match, or the sequence has
been evicted from the retained history, the server does **not**
start streaming from the present. It sends a `resync` event
naming the reason -- the epoch changed, or the cursor was
evicted -- and then behaves exactly like a new client: snapshot,
in-sync marker, stream.

The resync is sent **in band, as data on the stream**, not as a
connection error or an HTTP status. This is deliberate. A
connection error is something every client retries blindly,
which is precisely the behaviour that would turn a gap into
permanent divergence. A `resync` event is something a client
must parse to continue reading its stream, so the only client
that can ignore it is one that has stopped reading. A client
that treats `resync` as fatal is a correct client; one that
silently continues cannot exist.

### Retention, and why resync is unavoidable

The retained history is bounded, by a count of events and by
age, and the count is the operative bound because the event rate
between a home network and a hypervisor fleet differs by orders
of magnitude. It is sized so that a consumer restart measured in
seconds and a partition measured in minutes both resume, and a
consumer that has been down since yesterday resyncs.

Retention is **per daemon, not per client**. The server does not
hold a queue for an absent client, because an absent client that
never returns would pin that queue forever, and because a
per-client queue makes the server's memory a function of how
many consumers have ever connected. This is the choice that
makes resync a normal event rather than an error path, and it is
why resync is specified in as much detail as resume.

The consequence has to be said plainly: **the watch stream is
not an audit log.** It is a mechanism for a consumer to converge
on the store's current state, and over a long enough absence it
converges by replacement rather than by replay. Anything that
needs every transition -- accounting, billing, forensics --
consumes it live, records each `resync` as a gap it cannot fill,
and does not pretend otherwise.

### Slow readers

A consumer that falls behind the per-connection buffer is
**disconnected**, with a final event saying so, and resumes from
its cursor.

This is a deliberate departure from how the nearest thing in
this organisation behaves: `ryll`'s control socket keeps a
bounded event queue, drops the oldest events when it fills, and
reports a running count of what it dropped
(`docs/control-socket-protocol.md` there). That is the right
trade for a test harness whose events are diagnostics and whose
session must stay responsive. It is the wrong trade here,
because these events carry authoritative state: a dropped lease
event leaves a consumer holding an address that moved, and a
counter telling it that some unspecified number of unspecified
things were dropped gives it no way back. Disconnection is
kinder, because disconnection is the one failure every client
already handles, and because the cursor makes the recovery
exact.

### Heartbeat

An idle stream emits a periodic keepalive carrying the current
cursor. Two reasons, and the second is the non-obvious one.
First, a consumer can tell "nothing has happened" from "my
connection is a black hole", which in a system whose failures
are silent is worth the bytes. Second, the cursor in a heartbeat
lets a client advance its resume point during a quiet period, so
a consumer that has been idle for hours still resumes rather
than resyncing -- without it, a quiet network is the case most
likely to fall out of the retention window.

## Detecting a wrong answer

The stated failure mode of this project is a silently wrong
answer, so a client needs mechanisms for noticing that it was
given one. Six, in rough order of how much they are worth.

**The explain query.** Given a synthetic client identity and an
attachment, the API answers what it *would* reply, without a
packet being sent: the binding that would match, which selector
matched it, the address and options that would be returned, and
the network it was all resolved in. This is the single most
valuable diagnostic the API can offer, because it converts the
project's whole failure class from something observed hours
later in a guest into a question an operator or a test can ask
directly. It is cheap, because the front-ends need that lookup
anyway -- it is the same code path with the packet replaced by
its parameters.

**Every answer names its scope and its cursor.** A read of a
lease returns the network in the body. A consumer that asked
about one network and is handed a body naming another has caught
a bug in the API, in its own client, or in something between
them -- and the cache keyed by address rather than by
`(network, address)` is exactly the bug this catches. The cursor
on the same response lets a consumer that is also watching
refuse to apply a read that is older than its streamed state,
which is the race that otherwise reverts a correct view to a
stale one.

**Every lookup says which selector matched.** An operator who
configured a MAC reservation and is told that a vendor-class
binding matched has found a precedence problem before a user
does. Since the precedence is this API's own invention rather
than anything a standard fixes, reporting it is the only way a
consumer can check that its understanding matches the daemon's.

**A per-network digest of the configured set.** Each network
exposes a cheap value that changes when its bindings change. A
consumer holding a mirror compares one value instead of diffing
everything, and a mismatch means "resync", not "investigate".
This is the direct replacement for walking a lease file looking
for entries that should not be there: the question becomes "do
we agree", asked in one call, rather than "let me re-derive what
you should have done".

**Counters labelled by network.** Every metric the daemon
exports carries the network it belongs to. A global count of
leases granted cannot show one network answering out of
another's pool; the same count per network can.

**One counter that should always be zero.** Specifically: the
number of requests whose arrival attachment resolved to one
network while the matched binding belonged to another. By
construction that cannot happen, which is the point -- a counter
that is always zero in a correct system is a better alarm than
any dashboard, because a single non-zero reading is unambiguous
and needs no baseline to interpret.

## Authentication

**On the Unix socket, the peer is the credential.** The socket's
file mode is the gate, and the connecting process's user and
group, read from the kernel rather than claimed by the client,
are the identity. The local case therefore needs no secret at
all, which matters more here than usual: there is no
configuration file in this project, so there is nowhere to put a
secret that the daemon would have to read at startup.

**Beyond the socket, bearer tokens.** A token is an id, a secret
shown once at creation and never again, and a scope. The scope
is a list of networks -- or the explicit word for all of them --
plus a capability set: read bindings, write bindings, watch,
read content blobs. Blob reading is its own capability for the
reason given above: those hold credentials. The normal token for
a user interface is read-plus-write on the networks it manages,
with no blob access; the normal token for a consumer like a
hypervisor fleet is full access to the networks it owns and none
to anything else.

**There are no user accounts, no passwords and no sessions.**
The daemon is not an identity provider. Its user interface has
its own users, in its own repository, and holds one teneral
token on their behalf. Putting accounts here would make the user
interface's privilege model teneral's problem, which is the
mechanism by which a user interface ends up with a privileged
path into the store -- the failure that keeping it in a separate
repository exists to prevent.

**Bootstrap has no secret on disk.** The first token is minted
over the Unix socket by a peer the socket's permissions already
trust. There is no initial password, no token file and no
first-run secret to leak, because the only path that needs no
token is the one that filesystem permissions already gate.

**Every write records the token that made it**, kept with the
resource. "Who moved this binding, and when" has to be
answerable, because the consequences here are discovered hours
after the cause.

## The transport

### The case for protobuf on a Unix socket

This was the project's intended default, and it is a reasonable
one. The control plane is local: the daemon runs on the host
whose networks it serves, and so does the consumer. A schema
file gives both ends generated types and makes the resource
model a reviewable artifact rather than a documentation
convention. Streaming is a solved problem rather than a thing to
design. And most of all, the daemon is the most privileged
process on the box -- it holds `CAP_SYS_ADMIN` to move between
namespaces, `CAP_NET_RAW` to answer a client that cannot yet
answer ARP, and every network's bindings in one address space --
so a transport whose entire access control is a file mode has a
genuinely smaller attack surface than one with a listening port
and a credential to verify.

The precedent is weaker than it looks, though, and it is worth
being accurate about it. The two things in this organisation
this design would be "matching" do not agree with each other.
Shaken Fist's privileged executor is raw protobuf over a Unix
stream socket, one request per connection, with no length
framing and no server-initiated anything
(`shakenfist/util/concurrency.py`) -- so it is a precedent for a
socket and an encoding, but not for a stream, and the stream is
the part that is expensive to get right. `ryll`'s control socket
is newline-delimited JSON over a Unix stream socket, with
subscribe, unsubscribe, bounded queues and a drop report; it is
the organisation's only real precedent for a watch primitive,
and it chose JSON. The shared convention is the Unix socket. The
encoding is not a convention, and whichever is chosen here, the
streaming half is new work.

### The case against it, which is decisive

A browser cannot open a Unix socket, and it cannot speak gRPC
without a translating layer. Both of those are usually stated
together as one objection, and separating them is what settles
this.

The first is not a real problem. A reverse proxy -- which this
deployment needs anyway, to terminate TLS and to serve the user
interface and the API from one origin -- can forward to a Unix
socket as readily as to a port. Such a proxy moves bytes: it
does not parse a lease, does not know what a network is, and
therefore cannot drop a scope it never looked at.

The second is a real problem, and it is the whole decision. A
proxy that bridges a browser to a protobuf or gRPC control plane
cannot forward bytes; it must translate. Translating means it
holds a model of the resources, the scopes and the stream, which
means the rules that keep one network's answers out of another's
exist in two places written by two people at two times. In a
project whose central correctness property is cross-network
isolation and whose failures are silent, a second implementation
of the scoping is not a cost to weigh against convenience. It is
the defect this project exists to avoid, installed in the one
component nobody would think to test with two networks.

So the objection is not that a browser needs TCP. It is that a
browser needs HTTP, and that HTTP is the one thing that makes
the proxy a byte mover instead of a second implementation.

### The recommendation

**HTTP with JSON bodies, served on a Unix domain socket, with
watch delivered as a long-lived event-stream response. One
listener, one implementation, no TCP port in the daemon.**

This changes the encoding the project intended and keeps the
socket it intended -- and that is worth saying plainly rather
than softening, because it reads at first like a reversal and is
mostly a correction. The reasoning for the socket survives
intact: local only, file permissions as the gate, no certificate
and no listen address for the daemon to own. What does not
survive is protobuf on it, for the single reason above.

Three consequences are worth naming, since they are the parts
that make this recommendation cheap rather than merely
defensible:

- **The browser's reconnect comes for free, and it is exactly
  the mechanism the design needs.** A browser's native
  event-source client reconnects on its own and replays the last
  event id it saw. That id is the cursor. The resume semantics
  specified above are therefore what the first consumer's
  platform already implements, rather than something its author
  has to be persuaded to implement correctly.
- **One watch implementation serves both consumers.** A
  hypervisor fleet's client passes the same cursor explicitly on
  the same endpoint. There is no second streaming path to keep
  in step, which matters because the two consumers' streams are
  the thing most likely to drift.
- **The encoding stays replaceable.** HTTP negotiates content
  types. If JSON framing is ever measured to be the bottleneck,
  a binary encoding arrives on the same endpoints with the same
  semantics -- a changed header, not a changed architecture.

For completeness, the two alternatives considered and not
recommended. A WebSocket for watch: it works, but it has no
resume story of its own, so the cursor machinery would have to
be hand-written on both sides, and it buys bidirectionality that
a watch does not use. A long poll: it needs the same cursor
bookkeeping as the stream and loses the browser's built-in
reconnect, so it is the same work with less of it done for you.

### What would change the recommendation

Four things, each stated so that it can be checked rather than
argued:

1. **The user interface turns out to have no server-side half.**
   If the IPAM front end is to be a static bundle talking to the
   API directly, there is no reverse proxy to bridge the socket,
   and the daemon needs its own port, its own TLS and its own
   cross-origin policy. That is a decision in the interface's
   repository, it reverses the cheapest part of this
   recommendation, and it should be settled before the first
   real deployment rather than discovered during it. It is the
   one question in this note that someone else answers.
2. **The stream's cost is measured and JSON is the reason.** Not
   a feeling about parsing overhead: a number. If one consumer
   mirroring a whole store costs more than a few percent of a
   core at the event rate a hypervisor fleet actually produces,
   the encoding is worth revisiting -- and the revisit is
   content negotiation on these endpoints, not a different
   transport.
3. **A consumer appears that needs the API from another host and
   cannot have a proxy on this one.** Then the daemon needs a
   port. Note this is an argument for a second listener on the
   same HTTP implementation, not for a second protocol.
4. **Event streams prove unreliable through the chosen proxy.**
   Response buffering in a reverse proxy is a well-known way to
   break a long-lived stream, and it is configuration rather
   than protocol. If it proves fragile in practice, the fallback
   is a chunked newline-delimited response on the same endpoint
   with the same events. That is worth noting because it means
   none of the semantics above depend on the stream format.
   The browser's automatic replay of the last event id is a
   convenience; the cursor is the load-bearing part.

## What this asks of the storage layer

The store is planned as SQLite behind a trait, with the
acknowledged risk that a trait designed against one backend
usually fits only that backend. The API shape above makes five
demands, and a trait written without them will have to be
reopened when a second backend is tried -- which is the phase
where overlapping address space and namespaces arrive, and the
worst time to be changing the store's interface.

1. **A total order over mutations, assigned by the store.** The
   cursor is not something the API layer can invent on top of a
   CRUD interface: with more than one writer there is no
   consistent order to invent. The trait's write operations
   return the cursor they were assigned, and reading the history
   after a cursor is a trait operation rather than a query built
   above one.
2. **Reads consistent with a point in that order.** The
   snapshot-then-stream contract needs a view at a cursor.
   SQLite gives this for nothing with a read transaction, which
   is exactly why a trait designed only against SQLite will
   forget to express it. It needs to be a read handle that can
   be held and that reports its cursor, not a collection of
   independent getters that happen to agree most of the time.
3. **Compare-and-set on identity.** "Create this binding only
   if no lease holds the address" and "revoke that lease and
   create this binding" are single operations, because expressed
   as a read followed by a write they are correct only on a
   single-writer store. They are also the two operations the
   existing consumer most needs, so they are not hypothetical.
4. **The network is a parameter of every trait method.** Not a
   column that implementations are trusted to filter on. This is
   both the guard against the cross-network bug at the lowest
   layer and the hook a second backend would shard on; a trait
   whose methods take an address and whose implementation
   happens to scope by network is the shape that breaks, and it
   passes every single-network test.
5. **History retention belongs to the store.** The event window
   is durable and is truncated durably, so its bound and its
   eviction are the store's to enforce. An API-layer ring buffer
   loses its history across a restart, which turns every daemon
   restart into a resync for every consumer and wastes the
   resume path entirely.

## Left open, deliberately

- **The wire details.** Field names, the exact event grammar,
  pagination of large collections, and the operation list. This
  note fixes identities and semantics; a specification that
  fixes bytes should be written against a store that exists.
- **IPv6 resources.** DUID and IAID appear in the selector
  model, because leaving them out would shape the identity
  model wrongly, but v6 ranges, prefix delegation and router
  advertisement are not designed here.
- **Multi-host deployment.** One daemon, one store, one host.
  The API would not change shape for a second host, but nothing
  above has been checked against that claim.
- **Rate limits and quotas on the control plane.** The data
  plane's input is hostile and unauthenticated; the control
  plane's is authenticated and local, so this is deferred rather
  than decided.

## Related documents

- [ARCHITECTURE.md](../ARCHITECTURE.md) -- the layers this API
  is the control plane for.
- [provenance.md](provenance.md) -- why behaviour is cited to an
  RFC or to a measurement, which is why the selector precedence
  above is documented as this project's own choice.
- [plans/](plans/) -- what is being built, in what order.
