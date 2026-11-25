# Teneral implementation phase 4: the control API

## Planning effort

High, and the master plan says why: the API's shape is on its
list of high-effort work, alongside the store's schema and
anything that decides which network a request belongs to. This
phase is all three at once. It also carries a ruling that
reverses a recommendation phase 1e made with a long argument,
which means the plan has to record what the project now owes
because of it rather than quietly dropping the argument.

Review effort: high. Every step either touches the store's trait
or decides what a consumer is allowed to see, and both are places
where a defect is silent and cross-network.

## Scope

**In.**

- A control plane: protobuf messages, length-delimited, over Unix
  domain sockets, in a new `teneral-api` crate -- server, client
  and the schema both halves are generated from.
- The resources the store actually holds: networks, attachments,
  address ranges, bindings, and leases (read and revoke only).
- The three paths a consumer learns an address by, from
  `docs/api-design.md:441-530`: assert-and-be-told, ask, and be
  told.
- The watch primitive in full: snapshot, in-sync marker, resume,
  the three resync reasons, the heartbeat, and disconnection of a
  reader that falls behind.
- Authentication: the socket's mode and its peer as the outer
  gate, bearer tokens that narrow and never widen, token minting
  by a peer the socket's mode already trusts, an `admin`
  capability for the operator's operations, and a token recorded
  against every write.
- The diagnostics `docs/api-design.md:748-805` asks for: the
  explain query, the per-network digest, the scope and cursor on
  every answer, the selector that matched, and the counter that
  should always be zero.
- The two debts the master plan's *Future work* names as this
  phase's: minting a new epoch after a restore, and bounding how
  many read handles are live at once.
- Four changes to the store's trait that the control plane cannot
  be built without, and one it would be dishonest to defer; see
  decision 6.
- The `teneral` binary becoming a daemon that serves the sockets,
  with its options as flags, because there is no configuration
  file and will not be one.
- `docs/control-api.md`, and the fold-back into the documents the
  phase makes wrong.

**Out, with the reason.**

- **Zones, static records and projected records.** Phase 6's,
  with the DNS front-end that consumes them. The store does not
  hold them: `EventKind` has five variants and none is a record
  (`teneral-store/src/history.rs:130-142`).
- **Content blobs**, and the blob-reading capability that goes
  with them. Phase 10's, with the metadata front-end. The token
  model is built so that adding the capability later is a row in
  a `TEXT` column rather than a migration.
- **The DHCP and DNS front-ends.** Phases 5 and 6. Nothing in
  this phase writes a lease: the API exposes reading and revoking
  a lease because derived state is not writable
  (`docs/api-design.md:212-217`), so the three lease-writing
  store methods still have no caller but the tests when this
  phase ends.
- **The expiry sweeper and its interval.** The master plan's
  *Future work* records that the sweep interval is also an
  address-stickiness knob and that phase 5 chooses it knowing
  that. A daemon that swept on a timer this phase picked would
  have chosen the stickiness of every address in the deployment
  as a side effect, so this phase runs no sweeper.
- **A TCP listener, TLS, CORS, and anything a browser touches
  directly.** The ruling in decision 1 puts the browser behind a
  consumer that translates, and that consumer is the home IPAM in
  its own repository.
- **Prometheus metrics.** The diagnostics call carries the
  counters; an exporter is phase 7's, with the deployment that
  scrapes it.
- **Rate limits and quotas.** `docs/api-design.md:1096-1099`
  defers them, and the reasoning holds: the control plane's input
  is authenticated and local.
- **How an authenticated human maps to a scope.**
  `docs/api-design.md:1100-1105` wants that decided against a
  real authentik configuration. This phase does the half that is
  decidable here -- tokens, their scopes, and who may mint one.
- **`update_binding_revoking_lease`.** The *Future work* entry
  says to fill it when a consumer needs one. This phase's survey
  looked: nothing in the API's shape needs it, because changing a
  binding onto an address something else holds is refused with
  the holder named, which is a legitimate answer. The entry
  stands, with that recorded.

## What the survey found

The master plan's sketch for this phase is one sentence -- "the
transport chosen in 1e, CRUD over the store, watch/subscribe,
authentication" (`docs/plans/PLAN-teneral-implementation.md:456`).
It is accurate and it is not a specification. The specification
is `docs/api-design.md`, 1070 lines of it, written by phase 1e
and fixing the resource model, every identity, the watch
primitive's behaviour across a reconnect, the authentication
model and six diagnostics. This phase file is written against
that document, exactly as phase 3's was. Where the sketch and
the note disagree, the note wins; where the note and the
operator's ruling disagree, the ruling wins and the plan says so.

Fifteen findings, each checked against the tree rather than
assumed.

1. **Open question 1 was still open, and is now answered.** The
   master plan's question 1
   (`docs/plans/PLAN-teneral-implementation.md:237-268`) carried
   protobuf over a Unix socket as its default and phase 1e's
   recommendation of HTTP with JSON bodies as its finding, with
   the note that the operator had not ruled. He ruled on
   2026-11-25, for the default: protobuf on the socket. Decision
   1 records it, what it overturns, and the three debts it
   creates. The question is marked resolved at its source in the
   same commit as this file, and `docs/api-design.md` carries the
   ruling at all four places that state the recommendation -- the
   short version's bullet, a ruling paragraph beside it, the
   recommendation itself and what would have changed it -- so that
   no step of this phase reads a recommendation the project is not
   building.

2. **The store already answers the three things that can be
   wrong with a cursor, as three separate answers.**
   `History` is an enum of four variants --
   `Resumed`, `Evicted`, `EpochChanged`, `NeverIssued`
   (`teneral-store/src/history.rs:276-345`) -- and the wire's
   three resync reasons are the last three of them. The API layer
   translates; it invents nothing. This is the single largest
   thing the survey found in this phase's favour.

3. **`live_views()` is not on the trait.** It is an inherent
   method of the concrete `SqliteStore`
   (`teneral-store/src/sqlite.rs:320`). The *Future work* entry
   that hands this phase the view bound says "the store does the
   half that is genuinely its own: `live_views()` reports the
   count, so a bound is enforceable from above", and that is true
   only for a caller holding the concrete type. A control plane
   over `dyn BindingStore` cannot read it. Decision 6 puts it on
   the trait.

4. **The store has no change notification of any kind.** No
   `notify`, `subscribe`, `watch` or `broadcast` anywhere in
   `teneral-store/src`. So the only way for the API layer to
   learn that the order advanced is to read the history, and
   every read handle is a thread of its own
   (`teneral-store/src/sqlite.rs:938-1066`): a watch stream
   polling on a timer would spawn and join a thread per poll per
   consumer. This is what makes the notification a correctness
   and cost matter rather than a nicety. Decisions 6 and 8.

5. **Nothing in the store knows about tokens.** Five migrations,
   schema version 5, and no table, column or comment mentions a
   token or an actor. `ARCHITECTURE.md:45-48` already says tokens
   are "deferred to the layers that will consume them", and this
   is that layer. Migration 0006 is this phase's.

6. **"Every write records the token that made it" has nothing to
   hang on.** `docs/api-design.md:877-880` asks for it and the
   event log records only `recorded_at`. Doing it means an actor
   argument on every mutating method of the trait -- sixteen of
   them, counted from the gate script's own output rather than
   estimated. The operator ruled on 2026-11-25 that this phase does
   it, and the timing is the argument: the only callers today are
   the store's own tests, and phase 5 is what adds the
   `acquire`, `allocate` and `renew` call sites that would make
   the same change a rewrite.

7. **The epoch cannot be replaced through the trait as it
   stands.** `fn epoch(&self) -> &Epoch` returns a borrow of
   state that minting replaces (`teneral-store/src/store.rs:116`),
   and the value is read once when the store opens. Minting is
   also a write whose verb `scripts/check-store-trait.sh` does
   not know: its verb list
   (`scripts/check-store-trait.sh:155`) has no `mint`, and the
   script's own header says adding a write means adding its verb
   in the same commit, which then requires the method to return
   the cursor it was assigned. That is not a nuisance -- the
   first position of the new order is exactly what a caller wants
   back. Decisions 6 and 9.

8. **The store's event state is JSON text built by the engine,
   with the store's own key spellings.** `json_object('network',
   network, 'namespace', namespace, 'ifname', ifname, ...)` and
   its siblings at `teneral-store/src/history.rs:788-830`,
   `teneral-store/src/ops.rs:726-820` and
   `teneral-store/src/lease.rs:460`. Phase 3 chose that
   deliberately so the store needs no serialisation dependency.
   Under decision 1 it is a cost the HTTP option would not have
   paid: the API layer has to parse that JSON and re-encode it as
   a typed message. Decision 11 says how, and makes the result
   falsifiable rather than hoped for.

9. **Two of the resource model's rows cannot be built here.**
   Zones, static records and projected records, and content
   blobs, are absent from the schema and from `EventKind`. So the
   watch's "resource kinds" vocabulary is five, not eight, and an
   unknown kind is rejected rather than ignored -- which is what
   `docs/api-design.md:545-549` asks for anyway, and which turns
   a consumer written against the full note into a version
   mismatch it hears about rather than a silently empty stream.

10. **Two documents still say this repository's daemon serves the
    web interface.** `ARCHITECTURE.md:79-82` and
    `docs/api-design.md:102-112`, with
    `docs/api-design.md:1008-1017` relying on it to uphold the
    transport recommendation. Open question 3's amendment of
    2026-11-23 overturned that: this repository is teneral's core
    libraries, and the binary that serves the interface is the
    home IPAM's, in its own repository, built from these
    libraries. This is load-bearing for this phase rather than
    cosmetic, because it is what makes the deliverable a library
    another repository links plus a thin binary, rather than a
    daemon with pages in it. The phase's documentation step
    corrects both.

11. **The `teneral` binary depends on nothing.** Its manifest has
    no `[dependencies]` section at all and `main.rs` prints a
    version string (`teneral/src/main.rs:15-17`). "CRUD over the
    store" has no wiring to inherit; the daemon is written in
    this phase from nothing.

12. **The fleet has already chosen the protobuf halves of this.**
    Shaken Fist's privileged executor is raw protobuf over a Unix
    stream socket, one request per connection
    (`docs/api-design.md:903-908` records the reading), and
    `ryll`'s control socket is the organisation's only watch
    primitive -- newline-delimited JSON, subscribe, unsubscribe,
    bounded queues, a drop report. `ryll` also carries `clap` 4
    with derive for its flags. So one request per connection and
    `clap` are consistent with the fleet; the stream is new work
    here, as `docs/api-design.md:912-914` says it is either way.

13. **Workspace mechanics need nothing, and four sentences will
    be wrong.** `make test`, `make lint` and `make deny` all pass
    `--workspace`, so a third crate is picked up with no change
    to the `Makefile` or the CI lanes. Four places say the
    workspace has two: `Cargo.toml:2`, `AGENTS.md:9`,
    `docs/development.md:13` and `ARCHITECTURE.md:43`. Phase 3
    shipped exactly this class of stale sentence and had to fix
    it afterwards; the definition of done greps for it.

14. **The licence gate is an allow-list with no exceptions**
    (`deny.toml`, `exceptions = []`, permissive licences only,
    `multiple-versions = "warn"`). The prost tree is MIT and
    should pass, and "should" is not a result: the dependency
    gate in step 4a proves it with `make deny` before any code is
    written, as phase 3's did.

15. **The build image has no protobuf compiler.** The `Dockerfile`
    adds `rustfmt`, `clippy` and `cargo-deny` to
    `rust:1.100.0-trixie` and nothing else, and compile targets
    run `--network none` against a read-only cargo cache. A
    `build.rs` that invokes `protoc` therefore needs `protoc` in
    the image, installed when the image is built, which is the
    one place in this repository that may reach the network.
    Decision 2.

Nothing else the master plan's sketch claims turned out to be
wrong, because it claims almost nothing. The note it points at,
on the other hand, was checked clause by clause and is sound
apart from finding 10.

## Decisions

### 1. Protobuf over a Unix socket, because the operator ruled so

**Resolved by the operator on 2026-11-25, for the master plan's
standing default and against phase 1e's recommendation.** The
control plane speaks protobuf messages over Unix domain sockets.
There is no HTTP in this daemon, no TCP listener, and no TLS.

Open question 1 is closed by this, and the honest record is that
it closed against the note that was written to decide it. Phase
1e's argument is not deleted and should not be: a browser needs
HTTP rather than TCP, so an ordinary reverse proxy bridges a
Unix socket for nothing *if what is on the socket is HTTP*, and
protobuf on the socket is what forces that proxy to translate
instead of forwarding. The note called that decisive. The
operator ruled otherwise, which is his to rule, and the plan's
job is now to name what the project owes because of it so that
the cost lands somewhere deliberate.

Three debts, each tracked in this phase and none of them
hand-waved:

- **Something has to translate for the browser, and it is now the
  home IPAM.** Under open question 3's amendment that consumer is
  a separate binary in a separate repository, built from these
  libraries. So the translation is not a hand-written proxy
  holding a second model of the resources: it is a Rust binary
  linking this phase's client, which forwards. That is a better
  outcome than the note feared, and it is not free -- the IPAM
  becomes a place where cross-network scoping could be
  re-implemented, in a repository whose tests this plan cannot
  reach. Risk 1 carries it, and this phase discharges what it can
  from here: the client's types cannot name a resource without
  naming its network, and every response repeats the network it
  was true of, so a translator that drops a scope produces a
  mismatch its own tests can see.
- **The browser's free reconnect is gone.** An event-source
  client replays the last event id by itself and that id is the
  cursor; nothing on a protobuf socket does. So the reconnect,
  the backoff and the cursor bookkeeping are written once, in
  this phase's client crate, rather than in every consumer.
  Decision 5 makes that the client's job explicitly, because the
  alternative is each consumer hand-rolling the one mechanism
  whose failure mode is silent divergence.
- **The store's event state has to be translated.** Finding 8.
  Decision 11.

One thing the ruling makes genuinely better, and it is worth
recording as more than consolation: the schema file is now a
reviewable artifact. The resource model stops being a
documentation convention and becomes a `.proto` that both ends
are generated from, which is the argument the master plan's
default always had and the one phase 1e never disputed.

### 2. `prost` with a real `.proto`, compiled at build time; not gRPC

The schema lives in `teneral-api/proto/teneral.v1.proto`.
`prost-build` compiles it from `build.rs` into `OUT_DIR`, and the
generated code is not committed: a generated file in the tree is
a second copy of the schema that can disagree with the first, and
`OUT_DIR` makes drift impossible rather than detectable. The
build image gains `protobuf-compiler`, installed in the
`Dockerfile`, which is the only place in this repository allowed
to reach the network -- compile targets run `--network none` and
stay that way.

**Not `tonic`, and not gRPC.** The reasoning, in the order it
matters:

- gRPC means HTTP/2, which means `h2`, `tower` and `hyper` inside
  the most privileged process on the box -- it holds
  `CAP_SYS_ADMIN` and `CAP_NET_RAW` and every network's bindings
  in one address space. A framing layer we can read in an
  afternoon is a better fit for that process than a protocol
  stack we cannot.
- One request per connection, or a sequence of them, is both
  fleet precedents' shape (finding 12). Nothing here needs
  multiplexing: a watch is a connection that stays open.
- Phase 9 teaches Shaken Fist, which is Python, to drive this. A
  generated gRPC client would be convenient and `grpcio` is a
  heavy dependency for a project that watches its import cost; a
  varint length prefix in front of a generated message class is
  about fifty lines and no new wheel.
- The streaming half is new work either way, and
  `docs/api-design.md:1036-1038` already establishes that the
  cursor is the load-bearing part rather than the stream format.

What is lost is worth stating plainly: with HTTP, an operator
could have read the API with `curl`. On a protobuf socket there
is no such tool, so decision 5 gives the binary a small set of
debugging subcommands over the same client library. An API that
cannot be poked at by hand is one whose first field-debugging
session is written in Rust.

### 3. Framing, and what a connection does

Length-delimited prost messages, in both directions, on a
`SOCK_STREAM` Unix socket. A varint length then the encoded
message, which is `prost`'s own length-delimited form rather than
a bespoke header.

- **A hard maximum message size of 1 MiB**, checked against the
  length prefix *before* any body is read. The control plane's
  input is authenticated and local, and a bound on an allocation
  derived from a peer's bytes is not a thing to skip for that
  reason; a panic or an OOM here is a cluster-wide outage, as
  `AGENTS.md` says. A length over the bound is refused and the
  connection closed, without reading the body.
- **A connection opens with `Hello` and is answered with
  `Welcome`.** `Hello` carries the protocol version and the
  token secret, which only the peer the daemon runs as may omit
  (decision 4). `Welcome` carries the store's
  epoch, its schema version, the current cursor and the effective
  scope the caller has been granted. A version this daemon does
  not speak is refused naming both versions, which is what makes
  a version mismatch a sentence an operator can read rather than
  a parse failure.
- **Then request and response pairs, in order, on the same
  connection, with no multiplexing and no request ids.** The
  handshake is paid once per connection rather than once per
  read, which matters for a user interface rendering a page out
  of five of them, and in-order pairs need no correlation
  machinery.
- **A `Watch` request is terminal.** The connection becomes a
  stream of events until either end closes it. No request follows
  it, which removes the question of what a request means while a
  stream is running.
- **A collection streams rather than paginates.** A list of
  leases is a sequence of messages on the connection, ended by a
  marker, not one message holding a vector.
  `docs/api-design.md:1085-1088` leaves pagination open; this
  closes it by making it unnecessary, and it keeps the 1 MiB
  bound a per-item bound instead of a ceiling on how large a
  network may get. It also means the collection reads and the
  watch snapshot share one code path, which is the path most
  worth having only one of.

### 4. One socket, and admin is a capability on a token

**Reversed by the operator on 2026-11-25, in the back brief.**
This decision first specified two sockets -- an admin one at mode
`0600` whose peer was its credential, and a client one at `0660`
where a token was required -- restating
`docs/api-design.md:846-854`, which requires that two trust models
not share a listener and that the daemon refuse to start if it
cannot tell them apart. The ruling is one socket, with the
operator's operations gated by a capability on the token rather
than by a file mode. What follows is that arrangement, including
what it gives up, because a reversal recorded without its cost is
a reversal nobody can revisit.

- **One socket**, `--socket`, mode `0660`. The mode is what
  decides who may connect at all, and the group is the
  deployment's lever: setting group ownership needs a privilege
  the daemon should not spend, so a systemd unit with
  `RuntimeDirectory=` and `Group=` is where phase 7 puts it.
- **A token is required, verified once at `Hello`**, with exactly
  one exception, and the exception is not a convenience. A peer
  whose uid is the one the daemon runs as may act without a
  token, as the owning peer, read from the kernel rather than
  claimed. `docs/api-design.md:871-875` requires that: the first
  token is minted over the socket by a peer the socket's
  permissions already trust, and there is no initial password, no
  token file and no first-run secret to leak. Remove that
  exception and the first token has to come from a secret printed
  at startup or written to disk, which is the one thing that
  paragraph rules out.
- **Four capabilities, and `admin` is one of them**: read, write,
  watch, admin. Minting and deleting tokens, listing them, and
  minting an epoch each require `admin` or the owning peer.
  Decision 7 carries the vocabulary.
- **A token narrows and never widens, and that now has a second
  half.** The first half is scope: a token cannot reach a network
  outside its own. The second half only exists because admin is a
  capability -- **a minted token's scope and capabilities are a
  subset of its minter's.** Without that rule, a token with
  `admin` on one network could mint itself an all-networks token,
  which is the whole boundary defeated in one call. The owning
  peer has no scope, so it may mint anything; every token may
  mint only downwards.
- **The authorisation decision is not defaultable.** Every
  request kind maps to the capability it requires through one
  exhaustive `match` in one function that every handler calls, so
  an operation added without a decision about its capability does
  not compile. This is the "a wildcard is typed, never
  defaulted" rule (`docs/api-design.md:198-204`) applied to
  authorisation, and it is the mitigation for what the ruling
  gives up.
- **The daemon refuses to start** if something already answers a
  `Hello` on its socket path, or if it cannot set the mode it
  intends. A stale socket file with nothing behind it is removed
  and recreated; one with a live daemon behind it is a reason to
  stop, not to take over.

**What this gives up, stated plainly.** The strongest boundary in
the design stops being a thing the kernel enforces and becomes a
check in the code: with two sockets, a client process could not
reach the minting path at all, because the filesystem would not
let it open the file; with one, it can reach the path and is
turned away by a comparison. A defect in that comparison is a
token that can mint tokens. Three things hold it: the exhaustive
match above, the subset rule, and a test per admin operation
asserting refusal for a token without the capability -- all three
in the definition of done. The trade bought for it is real: one
path to deploy, one listener to document, and no second socket
whose mode a packaging step can get wrong.

### 5. One new crate, `teneral-api`, carrying server, client and schema

A third workspace member. Modules: `proto` (the generated types),
`server` (the service over a `dyn BindingStore`), `client` (what
a consumer links), and `wire` (the framing both halves share).
No cargo features splitting them: a feature matrix is a thing
`--all-targets` does not test, and a consumer linking the client
also linking the server costs it compile time and nothing else.

The reason this is a library and not code inside the binary is
finding 10. The home IPAM is a separate product in a separate
repository that links these libraries; what it needs is a client,
and what phase 8 needs is a server it can mount in a process that
also holds a datapath. The `teneral` binary becomes the thin
thing: parse flags, open the store, serve the socket, shut
down cleanly.

The client owns the reconnect that the ruling took away: a
watch helper that holds the last cursor it saw, reconnects with
backoff, presents that cursor, and surfaces a resync to its
caller as a distinct event rather than as a gap. Every consumer
that hand-rolls this gets it subtly wrong in the same way, and
the failure is silent divergence.

The binary also gets a small set of debugging subcommands over
that client -- read a resource, explain an identity on an
interface, show the diagnostics, mint a token, mint an epoch --
for the reason decision 2 names. They are a client, not a second
API: each is one call and printing of its response.

### 6. The store's trait reopened in four places

**Ruled by the operator on 2026-11-25: all four, in this phase.**
Phase 3's trait is three days old and its only callers are the
store's own tests, which is the cheapest this will ever be.

1. **An actor on every mutating method.** `Actor` is an enum --
   a token id, a local peer's uid, or a named part of the daemon
   itself -- rendered into one `TEXT` column rather than
   normalised, because nothing reads it back as structure. It is
   stamped on the event log row and on the resource row, so
   "who moved this binding, and when" is answerable from the
   resource after the event has been evicted. Sixteen signatures
   change, from `create_network` to `sweep_expired_leases`; `scripts/check-store-trait.sh` is the guard that the
   edit did not drop a network parameter along the way.
2. **`epoch()` returns an `Epoch` rather than a borrow of one.**
   Finding 7. A one-line change that makes minting expressible.
3. **`live_views()` moves on to the trait**, enumerated in the
   doc paragraph the gate script reads, so a bound is readable
   through `dyn BindingStore`. The bound itself is not the
   store's; see decision 10.
4. **`subscribe()` returns a `tokio::sync::watch::Receiver<Cursor>`**
   that moves whenever the order advances. The store is the only
   thing that knows when that happened -- it assigns the order --
   and `tokio::sync::watch` is already in its dependency tree
   with the `sync` feature. It carries the latest cursor rather
   than the events: a receiver that missed three movements sees
   the newest, which is exactly right for a signal to go and
   read.

The gate script gains `mint` in its verb list and a sentence in
its header saying which traits it governs, for the reason in
decision 7.

### 7. Tokens live in their own trait, and not in the event log

A token is daemon-global: its answer is which networks it may
touch, so there is no network to ask within. It is also not a
watched resource -- no consumer mirrors the token list -- and the
event log's `network` column is not nullable, by a design phase 3
argued for at length. Three ways to force tokens into
`BindingStore` and the event log were considered and all of them
damage something: a sentinel network name collides with a real
one, because names are opaque and `*` is a legal name; a nullable
network breaks every reader's assumption; and a token mutation
that returns a cursor without logging an event is a lie by the
gate's own standard.

So: a `TokenStore` trait in `teneral-store/src/tokens.rs`, with
mint, delete, list and authenticate. Its audit is the token row
itself -- created when, by whom, last used when -- rather than
the log. `scripts/check-store-trait.sh` checks `store.rs` by
default and continues to, with a sentence added to its header
naming what it governs and why tokens are outside it; a gate that
silently does not cover something is worse than one whose scope
is written down.

The secret is generated by the engine, as the epoch is
(`randomblob`), hashed with SHA-256, and returned in plaintext
exactly once. `sha2` joins `teneral-store`'s dependencies. No
password hash is needed because there is no password: the secret
is 32 bytes of randomness, so a slow KDF would defend against an
attack nobody can mount. Verification hashes the presented secret
and looks the hash up, so the plaintext never reaches a
comparison and the API layer never holds hashing logic.

Capabilities are four: read, write, watch and admin. The first
three are `docs/api-design.md:816-825`'s; the fourth is decision
4's, and exists because the operator's ruling made the operator's
operations a capability rather than a second socket. The fifth
that note names -- reading content blobs -- arrives with blobs in
phase 10, and because capabilities are rows of `TEXT` that is an
insert rather than a migration. Minting is subject to decision
4's subset rule, which the store enforces rather than trusts the
server to: `mint` refuses a scope or a capability set that its
minter does not itself hold, because an authorisation rule
checked only in the layer that calls it is one a second caller
will skip. Scope is
either a list of networks or the explicit all-networks form,
carried as its own column, because a wildcard is typed and never
defaulted.

### 8. The watch pump: one reader, a broadcast, and a dedupe at the join

One task per daemon reads the history; every stream filters it.

- The pump waits on the `subscribe()` receiver, reads
  `history_everywhere(after)` once when it moves, broadcasts each
  event on a `tokio::sync::broadcast` channel of 1024, and
  advances its own position. One read handle per movement serves
  every consumer, which is the point: a handle is a thread.
- A stream **subscribes to the broadcast before it reads
  anything**, then does its snapshot or its catch-up read, then
  starts delivering from the broadcast while **discarding any
  event whose cursor it has already delivered**. Subscribe-then-
  read leaves an overlap; read-then-subscribe leaves a gap. The
  overlap is removable by comparing cursors because events carry
  theirs and the order is total; a gap is not removable by
  anything, which is why the order of those two operations is the
  single most important line in the step.
- A receiver that falls behind gets `RecvError::Lagged`, and the
  stream **disconnects with a final event saying so** rather than
  skipping what it missed. That is
  `docs/api-design.md:715-735`'s departure from `ryll`'s dropping
  queue, and the enum variant makes it a case the compiler
  insists on handling.
- The heartbeat carries, every 15 seconds, the highest position
  the daemon has delivered to that stream -- **not** the store's
  current position, which 4e established is the unsafe reading: the
  store's latest can include an event the stream has not been
  handed, and a consumer resuming from it would never hear about
  that event. Counting events outside the stream's scope is safe
  and wanted, because an event it was never going to be given is
  one it loses nothing by resuming past. With it, an idle consumer
  advances its resume point and a quiet network stops being the one
  most likely to fall out of retention.
- A resync carries its reason -- epoch changed, evicted, never
  issued -- as three values and not one message, because an
  operator reads them differently and only the third means
  somebody should go and look at the daemon.

### 9. Minting an epoch is an online operation, and the daemon does not guess

`mint_epoch()` is a store method, reachable only from the admin
socket, returning the first cursor of the new order. Every cursor
from the old order then fails to match, so every consumer gets an
`EpochChanged` resync and reconverges -- which is precisely what
should happen to a consumer holding positions from a history that
has been rewound. Minting *is* "everybody resync", expressed as
data rather than as an operational instruction nobody can
enforce.

**The daemon does not try to detect that it has been handed a
restored file, and this is a decision rather than an omission.**
The *Future work* entry asks whether it should refuse to serve
one until an epoch has been minted. Every detector needs durable
state outside the database -- a remembered high-water mark -- and
the restore that rewinds the database is a restore of the data
directory that remembered it, so the detector comes back rewound
too. A detector that fails exactly when it is needed is worse
than none, because it will be trusted. What this phase ships
instead: the daemon logs the epoch and the latest cursor at
startup, the diagnostics call reports both, `NeverIssued` is a
distinct answer that says a store has gone backwards, and the
restore procedure in the documentation says to mint before any
consumer reconnects. The entry is rewritten to record that
phase 4 considered and declined the mechanism, with this
reasoning, so the next reader does not take the silence for an
oversight.

### 10. The view bound belongs to the API, and the count to the store

A semaphore of **64 permits** in the server, acquired before
`read()` and released when the handle drops. Refusal is a named
refusal with a retry hint, not a connection error, because
`docs/api-design.md` asks for an error a consumer expects and
knows how to retry.

Why both halves: the policy and the error vocabulary are the
control plane's, and the number is a statement about how many
concurrent consumers there are supposed to be, which only this
layer knows. But the semaphore counts only the handles this
server opened, and phase 8 will put a datapath in the same
process; the store's `live_views()` is the ground truth across
every opener, so the diagnostics call reports it next to the
bound. A bound enforced on one number and reported on another is
the arrangement that notices the day they disagree.

Sixty-four because a handle is a thread holding a read
transaction, and because the deployments in view are one home
network and one hypervisor fleet. The flag exists so the number
can be wrong without a rebuild.

### 11. The event state mapping, through one converter per kind

The store hands the API JSON text (finding 8). The API parses it
into a mirror struct per kind with `serde`, then calls **the same
model-to-message converter the typed reads call**. One converter
per kind, two callers each.

The alternative that looks cheaper is to carry the state as an
opaque JSON string inside the protobuf event. It is rejected: it
would make every consumer parse JSON on a protobuf wire, and it
would throw away the one thing decision 1 bought -- a schema that
is a reviewable artifact.

Changing the store to return typed rows instead of JSON was also
considered, and declined as out of scope: phase 3 chose
engine-built JSON so that the store needs no serialisation
dependency, and that reasoning is untouched by this phase. The
API layer is allowed a serialiser; the store still is not.

What makes this checkable rather than hopeful is a test per kind
asserting that **the message a read of a resource returns equals
the message its event carries**, byte for byte after encoding.
Two spellings of one resource -- one in a read, one on the stream
-- is the defect this class of design produces, and it is
invisible until a consumer applies a stream event over a read.

### 12. Scope leaks in the store-global reads, and what they are refused with

Two trait methods take no network because their *answer* is a
network: `interface_owner` and `explain`
(`teneral-store/src/store.rs:158`, `:639`). Both are diagnostics
this phase must expose, and both are a cross-network read if
exposed naively -- a token scoped to one network could ask which
network owns an interface, or what a synthetic identity would be
answered on it, and learn about a network it may not touch.

So: the server resolves the answer, then checks the resolved
network against the caller's scope, and **refuses without naming
the network** when it is outside. Not "that interface belongs to
`guest`, which you may not see" -- the refusal is the same one a
caller gets for an interface no network has claimed, because the
difference between those two answers is itself the leak. For the
owning peer, and for a token whose scope is the explicit
all-networks form, both answer in full -- there is no network
outside the caller's scope for the answer to leak.

This is the one place in the phase where the project's central
failure mode arrives in the control plane rather than in the
store, so the definition of done requires a two-network test for
each.

**Corrected during 4d.** This decision counted the store-global
reads as two, and for a while the wire made them three: the
schema accepted at the gate gave
`LookupBindingOnInterfaceRequest` a namespace, an interface and an
identity and no network, so the server had to resolve the
interface and check the resolution exactly as it does for the two
above. That was a schema defect rather than a third diagnostic.
`BindingStore::lookup_binding_on_interface` takes a network
because the caller's network and the interface's network can
disagree, and that disagreement is the only data path into
`cross_network_answers` -- the counter `docs/api-design.md` calls
the best alarm this project has. A message that named no network
could not express it, so the guard could only ever have fired on a
coding defect inside the store. The field was added, and the
operation is now an ordinary network-named one: plain scope check,
no resolve-then-refuse, and not on this list. The two reads this
decision is about are `interface_owner` and `explain`, and they
are two.

### 13. The numbers, and where they live

There is no configuration file and will not be one
(`AGENTS.md`). Every number is a flag with a default, parsed with
`clap` 4 derive as `ryll` does:

| Flag | Default | Why that |
|------|---------|----------|
| `--store` | `/var/lib/teneral/store.sqlite` | the one piece of durable state |
| `--socket` | `/run/teneral/teneral.sock` | the one listener, mode 0660; the group is the deployment's, and admin is a capability rather than a second path |
| `--history-events` | 50000 | the operative bound; a fleet at ten events a second resumes after eighty minutes away |
| `--history-age` | 21600 | six hours, and a ceiling rather than a resume window: the count above is the operative bound. Tighter than the store's own default of a week, which bounds the log of a quiet store and means a quiet deployment reaches the resync path at all. Evicting history a consumer could have resumed from is a cost rather than a goal, which an earlier draft of this row had backwards |
| `--max-live-views` | 64 | decision 10 |
| `--max-connections` | 256 | a connection is a task; this is the bound that stops an accept loop becoming unbounded memory |
| `--heartbeat-seconds` | 15 | decision 8 |

No flag for the sweep interval, because this phase runs no
sweeper and phase 5 owns that number for the reason the master
plan's *Future work* gives.

### 14. The control API gets no provenance rows, and the file says so

`docs/provenance.md` opens with "Every behaviour visible on the
wire gets a row", and the rule exists because dnsmasq and
Technitium are GPL and the project must be able to say where each
protocol behaviour came from. This phase's bytes are visible on a
wire and no RFC specifies them, because teneral invented them
end to end; there is nothing to cite and nothing to measure, and
a row citing nothing would make the file's rule meaningless --
the argument phase 3 already made for the parts of the selector
precedence that are teneral's own.

So the phase adds no rows, and the documentation step adds one
clarifying sentence to that file's header scoping the rule to the
protocols teneral did not invent. An over-broad rule that
everybody quietly ignores is how a rule stops being enforced.

## Step plan

Seven steps, strictly sequential: every one but 4g touches the
shared worktree, and 4b through 4f each build on the step before.
Isolation is `none` throughout, as in phase 3, with the
management session reviewing each diff before it is committed.

**There is a gate between 4c and 4d.** 4c ends with the `.proto`
written and nothing generated from it but the handshake, and the
management session reviews that schema before 4d writes two
dozen operations against it. A schema is cheap to propose and expensive
to redo, and every later step is generated from it.

| Step | Effort | Model | Isolation | Brief for sub-agent |
|------|--------|-------|-----------|---------------------|
| 4a | high | opus | none | Tokens in the store, and write attribution. Read decisions 6, 7 and 13 and findings 5, 6 and 14 first. **Do the dependency gate before anything else**: add `sha2` to `teneral-store`, run `make lock` then `make deny`, and record the resolved versions in the manifest comment with the reason each dependency is there, as `teneral-store/Cargo.toml` does for its existing three. If `cargo deny` reports a licence this tree may not take, stop and report rather than adding an exception -- `exceptions = []` is a decision, not a default. Then: (1) migration `0006_tokens.sql` adding `token`, `token_network` and `token_capability` as `STRICT` tables -- singular, as every other table in this schema is -- with the all-networks form a column on `token` rather than a sentinel row, and the secret stored only as a SHA-256 hash; bump the schema version constant in `teneral-store/src/schema.rs`. (2) An `actor` column on the event log and a `written_by` column on every resource row, in the same migration, with the comment saying why it is kept on the resource as well as in the log -- the log is evicted and the question outlives it. (3) `Actor` in a new module: a token id, a peer uid, or a named part of the daemon, rendered to one `TEXT` value by a method, never parsed back. (4) An `actor: &Actor` parameter on every mutating method of `BindingStore` and every call site, stamped into both columns in the same statement as the write. (5) A `TokenStore` trait in `teneral-store/src/tokens.rs` with mint, delete, list and authenticate. Four capabilities -- read, write, watch, admin -- because the operator reversed decision 4 in the back brief and admin is now a capability rather than a second socket, and `mint` enforces decision 4's subset rule itself: a scope or a capability its minter does not hold is refused in the store rather than merely checked by the server, because an authorisation rule checked only in the caller is one a second caller will skip. Its doc comment explains why tokens are not `BindingStore` resources and why their mutations are not events -- decision 7 has the argument; do not restate it as fact without the reasoning. (6) Add a sentence to `scripts/check-store-trait.sh`'s header naming the traits it governs and why `TokenStore` is outside them. Tests: a minted token's secret verifies once and its hash is what the table holds; a token scoped to one network does not authenticate into another's; a token holding `admin` on one network cannot mint a token scoped to every network, nor one carrying a capability it lacks; every write records its actor and the actor survives on the resource after its event is evicted. Run `scripts/check-store-trait.sh` and `make test`. Commit subject: "Record who made each write, and mint tokens." |
| 4b | high | opus | none | The three remaining debts the store owes the control plane. Read decisions 6, 8 and 9 and findings 3, 4 and 7. Four changes. (1) `epoch()` returns `Epoch` by value; the concrete store holds it behind a lock because minting replaces it. (2) `live_views()` moves from `SqliteStore`'s inherent impl (`teneral-store/src/sqlite.rs:320`) on to `BindingStore`, and is added to the enumerated store-global paragraph at `teneral-store/src/store.rs:84` that the gate script reads out of the trait's doc comment (`ReadHandle` has its own at `:471`) -- a name enumerated that is not a method, or a method that is not enumerated, both fail the gate, so check it rather than assuming. (3) `subscribe()` returning a `tokio::sync::watch::Receiver<Cursor>` that moves on every mutation, fed from wherever the writer assigns the position so that a receiver cannot see a cursor before the transaction that issued it has committed -- a notification that outran its own commit would send every stream to read a position that is not there yet. (4) `mint_epoch()` returning the first `Cursor` of the new order, updating the single `store_identity` row, and adding `mint` to the verb list at `scripts/check-store-trait.sh:155` in the same commit, because that script's header says a write whose verb is unknown gets only the weaker check. Tests, each with two networks where the store is read: a cursor taken before a mint is reported `EpochChanged` after it, naming both epochs; the sequence counter does not restart, so a position from the old order is not confusable with one from the new; `subscribe()` fires once per mutation and its value is readable by `history_everywhere` the instant it arrives; `live_views()` through `dyn BindingStore` agrees with the concrete count. Run `scripts/check-store-trait.sh` and `make test`. Commit subject: "Let the store be watched and its epoch minted." |
| 4c | high | opus | none | The crate, the schema and the connection. Read decisions 1, 2, 3, 4 and 5 and findings 1, 12 and 15, and `docs/api-design.md:882-1038` for the transport the ruling overturned -- you are building the alternative that note argued against, so read its argument and do not reproduce its conclusion in a comment. **Dependency gate first**, as in 4a: add `teneral-api` to `members`, with `prost`, `prost-build` (build-dependency), `tokio` with the runtime features it needs, `serde` and `serde_json` for decision 11, and `clap` 4 derive in `teneral`; add `protobuf-compiler` to the `Dockerfile`; `make image`, `make lock`, `make deny`. The tokio feature rule in `AGENTS.md:53-65` is about `teneral-store`, which still may not have a runtime feature -- check that your change did not give it one through the workspace. Then: (1) `teneral-api/proto/teneral.v1.proto`, the whole resource and request vocabulary for the five resources that exist, the handshake, the watch events and the refusals, with no message for zones, records or blobs -- an empty message reserved for a resource phase 6 will design is a guess, and `EventKind` has five variants. Field numbering is permanent, so leave gaps where a resource will grow. (2) `build.rs` running `prost-build` into `OUT_DIR`; nothing generated is committed. (3) The `wire` module: length-delimited read and write with the 1 MiB bound checked against the prefix before the body is read, and a refusal rather than an allocation when it is exceeded. No `unwrap`, no slice indexing, no unchecked arithmetic on a length that came off a socket. (4) The one listener at mode 0660, the peer credential read through `UnixStream::peer_cred`, the `Hello`/`Welcome` handshake with version negotiation, the token verified once per connection against `TokenStore::authenticate`, the owning-peer exception that makes bootstrap possible, the single exhaustive request-kind-to-capability function every handler calls, and the startup refusals in decision 4 -- something already answering, a mode that cannot be set, a stale socket file removed rather than inherited. Decision 4 was reversed in the back brief, and this file holds the version that stands; do not build two sockets. (5) A refusal vocabulary: missing capability, outside your scope, no such resource, conflict with the holder named, too many views, unknown resource kind, version mismatch. Tests: a connection whose peer is not the daemon's own uid and which presents no token is refused; the owning peer is served without one; an admin operation is refused to a token without `admin`; a length prefix over the bound closes the connection without allocating; a live daemon on the path stops a second one; a `Hello` for version 2 is refused naming 1 and 2. **Stop here and report.** Do not implement a single resource operation: the schema is reviewed before 4d is briefed. Commit subject: "Put a protobuf schema on a Unix socket." |
| 4d | high | opus | none | The resource operations. Read decisions 3, 11 and 12, `docs/api-design.md:236-440` for the resource model and every identity, `:411-500` for the three paths a consumer learns an address by, and `:718-775` for the diagnostics. Implement, over `dyn BindingStore`: reads and writes for networks, attachments and address ranges; bindings including the assert-and-be-told form that reports a conflict with the holder named and the variant that revokes the conflicting lease in one operation (`create_binding_revoking_lease`, `teneral-store/src/store.rs:324`); leases read and revoked but never created, because derived state is not writable; the lookup by selector and the lookup on an interface; `leases_held_by` as a first-class query rather than a filter over a list; the per-network digest; the explain query; and a diagnostics call carrying the epoch, the schema version, the current cursor, the live view count, the bound, and the cross-network counter that should always be zero. Every collection streams, ended by a marker (decision 3). Every response repeats the network it was true of and the cursor it was read at -- not because a caller cannot remember what it asked, but because a body naming another network is how a consumer catches a cache keyed by address instead of by `(network, address)`. Both store-global reads get the scope check in decision 12, refusing without naming the network. Use one converter per resource kind, written here and reused by 4e (decision 11). Tests: two networks holding the same address, every read and write asserted to answer from its own; a token scoped to one network refused on the other for each operation in turn, including the two store-global ones; a conflict names the holder; a revoke-in-one-operation leaves no state in which the address was taken and the binding not written. Commit subject: "Serve the store's resources over the socket." |
| 4e | xhigh | opus | none | The watch primitive, and the view bound. The hardest step in the phase: a defect here is a consumer holding a wrong answer forever with nothing to tell it. Read `docs/api-design.md:531-747` in full -- it specifies this almost completely -- and decisions 8 and 10, and finding 2, which is the reason this step translates rather than invents: `History`'s four variants at `teneral-store/src/history.rs:276-345` are the wire's resume and three resync reasons. Build: (1) the pump, one task per daemon, waiting on `subscribe()`, reading `history_everywhere` once per movement, broadcasting on a channel of 1024. (2) The stream: subscribe to the broadcast **before** the snapshot or catch-up read, then discard any broadcast event at or below the last cursor delivered. Write the comment that says why that order and not the other, because the reverse order is the natural way to write it and leaves a gap. (3) A new client gets a snapshot of each network in its scope read on **one** read handle, so a multi-network scope is one consistent view rather than several, then an in-sync marker carrying that cursor, then the stream from exactly it. (4) A returning client gets resume or one of the three resyncs, each named, in band as data on the stream and never as a connection error. (5) A lagging receiver is disconnected with a final event; `RecvError::Lagged` is the trigger and nothing drops an event silently. (6) The heartbeat with the current cursor. (7) Unknown resource kinds rejected, not ignored. (8) The semaphore of `--max-live-views` permits around every `read()` in the crate, with the named refusal and a retry hint. Tests, all with two networks: a stream scoped to one network sees none of the other's events, including on the snapshot; a lease created and released while a consumer was away produces both events on resume, not neither; a cursor from before a mint resyncs with the epoch reason; an evicted cursor resyncs with the eviction reason and a position never issued with its own; no event is delivered twice and none is missing across the snapshot-to-stream join, asserted by a mutation that makes the comparison off by one and a test that catches it; the bound refuses the 65th view and the count returns to zero afterwards. Then run the `mutation-check` skill over the join and the dedupe comparison, and report the script's path and which test killed each mutant. Commit subject: "Deliver the watch stream and bound the views." |
| 4f | medium | sonnet | none | The daemon, and the debugging client. Read decisions 5, 9 and 13. (1) `teneral/src/main.rs` becomes a daemon: `clap` 4 derive for the flags in decision 13's table, open the store with `SqliteStore::with_clock_and_retention` (`teneral-store/src/sqlite.rs:239`) using the retention flags, serve the one socket at mode 0660, and shut down on SIGINT and SIGTERM by stopping the accept loops, closing the streams with a final event, and dropping the store -- whose `Drop` joins its threads (`teneral-store/src/sqlite.rs:403`), so the order matters. Log the epoch, the schema version and the latest cursor at startup, for the reason decision 9 gives. (2) The client subcommands: get a resource, explain an identity on an interface, show the diagnostics, mint a token, mint an epoch, watch a scope and print events. Each is one call over `teneral-api`'s client and a printing of the response; none of them holds logic the server should hold. (3) `--help` names every flag and its default. Tests: the daemon refuses to start when a live daemon already answers on its socket path, and removes a stale socket file rather than inheriting it; it refuses a store path it cannot open rather than creating one silently in the wrong place; a round trip through the real client over a real socket in a temporary directory, with two networks, asserting each answer names its own network. No sweeper: phase 5 chooses that interval, and the master plan's *Future work* says why. Commit subject: "Make teneral a daemon that serves its socket." |
| 4g | medium | sonnet | none | Documentation and fold-back. This step writes no server code. (1) `docs/control-api.md`: the transport and its framing, the handshake, the one socket with its mode and the owning-peer exception, the four capabilities and which operation needs which, every operation with its scope rules, the watch's full contract including the three resync reasons, the token model and its capabilities, the epoch-mint operation and the restore procedure it belongs to, and the numbers in decision 13's table with what each one trades. Write it as a reference for somebody who has no plan in hand. (2) Correct finding 10 at both sources: `ARCHITECTURE.md:79-82` and `docs/api-design.md:102-112`, which say this repository's daemon serves the web interface, when open question 3's 2026-11-23 amendment put that binary in the home IPAM's own repository. Check `docs/api-design.md:1008-1017`, which leans on the same claim to uphold a recommendation that decision 1 has now overturned, and make both facts true at once rather than fixing one. (3) The ruling in `docs/api-design.md` was recorded by the planning commit at all four sites that state the recommendation; check that and do not record it twice. (4) `ARCHITECTURE.md`: the control plane exists, as a crate, with what it serves and what it does not. This is a shape change, so it belongs there. (5) `AGENTS.md`: only the conventions that changed -- three crates rather than two, the actor on every write, tokens narrow and never widen, minting needs the `admin` capability or the owning peer and a minted token cannot exceed its minter, and the rule that a generated protobuf file is never committed. Nothing else; every line there costs context on every task. (6) The "two crates" sweep in all four places from finding 13: `Cargo.toml:2`, `AGENTS.md:9`, `docs/development.md:13`, `ARCHITECTURE.md:43`. (7) `docs/development.md`: `protobuf-compiler` in the image and what regenerates the schema. (8) `README.md` only if the pitch or the links changed -- a new crate is not a pitch change, a daemon that now does something may be. (9) `docs/provenance.md`: the one sentence in decision 14. (10) Fold back into the master plan: phase 4's row, open question 1 marked resolved with its date if the planning commit has not, and *Future work* entries for what this phase deferred -- the blob capability, the authenticated-identity-to-scope mapping, `update_binding_revoking_lease` with what this phase found, and the epoch-detection mechanism this phase declined with decision 9's reasoning. Commit subject: "Document the control API and close the phase." |

## Risks and mitigations

**The ruling's cost lands in a repository this plan cannot
test.** Decision 1 moves the browser's translation into the home
IPAM. If that translator re-implements scoping, the project's
central failure mode ships in the component nobody tests with two
networks -- which is exactly what phase 1e predicted of a proxy.
*Mitigation, and it is partial on purpose:* this phase makes the
client's types unable to name a resource without its network, and
makes every response repeat the network and the cursor, so a
translator that drops a scope produces a mismatch its own tests
can see. The management session records the risk in the master
plan's *Future work* so that the IPAM's own plan inherits it as a
requirement rather than as a hope.

**Sixteen signatures change in 4a, and the gate is the only
thing watching.** An actor parameter inserted across every mutating
method is mechanical, and the mechanical edit that drops a
`network` parameter on the way through is the defect this project
exists to prevent. *Mitigation:* `scripts/check-store-trait.sh`
fails on exactly that, the management session runs it on the diff
rather than trusting the report, and runs it once against a
deliberately broken copy to confirm it still fails -- a gate
nobody has seen fail is a gate nobody knows is running.

**The snapshot-to-stream join is where a silent gap lives.** The
natural way to write it -- read, then subscribe -- loses every
event in between, and the loss is invisible until a consumer acts
on state it never received. *Mitigation:* decision 8 fixes the
order, 4e's brief requires the comment saying why, and the
definition of done requires a mutation that makes the dedupe
comparison off by one together with the test that catches it.

**The notification can outrun its own commit.** A `subscribe()`
that fires before the transaction is durable sends every stream
to read a position that is not there, and the symptom is an
intermittent empty read rather than an error. *Mitigation:* 4b's
brief requires the signal to be sent from where the position is
assigned and after the commit, and a test that reads immediately
on the signal and finds the event.

**`protoc` in the image, and offline builds.** Compile targets
run `--network none`; only image builds and `make lock` reach the
network. A `build.rs` that needs a tool the image lacks fails
every build, and a vendored compiler crate would be a second way
to get one. *Mitigation:* 4c's dependency gate builds the image
first and proves a compile works offline before any code depends
on it.

**The dependency tree roughly doubles.** prost, serde, serde_json,
clap and tokio's runtime features arrive at once, under an
allow-list with no exceptions and `multiple-versions = "warn"`.
*Mitigation:* both dependency gates run `make deny` before code
is written, and a licence this tree may not take stops the step
rather than earning an exception.

**A store-global read is a cross-network leak with a URL.**
Decision 12 covers the two that exist. The risk is the third one
somebody adds later. *Mitigation:* the refusal is written once in
a helper both callers use, and `docs/control-api.md` states the
rule for whoever adds the next one.

**This phase is large.** Seven steps, a new crate, a migration, a
reopened trait and the hardest streaming problem in the project.
*Mitigation:* the gate between 4c and 4d, strict sequencing, one
commit per step, and the management session reproducing the
claims that matter rather than reading reports -- the discipline
that found five wrong premises in phase 3.

## Definition of done

Every item is checkable, and the management session runs it
rather than ticking it.

1. `make test` passes, and the count is recorded in the phase's
   execution notes.
2. `make lint` is clean with `-D warnings`.
3. `make deny` reports advisories, bans, licences and sources ok,
   and `deny.toml`'s `exceptions` is still `[]`.
4. `pre-commit run --all-files` passes.
5. `scripts/check-store-trait.sh` passes with every new method,
   reports the new store-global names as enumerated, and knows
   the `mint` verb. A copy of the trait with one `network`
   parameter removed still fails it, and the output is recorded.
6. `Cargo.lock` resolves every new dependency, and the MSRV at
   `Cargo.toml` is still the only place the floor is written
   outside the lockfile. If a dependency raised it, the raise is
   in the same commit and the comment says what needed it.
7. The build image carries `protobuf-compiler`, and a compile
   target still runs with `--network none`.
8. No generated protobuf file is committed:
   `git ls-files | grep -c '\.rs$'` over the API crate matches
   the hand-written files, and `teneral.v1.rs` is absent from the
   tree.
9. `teneral-store` still declares no tokio runtime feature in its
   own `[dependencies]`, **and** `make lint` compiles that crate on
   its own as well as the workspace, so `spawn_blocking` still
   cannot compile in it. Both halves are required, and the second is
   this phase's finding rather than an extra precaution: cargo
   unifies a dependency's features across everything it builds at
   once, so once a sibling crate takes a runtime feature the feature
   list alone no longer holds the phase-3 guarantee in a
   `--workspace` build. Checked by writing a `spawn_blocking` call
   in the store and confirming the per-crate check refuses it while
   the workspace build accepts it.
10. Every mutating store method takes an actor, and a write's
    actor is readable from the resource after its event has been
    evicted.
11. A minted token's secret appears nowhere but the one response
    that returns it: the table holds a hash, and `gitleaks`
    passes over the tests that mint one.
12. A token scoped to one network is refused on another for every
    operation, including `explain` and the interface-owner query,
    and the refusal does not name the network it refused.
13. Two networks holding the same address, asserted for every
    read, every write and every watch scope in the phase. This is
    the project's central correctness property and one network
    proves nothing about it.
14. A lease created and released while a consumer was away
    produces both events on resume.
15. A cursor from before a mint resyncs with the epoch reason; an
    evicted cursor with the eviction reason; a position never
    issued with its own. Three tests, three reasons, not one
    message.
16. Nothing is delivered twice and nothing is missing across the
    snapshot-to-stream join, proved by mutation: the mutant that
    makes the comparison off by one is killed by a named test,
    and the run is recorded with the script's path.
17. A reader that falls behind is disconnected with a final event
    and no event is silently dropped.
18. The 65th concurrent view is refused with the named refusal,
    and `live_views()` returns to zero afterwards.
19. A length prefix over 1 MiB closes the connection without
    allocating the body.
20. The daemon refuses to start when something already answers
    on its socket path, removes a stale socket file rather than
    inheriting it, and creates the socket at mode 0660.
21. Every admin operation is refused to a token without the
    `admin` capability, a token cannot mint a scope or a
    capability its minter lacks, and the request-kind-to-
    capability function is exhaustive, so an operation added
    without a capability does not compile. These three are what
    stand in for the file mode the back brief's reversal
    removed, which is why they are checked rather than assumed.
22. A streamed resource's message equals the message a read of
    that resource returns, for all five kinds.
23. `docs/control-api.md` exists and every operation in the
    `.proto` appears in it, checked by
    `scripts/check-api-docs.sh`, which fails when an operation is
    documented and absent or present and undocumented. The script
    is run both ways to prove it fails.
24. No sentence in `README.md`, `ARCHITECTURE.md`, `AGENTS.md` or
    `docs/` says the workspace has two crates, and
    `grep -rn 'two crates\|two members'` returns only accurate
    sentences.
25. No document says this repository's daemon serves the web
    interface, and `docs/api-design.md` states the ruling at every
    place it states the transport recommendation.
26. `docs/provenance.md` has 28 rows still, and its header scopes
    its rule to the protocols teneral did not invent.
27. The master plan's phase 4 row, open question 1, and the
    *Future work* entries this phase closes or rewrites are all
    updated, and every entry this phase defers is written in full
    rather than named.
28. Nothing this phase started is still running: no teneral
    process, no container, no `cargo` on the host.

## Back brief

Points for the operator before any step runs. The transport and
the trait reopening were settled on 2026-11-25 and are recorded
in decisions 1 and 6; what remains is the shape, which is cheap
to agree now and expensive to redo.

1. **The schema gate.** 4c stops after writing the `.proto` and
   before any operation is generated from it. Field numbers are
   permanent and every later step depends on the shape, so the
   schema is reviewed before 4d is briefed. Confirm that the gate
   stays.
2. **Two sockets rather than one.** Decision 4 splits the
   operator's operations from the clients' by file mode, since
   the ruling removed the header-trusting listener the design
   note split. Confirm, or say that one socket with a capability
   is enough.
3. **The daemon does not detect a restored file.** Decision 9
   declines the mechanism the *Future work* entry asked about,
   because any detector's state is captured by the same backup
   that rewinds the store. Confirm the decline, since the entry
   calls this the most serious item in the section and declining
   it deliberately is a different thing from leaving it.
4. **The numbers in decision 13.** Retention of 50,000 events or
   six hours, 64 views, 256 connections, a 15-second heartbeat, a
   1 MiB message. Each is a flag, so none is permanent, but the
   defaults are what a deployment gets.
5. **Phase 4 ships no sweeper**, so an expired lease is never
   removed until phase 5. Confirm that is the right place for the
   interval, given it is also the address-stickiness knob.

## Execution notes

**Back brief answered 2026-11-25 (Australia/Melbourne).** Four
questions, three of which confirmed the plan and one of which
reversed it.

- **The 4c schema gate stays.** The step stops with the `.proto`
  written and only the handshake generated from it, and the
  management session reports the schema before 4d is briefed.
- **Decision 4 is reversed: one socket, with admin as a
  capability on a token**, rather than an admin socket and a
  client socket distinguished by file mode. The decision is
  rewritten in full above rather than annotated, because a step
  that read the old text would build the wrong thing. Three
  checks were added to the definition of done to stand in for the
  boundary the kernel used to enforce: an exhaustive
  request-kind-to-capability function, the subset rule on minting
  enforced in the store, and a refusal test per admin operation.
  Decisions 3, 5, 7, 12 and 13 and the briefs for 4a, 4c, 4f and
  4g were corrected in the same commit, and the capability
  vocabulary grew from three to four.
- **The decline in decision 9 is confirmed**: the daemon does not
  try to detect a restored database file, and 4g rewrites the
  master plan's *Future work* entry to record that this phase
  considered and declined the mechanism.
- **The defaults in decision 13 stand as planned**, including
  that this phase ships no sweeper, so nothing removes an expired
  lease until phase 5 chooses that interval knowing it is also
  the address-stickiness knob.

**4a, reviewed and committed 2026-11-25.** Two things the review
sent back to the agent, both kept in the step's one commit. The
first: attribution was write-only as first implemented -- the
column recorded who made every write and no read selected it, so
the question it exists to answer was unanswerable. `Event` now
carries the log's actor, which is the only place a *deletion's*
actor can be named at all, since a deletion's state is absent, and
the five resource models carry `written_by` with the same spelling
in the event state's JSON. The second: `delete` and `list` are
scoped in the store rather than left to the server, for the reason
`mint` already was -- an authorisation rule checked only in the
caller is one a second caller will skip. The tables were also
renamed to the singular to match every other table in the schema,
which was carelessness in this plan rather than a decision, and
this file now names what was built. Two extras of the agent's were
kept: the sentence in `ARCHITECTURE.md` that this step made false,
and a table count in `docs/binding-store.md` that said seven while
listing eight before this phase touched it.

**4c found that the phase-3 tokio guarantee was weaker than it
read.** The rule that the store owns the threads its blocking
happens on was said to be enforced by tokio's feature list -- the
store takes `sync`, so `spawn_blocking` does not exist in it. Cargo
unifies a dependency's features across everything it builds at
once, so `teneral-api` taking `rt` for its connection tasks puts
`spawn_blocking` back within the store's reach in any
`--workspace` build. The step proved both halves: with a
`spawn_blocking` call written into the store, `cargo check -p
teneral-store` refuses it and `cargo check --workspace` accepts it.
The remedy is mechanical rather than a note asking for care --
`make lint` now compiles the store on its own as well as the
workspace, which is the one invocation where the crate gets only
its own features. `AGENTS.md`'s invariant and item 9 of the
definition of done above were both rewritten to say what is now
true, including the trap, because an invariant that claims an
enforcement it does not have is worse than one that asks for
vigilance.

**4d found a defect in the schema the gate accepted, and the gate
is where it should have been caught.** The whole point of stopping
4c with the `.proto` written and reviewing it before 4d was briefed
was to catch exactly this, and the review read the message's three
fields without asking what the operation's network was for.
`LookupBindingOnInterfaceRequest` carried no network, so the step
built the resolve-then-refuse treatment for it as if it were a
third store-global read. What that would have cost is not the
handler, which worked: it is that
`cross_network_answers` -- the counter that should always be zero,
and the alarm this project would rather have than any dashboard --
becomes unreachable from the wire. The store refuses and counts
when a caller's network and an interface's network disagree, and a
message with one network in it cannot disagree with anything. The
guard would have stood for the life of the daemon able to fire only
on a defect inside the store, which is the half of the failure
class least likely to happen and the half the store's own tests
already cover. The field was added as `network = 4`, the operation
became an ordinary network-named one, `interface_owner` was added
to `ReadHandle` so that the two reads whose answer *is* a network
can be resolved from the position the rest of an answer is read at,
and two tests were added over the socket: a lookup naming one
network for another's interface is refused without naming either
and the counter moves, and the ordinary case answers from the
network the caller named.

**4e, reviewed and committed 2026-11-26.** The step found that the
heartbeat as this plan and `docs/api-design.md` both specified it --
"the current cursor" -- is the unsafe reading of the one mechanism
whose job is to stop a consumer falling out of retention. Both are
corrected above and at their source, and the stream keeps two
positions rather than one: what the consumer holds, which a resync
and a disconnection report, and what it may resume from, which the
heartbeat carries. Its mutation run is worth recording in full
because of what it caught: the mutant that moves the subscription
after the read -- the gap the whole join is arranged to prevent --
**survived** the first attempt, because the pump's own lag
re-broadcast the missing events and a concurrent-writes test could
not see the difference. The step then built a deterministic window
(a snapshot larger than any socket buffer, one event read to prove
the view is open, writes during it) and the mutant died. A test
that cannot distinguish the defect from the repair is not a test of
the defect, and only the mutation run said so. Two of the required
tests are in-crate rather than over the socket -- the lagging
reader and the view bound -- because the socket versions would be
timing-dependent; a socket test covers the bound's refusal reaching
a caller with its retry hint.

**Not a phase-4 change, recorded here because it landed on this
branch.** `bluecatengineering/dhcproto#111`, the `hlen` clamp this
project filed on 2026-11-22, merged upstream on 2026-11-24 as
`6194561`. The master plan's *Future work* entry and the staged
work's disposition table both said it was unreviewed and
unmerged, and now say otherwise. The guard phase 5 owes is
unchanged, because 0.15.0 is still the only release and still
carries the defect.

**4f chose where the daemon logs, and phase 7 revisits it.** The
daemon writes one line per message to standard error and uses no
logging crate. A systemd unit captures standard error into the
journal, which supplies the timestamps and the filtering, and which
logging framework to carry is a packaging decision rather than a
reason to add a dependency to a daemon that can already be run and
read. Phase 7 owns packaging and so owns the revisit. The daemon is
the `serve` subcommand rather than the bare binary, because the
client subcommands share the binary and a bare invocation that
started a daemon would be one typo from a second one.
