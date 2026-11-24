# The binding store

The binding store is the layer everything else in teneral is a
projection of. It holds the networks the daemon serves, the
interfaces attached to them, the address ranges they own, the
answers an operator configured for a client identity, and the
leases that record who is actually using which address right now.
Every front-end is a translation between a wire protocol and a
question asked of this store, and no front-end holds per-network
state of its own.

It is a crate, `teneral-store`, rather than a module of the
daemon. Its surface is an interface other things link, and one of
its rules is only mechanical across a crate boundary: every
operation takes the network it applies to, and a caller that
wanted to skip the network would have to add a public method to
do it, which is a diff a reviewer sees. Nothing in the crate
hands out a database connection and no SQL leaves it.

There is one implementation, SQLite behind a trait. SQLite is
chosen for what it is good at here: one process, one file, real
transactions, and a read snapshot that costs nothing. The trait
exists because a second backend is foreseeable, and it is written
against what the control plane asks of a store rather than
against what this engine happens to offer -- the five demands
below are that specification, and each method group says which
of them it serves.

## Contents

- [The shape of the interface](#the-shape-of-the-interface)
- [The five demands](#the-five-demands)
- [The schema](#the-schema)
- [Client identity and selector precedence](#client-identity-and-selector-precedence)
- [The lease lifecycle](#the-lease-lifecycle)
- [The event log, the cursor and the epoch](#the-event-log-the-cursor-and-the-epoch)
- [Retention](#retention)
- [The diagnostics](#the-diagnostics)
- [Migrations and the pragmas](#migrations-and-the-pragmas)

## The shape of the interface

Two traits. `BindingStore` is the store: everything that changes
it, plus the handful of reads that cannot be answered from inside
one network. `ReadHandle` is a view of the store frozen at one
position in its history, and is where every network-scoped read
lives. A store and a consistent view of one are different things
with different lifetimes, which is why they are different types.

Both are asynchronous, and that is a design rather than an
annotation. SQLite is a synchronous library; a read transaction
borrows its connection, and a connection may be moved between
threads but not shared, so a transaction cannot be held across an
`await` in any future a multi-threaded executor will accept. A
held, consistent view is exactly what the second demand asks for.
The two cannot both be true inside the caller's task, so the
transaction lives somewhere that is not a future:

- **One writer thread** owns the write connection and is fed by a
  channel. A write is a message and its reply carries the cursor
  the write was assigned. One writer is what makes the sequence
  numbers a total order rather than an interleaving that looks
  like one, and it is what puts every commit's `fsync` on a
  thread the store owns.
- **One thread per live view**, each holding its read transaction
  on its own stack for as long as the view is held. The
  write-ahead log makes concurrent readers free, so they contend
  neither with the writer nor with each other. A view is an
  asynchronous object whose methods are channel round trips, and
  dropping it ends the transaction and the thread.

The threads are the store's own rather than a runtime's shared
blocking pool. A shared pool is shared with every other blocking
call in the process, so a writer in it can be crowded out by work
that has nothing to do with this store -- and the failure that
produces is one network's request waiting on another network's
`fsync`, which no single-network test can see. It also keeps the
choice of runtime with the binary: the only runtime machinery the
crate uses is a channel.

Two consequences are worth knowing before building on it.

A caller never wraps a store call in `tokio::task::spawn_blocking`.
There is nothing left for such a helper to move, and the crate
could not make that call itself even by mistake: it depends on
tokio for channels alone, and `spawn_blocking` does not exist
without a runtime feature.

And a view costs a thread while it lives, so it is something to
open, use and drop rather than to keep by. The store reports how
many are live, which is the quantity a bound on them would be
enforced against.

## The five demands

The control plane's design makes five numbered demands of a
storage layer; see
[api-design.md](api-design.md#what-this-asks-of-the-storage-layer)
for them in their own words. They are what the traits' shape comes
from, and each method group's documentation names the ones it
serves, so that a second implementation can tell a shape that was
chosen from one that merely happened.

**1. A total order over mutations, assigned by the store.** Every
mutating method returns the cursor it was assigned, never a bare
success. A cursor is not something a layer above can invent on top
of a CRUD interface: with more than one writer there is no
consistent order to invent. Each write also appends an event
describing its result, in the same transaction as the write
itself, so a write that could not name its cursor would be a write
that had emitted no event -- and a store whose history silently
misses a change is worse than one that refused the change
outright.

**2. Reads consistent with a point in that order.** `read()` hands
back a `ReadHandle`, which reports the cursor it reads at and
answers every question from that one position for as long as it is
held. Independent getters that agree most of the time are the
worst possible property: a consumer assembling a snapshot from
them would see a network's ranges as they were before a change and
its attachments as they were after, with nothing in the result
saying so.

**3. Compare-and-set on identity.** The methods that change or
remove an existing resource take the cursor the caller read it at
and refuse if the resource has moved on. The two operations the
specification names by hand are each one method, not a pair:
`create_binding` refuses an address a lease holds, and
`create_binding_revoking_lease` takes the lease and writes the
binding in one transaction, so there is no state in which an
address was taken from a client and the binding that justified
taking it was not written. Two methods compare an identity rather
than a position, because that is the token their caller has:
`renew_lease` compares the client, a renewal arriving from the
wire with no position to present, and
`create_binding_revoking_lease` compares the lease id.

**4. The network is a parameter of every method.** Not a column an
implementation is trusted to filter on. A method that takes an
address and whose implementation happens to scope by network is
the shape that breaks, and it passes every single-network test.
The exceptions are enumerated in each trait's own documentation so
that a reader can tell a deliberate one from an oversight, and
they are all of one kind: a question with no network to be asked
within. `epoch` and `schema_version` describe the file;
`cross_network_answers` counts a disagreement between two
networks, so attributing it to one of them would hide which pair
it was; `read` is scoped by a position rather than by a network;
`networks` answers with the set of networks; `history_everywhere`
is a scope a watch may legitimately ask for; and
`interface_owner` and `explain` both *answer* with a network,
being the resolution step a datapath performs before anything is
scoped. Note that the last two are the opposite of the shape the
demand warns against: they take no network and return the one
they found, so a caller cannot ask about one network and be handed
another's answer.

**5. History retention belongs to the store.** The event window is
durable and is truncated durably. The bound is applied inside the
write that triggered it and the low-water mark it moves is a row
in the database, not a counter in memory. A ring buffer in the
layer above would lose its history at every restart and so turn
every restart into a resync for every consumer, which is the
resume path wasted entirely.

## The schema

Seven tables. Every one is `STRICT`, so a value whose type does not
match its column is refused rather than stored as given -- without
it a column declared `INTEGER` holds the text `10.0.0.5` quite
happily, and every comparison against it then answers something
other than what the query meant.

An IPv4 address is one `INTEGER` holding its 32-bit value, never a
dotted quad in `TEXT`. "The lowest free address in this range" is
then ordinary SQL and range containment is a comparison; as text
both are string operations that give the right answer for a /24 and
the wrong one the first time a range crosses an octet. Every
address column carries `CHECK` constraints bounding it to the
unsigned 32-bit space, SQLite's integers being signed and 64-bit.
IPv6 is not in this schema at all, and the column is deliberately
not widened in advance: the migration that adds v6 is where the
choice between a 16-byte blob and a table of its own gets made,
with something actually using it.

Every configured row carries a `cursor_seq`, the sequence number of
the event the write that produced its state emitted. A reader gets
the position each row was last written at without a second query,
and a caller that wants to change a row presents that position
back.

| Table | Key | Why that is the identity |
|-------|-----|--------------------------|
| `store_identity` | the single row | The epoch: the half of a cursor that says whose sequence numbers these are. |
| `event_log` | `seq`, autoincrementing | One row per mutation, in the order the store applied them. |
| `event_history` | the single row | How far the log has been truncated. |
| `network` | `name` | The caller's own name for the network, opaque to the store. |
| `attachment` | `(namespace, ifname)` | An interface belongs to exactly one network, and the key is what refuses the second claim. |
| `address_range` | `(network, name)` | A span of addresses a network may allocate from, named within it. |
| `binding` | `(network, selector kind, selector value)` | A selector means nothing outside a network, so it is not a resource of its own. |
| `lease` | `(network, address)` | Within one network an address has at most one holder; a client may hold two addresses. |

**A network is a name and nothing else.** The name is the caller's,
unique in the store, and opaque here: a UUID and the word `guest`
are equally valid, because a consumer that already has a stable
identifier for the thing should not have to keep a mapping to a
second one this store invented. It is deliberately not keyed on the
subnet, which two networks may legitimately share, nor on an
interface, which can be renamed. A network holds no addresses of
its own and owns zero or more ranges as child rows, which is what
makes a static-only network with no range and a home network with a
pool the same resource with different children rather than two
modes.

**An attachment's primary key is `(namespace, ifname)`, with the
network an ordinary column**, and that is the most important line
in the schema. An interface belongs to exactly one network, and a
second network's claim on an already-attached interface is the one
place where a cross-network leak would be *configured* rather than
coded -- so the database refuses it, whether or not any code above
remembered to look. Keyed the other way round, as
`(network, namespace, ifname)`, both rows would be accepted and
every single-network test would still pass. Note that the network
is still part of how an attachment is *addressed*; the uniqueness
key is deliberately narrower than the address. The namespace is in
the key because an interface name is unique only within a network
namespace, and both of the namespace-using consumers put each
network in one of its own.

**A range is identified within its network**, and ranges in one
network may not overlap each other: two ranges that both contain an
address make "the lowest free address" a question with two answers.
Identical spans in *different* networks are not a conflict and
routinely occur, which is the case the whole store exists to get
right.

**A binding's identity has three parts because a selector is not a
resource.** A selector is a `(kind, value)` pair meaningful only
inside a network -- the same hardware address in two networks is two
unrelated clients, legitimately -- so there is no table in which a
selector exists on its own, no row to reach without naming a
network, and no identifier the store could hand out that would let
somebody try. The address is *not* required to fall inside one of
its network's ranges: a network with no range at all is a supported
and common configuration, and a reservation outside a pool is how
an operator keeps an address the pool can never hand out. Nor is
the address unique within a network, because two selectors for one
machine -- its MAC and its client identifier -- legitimately point
at one address and the store cannot tell that case from two
machines colliding.

**A lease is keyed by `(network, address)` and not by the client.**
Within one network an address has at most one holder at a time,
which makes it a true key; a client may legitimately hold two
addresses while it moves from one to the other, which makes the
client not one. The consequence is that "what does this client
hold" is a query rather than a key lookup, and it is a first-class
operation rather than a filter a caller applies to a list of
everything -- that question being the one the existing consumer
cannot ask at all today.

Every child table references its network with `ON DELETE RESTRICT`.
A network with interfaces attached, ranges configured, bindings
written or leases held refuses to be deleted, and the refusal names
what is in the way: deleting a network and silently detaching its
interfaces would hand those interfaces to whichever network claimed
them next. A lease is deliberately not a child of a range, because a
pool can be resized under a lease that is already held, and
cascading that into a revocation would take an address away from a
client still using it.

Every index over a network's own rows leads with the network. An
index on an address or a holder alone would answer a question
nothing should be asking -- which networks hold this MAC -- and a
lookup that can be answered without naming a network is the shape
that serves one network's answer to another.

There is exactly one index that does not, and it is worth knowing
which so that it is not mistaken for an oversight:
`event_log_by_time`, on the recorded time and the sequence, which
serves the age half of the retention bound. That bound is
store-global by construction, because the event log is one order
over every network together and truncating it is a single floor
rather than a floor per network. An index leading with the network
could not answer "what is older than this" without visiting every
network in turn. Nothing routes a read of a network's data through
it: it exists for the sweep of the log itself, which is the one
operation in the store that is legitimately not scoped to a
network.

## Client identity and selector precedence

A client presents several attributes at once: a hardware address,
perhaps a client identifier it was configured with, perhaps a DUID
and IAID, perhaps a user class or a vendor class. Each is a
selector kind, an operator may configure a binding against any of
them, and several of a client's attributes may have bindings in the
same network -- so an order between them has to exist.

Highest first: the client identifier, then the DUID and IAID, then
the hardware address, then the user class, then the vendor class.
One principle generates it. An attribute naming a single client
outranks one naming a class of clients; among the individual ones
the identity the client asserts outranks the identity of the
hardware it happens to be in; among the class ones the class the
deployment set outranks the class the vendor shipped.

The top pair of that order is not teneral's choice. RFC 2131
section 4.2 is normative: where a client supplies a client
identifier the server MUST use that identifier to identify the
client, and the contents of `chaddr` are the MUST-use fallback only
where no client identifier was supplied. That rule has a row in
[provenance.md](provenance.md) (`selector-client-id-over-chaddr`).
The rest of the order -- where DUID/IAID sits, the user class
against the vendor class, and the override semantics that make this
a precedence at all rather than a list of rules applied in turn --
is genuinely this project's own, is argued in
[api-design.md](api-design.md), and gets no provenance row, because
a row citing nothing would make that file's rule meaningless.

Because part of the order is invented, every lookup returns **which
selector matched**. That is not a nicety: a consumer has no way to
check that its understanding of an invented precedence agrees with
the daemon's except by being told which rule was applied. An
operator who configured a reservation against a hardware address and
is told a vendor class matched has found a precedence problem before
a user does.

## The lease lifecycle

Three states, of which only two are rows:

- **Unleased.** There is no row for `(network, address)`.
- **Held.** There is a row whose expiry instant is still ahead of
  the clock, and it names the holder.
- **Expired.** There is a row whose expiry instant has passed.
  Nothing holds the address. This is not a stored state and no
  write produces it: it is the *same row*, read later.

| From | By | To |
|------|----|----|
| Unleased, Expired | acquiring, with the address named | Held, under a new lease id |
| Unleased, Expired | allocating, with the address chosen here | Held, under a new lease id |
| Held | allocating, where the holder already holds it | Held, same lease id, later expiry |
| Held | renewing | Held, same lease id, later expiry |
| Held, Expired | revoking | Unleased |
| Expired | sweeping | Unleased |

Two transitions are absent on purpose. There is no way to move a
held address to a different holder in one step without saying so:
an acquisition against an address another client holds is refused
and names the holder, and taking it requires revoking the lease,
which is something a consumer has to ask for. And there is no
renewal of an expired lease -- the address was free in the interim
and may have been given away, so the honest answer to a client
renewing something lapsed is that it must acquire again, which
issues a new lease id because the holding ended.

**A lease id is the sequence number at which its current holder
acquired the address**, so it changes when and only when the address
changes holder. A renewal keeps it; an acquisition issues a new one.
Derived from the store's own order it is deterministic in a test,
needs no generator, and is already a position a consumer can compare
against its stream. It is paired with the epoch outside the table,
a sequence number from a restored file meaning nothing on its own.

**A lease is recorded against the highest-precedence attribute the
client presented**, by the same order a binding lookup walks, so a
client is identified the same way whether it is being matched
against configuration or being given an address. Asking whether an
existing lease is *this* client's is the other question, and it is
answered by whether the client still presents the attribute the
lease was recorded against: a client that has since begun presenting
something of higher precedence has not become a different client,
and treating it as one would hand it a second address while refusing
it the first. The converse is a real limitation and is stated in
*Known gaps* below.

### Expiry is absolute and lazy

A lease carries the instant it expires, as seconds since the Unix
epoch, not a duration and a start. A row whose instant has passed is
expired at the moment it is read -- by every read and by every
allocation -- whether or not anything has swept it. A lease is held
while the clock is before its instant and expired from the instant
itself, and every statement spells that same rule.

The sweep therefore has exactly one job, which is to turn expiries
into events so that a consumer watching the history hears that an
address was given up rather than inferring it from a timestamp it
was left to compare itself. It is an observability mechanism and not
a correctness one: nothing waits for it, so a sweeper that dies
cannot cause a double allocation. Nothing built later should come to
depend on it having run.

Time arrives through an injected clock, so a test advances it on
command rather than sleeping. A test suite with real waits in it
gets made faster by somebody eventually, and the thing they shorten
is the margin the test was measuring.

### Which address a client with none is given

The order is RFC 2131 section 4.3.1's, which specifies four steps,
introduced by SHOULD rather than MUST:

1. the address the client already holds under a live lease;
2. the address it held under a now-expired or released lease, if
   that address is in the pool and not already allocated;
3. the address it asked for in the Requested IP Address option, if
   that address is valid and not already allocated;
4. a new address from the pool.

The row is `lease-address-selection-order` in
[provenance.md](provenance.md). Two things in teneral's
implementation of it are *not* from that source and are named there
as the project's own: which address the last step picks, the section
being silent and teneral taking the lowest free one; and the reading
of "valid" as falling inside one of the network's configured ranges,
there being nothing else in this daemon for an address to be valid
against.

An address a binding reserves is not offered from the pool. An
address held for a machine that is switched off must not go to
somebody else.

A network with nothing to allocate is refused with the reason, and
the two reasons are told apart because they have different fixes: a
network with no range allocates nothing by design, and the caller
probably meant to assert an address rather than ask for one, while a
network whose ranges are full is an operational problem.

### The durability point

A lease that has been acknowledged to a client and then lost to a
power cut is a duplicated address: the store will hand the same
address to somebody else, and the first client will keep using it.
That is the project's failure mode in its purest form, so the order
is fixed. **The store commits durably, and only then is the lease
returned to the caller that may acknowledge it.** Never the other
way round, and never with the commit deferred to a batch.

Two halves make that true. The write connection runs with
`synchronous = FULL`, which under the write-ahead log is what makes
a commit `fsync` -- `NORMAL` does not, so it is durable against a
process crash and not against losing power, and losing power is the
case that produces the duplicate. And the lease a write hands back
is built after its transaction has committed, from the values it
wrote, rather than being available any earlier. The store offers no
operation that lets a caller see a lease before it is committed.

The cost is one `fsync` per lease transaction. If that ever proves
too expensive, the honest fix is batching acknowledgements, not
weakening the commit.

## The event log, the cursor and the epoch

A **cursor** is a position in the store's one order of mutations:
an epoch and a sequence number. The sequence is assigned when a
mutation commits, is never issued twice, and increases over every
network's mutations together. The epoch says whose sequence numbers
these are, so that a consumer presenting a remembered position is
told "carry on from here" rather than being handed a different
history's events under numbers it recognises.

There is deliberately no ordering implementation on a cursor.
Comparing two is meaningful only once their epochs are known to
match, and an `Ord` that quietly ignored the epoch would make the
meaningless comparison the easy one to write.

A cursor is not a timestamp, and **the order is not causality
between networks**. Two networks' events are interleaved by nothing
more than when they arrived; the sequence exists so that a stream
can be resumed, not so that something in one network can be shown
to have caused something in another. Within one network the order
is meaningful and is the order the store applied the changes in.
The store can do nothing to enforce that beyond saying it, which is
why it says it on the cursor, on each event, and on each method
that returns one.

The log's sequence column is `INTEGER PRIMARY KEY AUTOINCREMENT`,
and the keyword is load-bearing. A bare rowid is the largest in use
plus one, so SQLite reuses the numbers of deleted rows -- and this
table is *designed* to be deleted from, because the retained
history is bounded. A reused sequence number hands a resuming
consumer somebody else's history while looking perfectly
well-formed, which is the failure the epoch exists to prevent,
reintroduced one layer down.

An event carries its network, its kind, the resource's identity
within that network, and the resource's **whole post-state** rather
than a delta -- so applying an event is idempotent and at-least-once
delivery is enough; a deletion is spelled as the absence of a state.
The kind is a string and the state is serialised, rather than a
column per resource, which is what makes a resource this schema does
not hold yet a migration and a variant each rather than a reshaping
of the one table every consumer reads. Every event names its network
even when it was read through a single network's history: a consumer
that learned to infer it from the question it asked is wrong the day
somebody widens the question.

### What a presented position can be told

A consumer with no position takes a **snapshot**: the current state
of one network as one event per resource, read at a single position
and carrying that position. It is a consistent view rather than a
walk that races mutations, because it is a view. A stream that then
resumes at exactly that position has no gap for an event to be lost
in and no overlap to deduplicate, because the snapshot is everything
up to and including it and a resume asks for what came after. A
network the store does not hold is refused rather than answered with
an empty snapshot: the act a consumer performs on a complete
snapshot is to delete whatever the store did not mention, so "there
is no such network" and "this network has nothing in it" must not
share an answer.

A consumer with a position reads the history, and gets one of
**four** answers. Three of them are refusals, and they are separate
values rather than one error carrying a sentence, because the layer
above has to act on them differently and a consumer must never have
to parse a message to tell them apart.

| Answer | What it means | What the consumer does |
|--------|---------------|------------------------|
| **Resumed** | The position was inside the retained history. Every event after it, in order, and the position the stream has now reached. | Carries on. |
| **Evicted** | The events after that position have been truncated. Carries the low-water mark, so what is gone is described rather than guessed at. | Takes a snapshot. |
| **Epoch changed** | The position belongs to a different store's order, so its sequence number means nothing here. | Takes a snapshot. |
| **Never issued** | The epoch matches and the position is *ahead* of everything this store has ever assigned. | Takes a snapshot. |

A resume delivers the **intermediate states**. A lease created and
then released while a consumer was away produces both events and not
neither: collapsing them would be cheaper and would make the stream
unusable as a sequence, because a consumer holding the earlier state
would never see it end.

### The epoch, a restore, and the gap that is left

The epoch is generated by the engine when the database is created,
as sixteen random bytes of hex, and it lives in the file. It
separates this store's history from another store's: a fresh file at
the same path, a second daemon, a database recreated after being
deleted.

Be precise about what it does **not** cover. It does not detect this
same store restored from a file-level backup, because the restore
brings the epoch back unchanged together with a sequence counter
wound back to the backup. A consumer whose position is ahead of the
restore point is caught, by presenting a position the store never
issued -- which is what the fourth answer above is for, and why
reporting it as eviction would be wrong: nothing was discarded,
nothing was ever there. A consumer whose position is at or below the
restore point is **not caught at all**, and is served different
events under numbers it has already seen. That is the failure the
epoch is named for, and against a restore of the same file it does
not prevent it.

The remedy is an operator action rather than a mechanism: after
restoring a store from a backup, mint a new epoch before any
consumer reconnects, which makes the restored store honestly a
different store. Nothing in the crate offers that operation yet; see
*Known gaps*.

## Retention

The history is bounded by a count of events and by their age, with
the **count as the operative bound**. The default is **50,000
events or seven days**, whichever bites first, chosen when the store
is opened rather than written into the database, both being
properties of a deployment rather than of a file.

The count leads because the event rate between a home network and a
hypervisor fleet differs by orders of magnitude, so a duration that
held a useful window for one would hold minutes or months for the
other. At a fleet's rate 50,000 events is a window measured in
hours, so a consumer restarted in seconds or partitioned for minutes
resumes and one that has been away since yesterday resyncs; at a
home network's rate it is months, which is where the age bound takes
over. In storage it is some tens of megabytes of rows. The week is
deliberately generous rather than tuned: its job is to put a ceiling
on a quiet store's log, not to shape the resume window.

None of this is a promise to a consumer. The resume path is
best-effort by construction and resync is the normal answer rather
than the error path, which is what makes these numbers safe to
change.

Two properties of the truncation are load-bearing.

It happens **in the same transaction as the write that triggered
it**, which is what makes the bound durable rather than aspirational:
a store that evicted outside the write would have a window a crash
could leave in any state, and a low-water mark that disagreed with
the log it describes.

And it only ever deletes a **prefix** of the order, expressed as a
sequence floor. The retained events are therefore always a suffix,
which is what lets one number describe completely what is gone, and
what makes "can this position be resumed" a comparison rather than a
search. The mark is a row in the database, not a counter in memory,
because a mark that reset to zero on restart would tell a consumer
whose position really had been evicted that it could resume -- and
then send it nothing.

One consequence looks like a dangling reference and is not. A
resource row records the position it was last written at, and nothing
stops that event being evicted while the row stays exactly as it is;
an address range nobody has touched for a week is the ordinary case.
The position goes on working for what a row's cursor is for, which is
"change this only if it is still what I read", a comparison of two
numbers and not a lookup in the log. What it stops being is a
position to resume a stream from, and a consumer that tries is told
the correct thing: the events after it are gone, take a snapshot.

## The diagnostics

The stated failure mode of this project is a silently wrong answer,
so a consumer needs mechanisms for noticing that it was given one.
Three of them are store operations rather than wire shapes.

**The explain operation.** Given a synthetic client identity and an
interface, it answers what the store *would* reply with no packet
involved: which network the interface resolved to, which binding
would match, which selector matched it, and which selectors would
have been consulted in what order. It converts the project's whole
failure class from something observed hours later inside somebody's
guest into a question an operator or a test can ask directly.
Reporting which network an interface resolved to is half of what
makes it a diagnostic, because an interface attached to the wrong
network looks exactly like a binding that does not match. It is the
same code path as the lookup with the packet replaced by its
parameters, deliberately: a diagnostic that reimplemented the
matching could agree with the documentation while the datapath did
something else, which would make it worse than having none.

**The per-network digest.** One cheap value per network that changes
when its configuration changes, so a consumer holding a mirror
compares one value instead of diffing everything and a mismatch means
"resync" rather than "investigate". Two things to know before relying
on it. It covers what an operator *configured* -- the network, its
attachments, its ranges and its bindings -- and deliberately not its
leases, because a lease changes whenever a client appears and a
digest including them would answer "has anything happened" rather
than "do we still agree". And it is opaque: compare two for equality
and nothing else, and only ever two of the same network's. It is not a
hash of content, so two identically configured networks have
different digests; what it answers is whether *this* network's
configuration is still the one the consumer last saw. A change in
another network cannot move it, which is the property the whole
mechanism rests on.

**The counter that should always be zero.** Every row the store hands
back carries the network it came from, and every read that named a
network compares the two before the row becomes an answer. A
disagreement is returned as an error and counted; it is never served.
The error half is what makes it a guard rather than a dashboard: a
mechanism that notices a cross-network leak and then serves the
answer anyway has converted a bug into a metric. The counter half is
an alarm with no baseline to interpret, since in a correct store it
stays at zero for the life of the process and a single non-zero
reading is unambiguous.

Where it can fire is worth being precise about. A read that filters
on the network it was given can only return rows from that network,
so comparing them is a tautology *while the statement is right*. The
comparison is there for the statement that stops being right: a
`WHERE` clause dropped in an edit, a parameter order rotated by one,
an index or a cache keyed on an address instead of on a network and
an address. Each is a one-character change that every single-network
test passes, and each becomes a refused read and a non-zero counter
instead of a wrong answer. One read can fire from data alone, and it
is the one the control plane actually offers: a lookup given both a
network and the interface a client arrived on. Those two facts come
from different places -- the network from the caller, the interface's
network from the store -- so they can disagree, and that disagreement
is exactly the quantity the specification asks to be counted.

The count belongs to the store rather than to a network in it,
because each reading *is* a disagreement between two networks and
attributing it to one of them would hide which pair it was. The error
names both.

Two further mechanisms from the same list are properties of every
answer rather than operations of their own: each answer names the
network it came from and the position it was read at, and each lookup
names the selector that matched.

## Migrations and the pragmas

The schema version lives in `PRAGMA user_version`. Migrations are
forward-only and are applied at open, each inside a transaction. A
migration file is never edited once it has been released: a change to
the shape of a table is a new file beside it, because an edit to an
old one describes a database that only a fresh install has ever had.

**A database a newer build wrote refuses to open**, and the refusal
names both versions. Opening a schema that is ahead of the code and
interpreting it anyway is the silent-wrong-answer failure applied to
storage: the columns the code knows will mostly still be there, and
the ones it does not know about will be the ones that mattered. There
is no downgrade path, because a downgrade that drops a column drops
bindings an operator configured.

Three pragmas are set on the connections the store opens:
`foreign_keys = ON` (per connection, not a property of the file,
which is why the `RESTRICT` references above only bind because every
connection asks), `journal_mode = WAL`, and `synchronous = FULL` on
the writer.

Two of the three are already the bundled engine's defaults, which
matters for how they are tested rather than for what they do. The
amalgamation this crate compiles is built with foreign keys on and
with `synchronous` at `FULL` under both journal modes, so a test that
merely observes either behaviour passes with the pragma line deleted
and proves nothing. Each is therefore guarded by a test that builds
the precondition the default hides -- a connection with the setting
turned off -- and asserts the pragma set turns it back on. The lines
stay because the alternative is an invariant resting on a compile
flag inside a dependency's build script that nobody reads. The rule
generalises: a pragma this engine already sets is a line whose test
proves nothing unless the test constructs the precondition.

SQLite itself is compiled from the amalgamation the binding carries,
which pins the storage engine's version in `Cargo.lock` exactly like
any other dependency. Linking whatever the build image happens to
carry would make the engine's version a property of the container
rather than of the source, and in a project whose stated failure mode
is a silently wrong answer, the version of the thing that stores the
answers is not something to discover at runtime.

## Known gaps

Honest limits of what is built, each of which a later layer has to
decide about rather than discover.

- **No operation mints a new epoch.** That is the remedy for a
  restore from a file-level backup, and there is nothing to call.
- **A client that stops presenting its client identifier is not
  recognised as its lease's holder.** A lease is recorded against the
  highest-precedence attribute presented, so a client that supplied
  an identifier and later stops supplying one looks like a different
  client. The store cannot know that two identities are one machine;
  this is the protocol's own warning about keying on a hardware
  address, in reverse.
- **There is no "revoke that lease and *update* this binding".** The
  create form exists because the specification names it; the update
  form does not.
- **Nothing bounds how many views are live at once.** Each costs a
  thread. The store reports the count, so the bound is enforceable
  from above, but refusing to open a view needs a policy and an error
  a consumer expects, and those are the control plane's vocabulary.
- **How often leases are swept is also how long a client keeps
  getting its old address back**, the second step of the selection
  order reading the lapsed lease row that the sweep deletes. Whoever
  chooses the sweep interval is choosing both.

## Related documents

- [api-design.md](api-design.md) -- the control plane this store sits
  under, the five demands in their own words, the watch primitive's
  wire half, and the selector precedence argued at length.
- [provenance.md](provenance.md) -- where each behaviour came from.
  The store's two RFC-cited rows are
  `selector-client-id-over-chaddr` and
  `lease-address-selection-order`.
- [../ARCHITECTURE.md](../ARCHITECTURE.md) -- the layers this store
  is layer zero of.
- [development.md](development.md) -- how to build, lint and test,
  including `scripts/check-store-trait.sh`, which checks the two
  things about the traits' shape that fail silently.
