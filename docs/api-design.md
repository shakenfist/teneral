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

**Read the transport bullet below with the ruling at the end of
this section in hand: the operator overruled it on 2026-11-25,
and the project builds protobuf on the socket instead.** The rest
of this note -- the resource model, every identity, the watch
primitive's whole contract, the authentication model and the six
diagnostics -- is unaffected and is what phase 4 implements.

- **One transport: HTTP with JSON bodies, served on a Unix
  domain socket, with watch delivered as a long-lived
  event-stream response.** No TCP listener in the daemon.
  **Overruled; see below.**
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

**The transport ruling, 2026-11-25.** The operator ruled for the
master plan's standing default -- protobuf messages over a Unix
domain socket -- and against the recommendation this note makes
below. Open question 1 is closed by that ruling. The argument
below is left standing rather than rewritten, because a note that
quietly agreed with whatever was decided would be worth nothing
the next time a transport is argued about, and because what the
project now owes is only legible against what the note predicted.
Three things follow, and
[the control API phase plan](plans/PLAN-teneral-implementation-phase-04-control-api.md)
carries each of them as work rather than as a caveat: something
has to translate for the browser, and it is the home IPAM's own
binary in its own repository, linking teneral's client rather
than reimplementing its model; the browser's free reconnect is
gone, so the cursor bookkeeping is written once in that client
crate instead of in every consumer; and the store's
engine-built JSON event state has to be parsed and re-encoded as
a typed message, which is a cost the HTTP form would not have
paid. One thing the ruling buys, which this note never disputed:
the resource model becomes a schema file both ends are generated
from rather than a documentation convention.

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
user interface is served by the daemon itself, from the shared
`sfui` design system vendored into its static assets, and it
consumes this API like any other client -- which is what stops it
acquiring a privileged path into the store.

That last clause is the whole of why serving the pages here is
safe, so it is worth stating as a rule rather than an intention:
**the web interface gets no API surface of its own.** Every byte
it displays arrives over the endpoints described below, with the
same network scoping and the same authorisation as any other
client's. A handler that exists only for the UI, or that skips a
scope check because "it is our own page", is the failure this
whole design is arranged to prevent, and it would be invisible in
a single-network test.

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
asserting the same address in one network, or an interface
claimed by a second network: each is refused with the
conflicting party named. Nothing is silently merged, because the
whole failure mode here is a silently wrong answer.

This rule used to name a third conflict -- a binding whose
address falls outside every range in its network -- and that was
wrong, so it is struck rather than quietly left out. A
static-only network has no range at all, which this document
describes a few paragraphs below as Shaken Fist's ordinary case,
and under the struck rule every binding in such a network would
have been refused. Even where a network does have a pool, a
reservation outside it is the usual way to hold an address the
pool can never hand out. Range containment is therefore not a
conflict, and the decidable conflict in this area is a binding
against an address a lease currently holds.

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
than by network.

Note that this is the only row in the table above where the
identity and the uniqueness key are not the same thing. The
network is part of how an attachment is *addressed*, because
every resource is reached under the network that owns it, but
what makes an attachment unique is narrower: the namespace and
the interface name alone, with the network an ordinary attribute
of the row. That is not a looseness in the table. It is the
constraint below expressed as a key, and the store implements it
that way deliberately, because a uniqueness key that included
the network would permit precisely the configuration the next
paragraph forbids. The other half of this resource is therefore
a constraint rather than a field: **an interface belongs to
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
two networks will do with the same client. Most of the order is
teneral's own choice rather than anything specified or observed
elsewhere, which is exactly why a consumer must be told which
selector matched rather than inferring it -- but not all of it
is, and the paragraphs below say which step is specified and
where. This sentence used to claim no RFC fixed any of it, which
was wrong; RFC 2131 section 4.2 settles the top pair.

The order, highest first, is the client identifier, then the
DUID and IAID, then the hardware address, then the user class,
then the vendor class. One principle generates it: an attribute
that names a single client outranks one that names a class of
clients; among those that name a single client, the identity the
client asserts *as a client* outranks the identity of the
hardware it happens to be in today; and among those that name a
class, the class the operator's own deployment sets outranks the
class the vendor shipped.

Taking the five in turn. The **client identifier** exists only
because somebody set it, and the reason the protocol carries it
at all is that a client's identity should survive the
replacement of its network card -- so it is both the most
deliberate and the most stable thing a v4 client can present.
The **DUID and IAID** are the same statement in v6: one client,
and one address association on it. It sits immediately below the
client identifier rather than beside it because the two cannot
collide in one request -- a v4 request carries no DUID and a v6
request no hardware address -- so the order between them is a
formality, fixed anyway because a published precedence with a
gap in it is one every consumer has to guess at. The **hardware
address** names an interface rather than a client: it changes
when a card is replaced and it is duplicated the moment a
virtual machine template is cloned, which makes it the weakest
of the three individual identities, and it is nonetheless the
one nearly every reservation is written against because it is
the one every client presents without being asked. The **user
class** is a label the client was configured with, naming a role
that many clients share. The **vendor class** names a product or
a firmware family that every device of that model presents, so
it is the broadest of the five and the one whose match is most
often an accident rather than an intention.

The one argument for a different order is worth recording,
because it is the one an operator will make: nearly all
reservations are written against hardware addresses, so putting
the hardware address first would match what people actually
configure. It is rejected because it inverts the reason client
identifiers exist. A deliberate per-client identity would lose
to the card the client happens to have in it this month, and an
operator who set an identifier precisely so that a replaced card
would keep its address would find that the replacement silently
got a different one.

What keeps the order safe is how narrow a question it answers.
Precedence only ever chooses between bindings an operator
configured in one network: nothing a client presents can take it
out of a binding, and the most a client can do is be moved from
one configured binding to another configured binding in the same
network. So the question is only ever "of the bindings written
here, which did the operator most likely mean for *this*
client", and the answer is the narrowest, because the broad ones
are there to catch the clients the narrow ones do not name.

One step of this order is not teneral's to choose, and saying so
matters more than the tidiness of claiming the whole of it.
**RFC 2131 section 4.2 is normative on the client identifier
outranking the hardware address**: where a client supplies a
client identifier the server MUST use that identifier to
identify the client, and the contents of `chaddr` are the
MUST-use fallback only where no client identifier was supplied.
That is a specified rule rather than a preference, it is cited
in `docs/provenance.md` as the `selector-client-id-over-chaddr`
row, and it is why the first two individual identities here sit
where they do rather than where an operator's habits would put
them. RFC 2131 section 4.3.1 is weaker and points the same way
for the class attributes: it requires that configuration
parameters be selected by applying its rules in the order it
gives, and the client-specific rule precedes the vendor-class
rule -- but it stops short of saying that a later rule overrides
an earlier one, so it supports individual-before-class without
settling it.

The rest is this project's own and gets no provenance row, and
that half of the arrangement is deliberate. Where the DUID and
IAID sit relative to the client identifier, whether the user
class outranks the vendor class, and the override semantics that
make this a precedence at all rather than a list of rules to
apply in turn, are all unspecified. That file admits two kinds
of source, an RFC section or a named observed-behaviour test,
and measuring another implementation would record that
implementation's choice rather than a fact about the protocol. A
row citing nothing would make the file's rule meaningless, and
that rule is the only thing keeping this project able to say
where its behaviour came from. For those parts the source is
this document, which is why the API publishes the order and why
every lookup reports which selector it applied.

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

### Which address the daemon chooses, when it chooses

The third path above is the one where the daemon picks. Unlike
the selector precedence, most of this order is **not** this
project's invention: RFC 2131 section 4.3.1 specifies it, and
teneral implements what it specifies. The four steps, tried in
turn:

1. The address the client already holds under its current lease.
2. The address it held under a lease that has since lapsed, if
   that address is in the pool and nothing holds it.
3. The address it asked for, if that address is valid and
   nothing holds it.
4. An address from the pool.

The specification's keyword there is SHOULD rather than MUST, and
it is worth saying so: this is the order a server ought to
consider, not one a client may rely on. `provenance.md`'s
`lease-address-selection-order` row carries the citation.

Three things in teneral's implementation of that order are
**teneral's own** and are published here for the same reason the
selector precedence is -- a consumer that cannot tell a specified
rule from an invented one cannot tell a bug from a disagreement:

- **The pool offers the lowest free address.** The specification
  says nothing whatever about which address the fourth step
  picks; its only remark nearby is that a server *may*, for
  example, choose the least recently used, and an example is not
  a rule. The lowest free address is chosen because it is
  deterministic, reproducible in a test, and expressible as a
  statement over an integer column, where a counter or a hash of
  the client's identity would be none of those.
- **"Valid" means inside one of the network's configured
  ranges.** There is nothing else in this daemon for a requested
  address to be valid against, a network here not being a subnet.
- **The pool never offers an address a binding reserves.** An
  address held for a machine that happens to be switched off must
  not go to somebody else, or the reservation becomes a conflict
  the moment the machine comes back.

Two consequences of the order are worth having in view. The
second step means a client keeps getting its old address back
after a lapse, for as long as the lapsed lease is still recorded
-- so whatever interval eventually sweeps expired leases is also
how long that stickiness lasts, and the two cannot be set
independently. And the first step means a renewal and an
allocation are the same request to this API: a client that
already holds an address is given it again, with the lease id it
already had, which is how a consumer tells that nothing changed.

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
mutation of the store, in one total order. The epoch says whose
sequence numbers those are: it is generated when the store is
created and does not change while that store is the store, so a
consumer presenting a remembered cursor is told "carry on from
here" rather than being handed a different history's events
under numbers it recognises.

Three things can be wrong with a presented cursor, and they are
answered separately because the consumer is in a different
position in each. Its epoch may not be this store's. Its
sequence may have been truncated out of the retained history. Or
its sequence may be *ahead* of everything this store has ever
issued.

The third is worth dwelling on, because it is what a restore
from a backup looks like from the inside -- and because this
section first said the epoch changes when a store is restored,
which the storage layer cannot make true. The epoch lives in the
database file, so restoring the file brings the epoch back with
it, along with a sequence counter wound back to where the backup
was taken. What marks that event is therefore not the epoch but
a consumer presenting a position the store has not issued, which
is why that is an answer of its own rather than being reported
as eviction. The epoch still does the job it is load-bearing
for, which is telling two stores' orders apart; noticing that
one store's order has been rewound is the third answer's.

That leaves a hole, and it is named here rather than left for a
consumer to find. The third answer catches a consumer whose
position is *ahead* of the restore point. A consumer whose
position is at or below it is not caught by anything: its cursor
is one the restored store has issued, so it resumes, and the
events it is given are different events under numbers it has
already seen. That is precisely the failure the epoch is named
for, and against a restore of the same file the epoch cannot
prevent it, because the restore brings the epoch back too.

The remedy is an operator action rather than a mechanism, and it
belongs in the restore procedure: after restoring a store from a
backup, mint a new epoch before any consumer reconnects. That
makes the restored store honestly a different store, which from
a consumer's point of view is what it is -- it has a different
history under the same name. Nothing offers that operation yet;
until something does, a restore is a resync for every consumer
by convention rather than by construction, and a daemon that
skips the convention is serving the project's signature failure.

Truncation does not change the epoch either, and that is
deliberate rather than an omission. Retention is per daemon
rather than per client, so a history truncated past what one
absent consumer held must not invalidate the cursor every
*present* consumer is holding: eviction is reported to the
consumer it affects and to nobody else.

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

**Resync.** If any of the three things above is wrong with the
cursor, the server does **not** start streaming from the
present. It sends a `resync` event naming the reason -- the
epoch changed, the cursor was evicted, or the cursor is a
position this store never issued -- and then behaves exactly
like a new client: snapshot, in-sync marker, stream.

The reasons are three answers and not one message, in the store
and on the wire both. A consumer logs them differently and an
operator reads them differently: an epoch that changed is a
different store, an evicted cursor is a consumer that was away
too long, and a position never issued is a store that has gone
backwards -- which is the only one of the three that means
somebody should go and look at the daemon.

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

An idle stream emits a periodic keepalive carrying the position
that stream has been brought up to. Two reasons, and the second is the non-obvious one.
First, a consumer can tell "nothing has happened" from "my
connection is a black hole", which in a system whose failures
are silent is worth the bytes. Second, the cursor in a heartbeat
lets a client advance its resume point during a quiet period, so
a consumer that has been idle for hours still resumes rather
than resyncing -- without it, a quiet network is the case most
likely to fall out of the retention window.

Which position it carries is the part to get right, and this note
first said "the current cursor", which is the unsafe reading. The
store's own latest position can include an event the stream has
not been handed yet, and a consumer that advanced its resume point
to that would never be told about that event -- a silent gap
produced by the very mechanism meant to prevent one. What is safe
is the highest position the daemon has delivered to *that* stream,
counting the events outside its scope, since an event a stream was
never going to be given is one it loses nothing by resuming past.

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

**There are no user accounts, no passwords and no sessions**, and
the daemon is not an identity provider. Human identity arrives
from an authenticating reverse proxy, which terminates TLS, runs
the OIDC flow, and forwards the authenticated user, their groups
and their email as request headers. This is the arrangement the
`private-ci` conductor dashboard already runs: nginx with an
`auth_request` against an authentik outpost, capturing the
outpost's response headers with `auth_request_set` and
re-injecting them on the proxied request.

**Those headers are trustworthy only because nothing else can
reach the daemon, and that is the invariant to defend.** A header
is a claim; what makes it evidence is that the only writer who
can set it is the proxy. The conductor achieves that by listening
on `127.0.0.1`, which is good but not airtight -- any local user
can reach a loopback port. A Unix socket is strictly stronger,
since its file mode names exactly who may connect, and that is a
further argument for the transport recommended below.

**A header-trusting listener and a token-trusting listener are
not the same listener.** If a machine client could connect to the
socket that accepts identity headers, it could simply supply its
own and become any user. So the two trust models get two
endpoints with two file modes: one the proxy owns, where identity
comes from headers and a bearer token is ignored; one for
programmatic clients, where identity comes from a token and
identity headers are rejected outright rather than ignored. The
daemon should refuse to start if it cannot tell them apart.

**Beyond those, bearer tokens as above**, which is what a
consumer like a hypervisor fleet uses: it cannot complete a
browser flow, and it should not have to. The conductor meets the
same problem for its webhook and its archive manifest, and solves
it with exact-match locations that bypass the proxy and
substitute an HMAC signature or an address allow-list -- a useful
precedent for which paths may skip the proxy, and a warning that
such an exemption must be an exact match rather than a prefix.

**Authorisation stays here, in the daemon.** The proxy says who
someone is; it does not say which networks they may touch. The
mapping from an authenticated identity or group to a scope is
teneral's, kept with the store, for the same reason the tokens'
scopes are: it is the only place that knows what a network is.

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

**Overruled by the operator on 2026-11-25, for protobuf on the
socket.** What follows is the recommendation as it was made, kept
intact because the reasoning is what a future argument will be
measured against; the ruling and what it costs are in *The
transport ruling* at the top of this note.

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

These four were written while the recommendation stood. Three of
them now read the other way round -- they are what would reopen
the ruling -- and the fourth is answered. Item 2 is the one worth
keeping in view: it asked for a measurement of JSON's cost, and
protobuf is the thing that measurement would have argued for, so
nobody now needs to take it. Item 3 is unchanged by the ruling,
because a consumer on another host needs a listener either way.

Four things, each stated so that it can be checked rather than
argued:

1. ~~**The user interface turns out to have no server-side
   half.**~~ **Answered, and it upholds the recommendation.** The
   interface is served by the daemon itself from vendored `sfui`
   assets, behind the same authenticating proxy that the rest of
   the API sits behind. So a reverse proxy is present by
   construction rather than by assumption -- it is where the OIDC
   flow lives -- and the socket has something to be bridged by.
   There is no static bundle talking to the daemon directly, no
   cross-origin policy to own, and no TLS for the daemon to
   terminate.
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
- **How an authenticated identity maps to a scope.** That
  authorisation belongs here rather than in the proxy is settled
  above; whether the mapping is by group, by an explicit grant
  per user, or by a token minted for a session is not. It wants
  deciding against a real authentik configuration rather than on
  paper.

## Related documents

- [ARCHITECTURE.md](../ARCHITECTURE.md) -- the layers this API
  is the control plane for.
- [provenance.md](provenance.md) -- why behaviour is cited to an
  RFC or to a measurement, which is why the selector precedence
  above is documented as this project's own choice.
- [plans/](plans/) -- what is being built, in what order.
