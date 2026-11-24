# Teneral implementation phase 3: the binding store

## Planning effort

**High**, and the master plan's own rule is what sets it: "High
effort is anything that touches the binding store's schema or its
lookup path". This phase is both of those and nothing else. Four
of its decisions -- the trait's synchrony, the durability point,
where the cross-network guard lives, and which resources the
schema covers -- are cheap to write now and expensive to revisit
once the API and two protocol front-ends are built on them.

## Scope

**In.** The storage layer, as a crate, with a SQLite
implementation behind a trait: the schema and its migrations, the
networks, attachments, address ranges, bindings and leases that
the lookup path needs, the lease lifecycle with its durability
point, the event log that gives the API's cursor a total order,
and the three diagnostics from `docs/api-design.md` that are
store operations rather than wire shapes -- the explain lookup,
the per-network digest, and the counter that should always be
zero. Two-network tests from the first commit, as the master plan
requires.

**Out.** Anything that speaks to anything. No transport, no
listener, no HTTP, no protobuf, no socket: phase 4 chooses the
transport and open question 1 is still unresolved, so a store
built with a transport in mind would be built with the wrong one
half the time. No DHCP or DNS wire handling, no packet parsing
and no `dhcproto` dependency. No namespaces -- phase 8 brings
`setns`, and the store does not know that namespaces exist beyond
carrying one as half of an attachment's identity.

Three resources from the API's model are deliberately deferred
rather than forgotten: zones and records to phase 6, content
blobs to phase 10, and tokens to phase 4. Decision 11 says why.

Also out: the differential rig. Nothing in this phase is visible
on a wire, so there are no bytes to compare, and the master
plan's success criterion is explicit that the rig binds changes
"where the change alters bytes on the wire". The rig must still
pass unchanged, which is a definition-of-done item.

## What the survey found

Checked against the tree on 2026-11-23, at `a6189fc`, and against
the crates.io API the same day.

1. **The master plan's phase 3 sketch is accurate but much
   smaller than the phase.** It asks for
   `(network, interface) x client identity -> facts`, SQLite
   behind a trait, the lease lifecycle and its durability point,
   and two-network tests. Nothing in it is false. But phase 1e
   wrote a specification after that sketch, and
   `docs/api-design.md:776-820` makes five numbered demands on
   the storage layer that the sketch does not mention: a total
   order over mutations assigned by the store, reads consistent
   with a point in that order, compare-and-set on identity, the
   network as a parameter of every trait method, and history
   retention owned by the store. Those five are most of this
   phase's cost. A reader who plans from the sketch alone will
   under-scope it, which is why they are restated here.

2. **`docs/api-design.md` is a far more complete input than the
   master plan implies.** The resource model at lines 195-296
   fixes identities for eleven resources and gives the reasoning
   for five of them. The watch primitive at lines 336-497 fixes
   the cursor as an `(epoch, sequence)` pair and specifies the
   snapshot, the in-sync marker, resume, eviction and resync in
   enough detail to implement the store half without choosing a
   wire format. "Detecting a wrong answer" at lines 499-556 names
   six mechanisms, three of which are store operations rather
   than API shapes. This phase implements against that document,
   not against the sketch.

3. **Phase 3 starts from nothing, as the sketch says.** The
   workspace has exactly one member (`Cargo.toml:8`), the crate
   has no runtime dependencies at all -- `serde` and `toml` are
   dev-dependencies read only by the differential rig
   (`teneral/Cargo.toml`) -- and `teneral/src/main.rs` is 29
   lines that print a version banner. There is no store, no
   schema, no SQL and no trait anywhere in the tree.

4. **The current `rusqlite` cannot be selected at the declared
   MSRV, and the failure is silent rather than loud.** Measured
   against the crates.io API on 2026-11-23: `rusqlite` is at
   0.43.0 (2026-11-15, MIT), and every release from 0.41.0
   onwards declares `rust-version = 1.89.0`. Releases up to and
   including 0.40.2 (2026-08-08) declare no `rust-version` at
   all. The workspace declares `rust-version = "1.85"` with
   `resolver = "3"` (`Cargo.toml:26` and `Cargo.toml:53`), so the
   MSRV-aware resolver will skip 0.41.0 through 0.43.0 and select
   **0.40.2** -- and because 0.40.2 declares no MSRV, that
   resolution *succeeds*. Phase 2's comment on `rust-version`
   anticipated "a resolution failure naming the crate rather than
   a silent lie here"; this is the other case, where the resolver
   quietly pins a release three months old and nothing says so.
   Decision 2 raises the floor to 1.89, which is the mechanism
   that comment describes, used as designed.

5. **`rusqlite`'s default features select the WASM FFI backend.**
   Its `default` is `['cache', 'ffi-sqlite-wasm-rs']`, and
   `ffi-sqlite-wasm-rs` enables `dep:sqlite-wasm-rs`. The native
   backend arrives differently: `libsqlite3-sys ^0.40.0` is a
   **non-optional** dependency under
   `cfg(not(all(target_family = "wasm", target_os = "unknown")))`,
   so it is always present on Linux and `bundled`'s
   `libsqlite3-sys?/bundled` does reach it. A manifest written
   with default features left on would therefore work and would
   also pull a WebAssembly shim into a Linux daemon's dependency
   tree. Decision 2 turns defaults off and names the three
   features wanted.

6. **`deny.toml`'s allow-list is expected to cover the whole new
   tree without an exception.** `rusqlite` 0.43.0 and
   `libsqlite3-sys` 0.40.0 are both MIT; `bitflags`,
   `fallible-iterator`, `fallible-streaming-iterator`,
   `smallvec`, `hashlink` and the `cc` build dependency are the
   usual MIT OR Apache-2.0. All of those are on the allow-list at
   `deny.toml`. This is a prediction from the manifests rather
   than a measurement -- `make deny` in step 3a is the
   measurement, and a licence outside the list is a decision for
   the operator, not a thing to add to `exceptions`.

7. **All 26 rows in `docs/provenance.md` are observed-behaviour
   rows**, and the file says so at lines 32-40: they were all
   taken in the Linux-bridge lab before any code existed. Phase 3
   would add the file's **first RFC-cited rows**, for address
   selection. That is worth noticing because it exercises the
   half of the provenance rule that has never been used, and
   because it is the honest source: RFC 2131 section 4.3.1
   specifies the order a server picks an address in, where
   nothing specifies dnsmasq's tag precedence.

8. **`PUSH-AUDIT.md`'s provenance grep does not cover this
   phase's code, and that is correct.** Line 179 scopes the
   "protocol behaviour changed without provenance" check to
   `teneral/src/dhcp/`, `dns/`, `tftp/` and `wire/`, none of
   which this phase creates. The slice-indexing check at line 157
   is scoped to `teneral/src/` and does cover the store. Said
   here so that a later reader does not "fix" the provenance grep
   to include a storage layer that has no wire behaviour to cite.

9. **Three claims in the tree are stale, and the planning commit
   corrects them at source.**
   - `ARCHITECTURE.md:53-56` says a socket created in a namespace
     keeping its namespace "has not been confirmed
     experimentally yet, and it is the assumption the whole
     design rests on". Phase 1c confirmed it; the measurement is
     the `netns-socket-bound-at-creation`,
     `netns-setns-moves-thread` and
     `netns-held-fd-keeps-namespace` rows in
     `docs/provenance.md`.
   - `README.md:15` says "**Status: nothing is built yet.**"
     Phase 2 shipped the workspace, the build image, the
     `Makefile`, the lint and licence gates, two-tier CI and the
     differential rig.
   - `docs/plans/PLAN-teneral-implementation.md:87` opens
     *Situation* with "Nothing has been built."

10. **A new crate needs no review-scope edit.**
    `.vscode/review-scope.toml` enumerates extensions rather than
    paths, and `*.rs` is in the include list, so a second crate's
    sources enter the review queue automatically. Checked because
    the opposite -- a scope keyed on `teneral/src/` -- would have
    silently excluded the whole phase from review tracking.

11. **This repository's plan index carries one row per master
    plan, not one per phase.** `docs/plans/index.md` says so in
    its own header, and the master plan's *Documentation index
    maintenance* section requires it. So registering this phase
    touches only the master plan's Execution table; the index's
    single row stays `In progress` and is correct as it stands.
    Noted because the `next-phase` skill describes repositories
    that carry a row per phase, and this is not one.

12. **The operator decided on 2026-11-23 that this repository is
    teneral's core libraries, and that the home IPAM becomes its
    own repository.** Recorded here because it lands squarely on
    this phase's first decision: a store that an out-of-tree
    consumer will link is a published interface, not an internal
    module. The planning commit amends open question 3 in the
    master plan, which resolved on 2026-11-21 that the daemon
    serves the web interface itself -- still true, but the daemon
    that serves it is the IPAM's binary built from these
    libraries rather than this repository's. The IPAM's name is
    not settled; `tinyipam` was proposed and `skep` offered as an
    alternative, and nothing in phases 3 to 6 depends on the
    answer.

## Decisions

1. **The store is its own crate, `teneral-store`.** Phase 2
   deliberately left the workspace at one member because a crate
   split "would prejudge phases 3 and 4"
   (`Cargo.toml:4-7`). Survey finding 12 is what settles it: with
   the home IPAM becoming a separate repository, the store's
   public surface is an interface another repository links, so it
   is a crate from the first commit rather than a module promoted
   to one later.

   The reason that matters beyond tidiness is demand 4. "The
   network is a parameter of every trait method. Not a column
   that implementations are trusted to filter on" is only
   enforceable if a caller cannot reach the SQL. Inside one crate
   it is a convention somebody polices in review; across a crate
   boundary with the connection type private, a front-end that
   wants to skip the network has to add a public method to do it,
   which is a diff a reviewer will see. The store's tests become
   integration tests against the published surface for the same
   reason.

   The binary crate `teneral` stays, and depends on the store.
   Nothing else moves.

2. **`rusqlite 0.43.0`, with
   `default-features = false, features = ["bundled", "cache",
   "extra_check"]`, and the workspace MSRV raised to 1.89.**
   Taking the three parts in turn.

   `bundled` compiles SQLite from the amalgamation the crate
   carries, which pins the storage engine's version in
   `Cargo.lock` exactly like any other dependency. The
   alternative links whatever `libsqlite3` the build image
   happens to carry, which makes the engine's version a property
   of the container rather than of the source. In a project whose
   stated failure mode is a silently wrong answer, the version of
   the thing that stores the answers is not something to discover
   at runtime. The cost is a C compile, which is why step 3a
   proves it works in the offline container before anything is
   built on it.

   `extra_check` makes rusqlite return an error when a
   row-returning query is run through `execute()` instead of
   silently discarding the rows. `cache` is the prepared
   statement cache, already a default, and cheap. Everything
   else is off, including the `ffi-sqlite-wasm-rs` default that
   survey finding 5 found selects a WebAssembly backend.

   The MSRV raise to 1.89 is forced by survey finding 4 and is
   exactly the move phase 2's own comment prescribed: "Raise it
   in the same commit as the thing that needed it." 1.89 is well
   below the build image's 1.100.0, so nothing else changes. The
   alternative -- staying at 1.85 and accepting rusqlite 0.40.2
   -- means starting a new subsystem on a release three months
   old, chosen by a resolver rule rather than by anybody, and
   with Renovate unable to move it until the floor rises anyway.

3. **The storage trait is asynchronous, and the store owns the
   threads its blocking happens on.** This decision was first
   written the other way round -- a synchronous trait, with
   phases 4 to 6 reaching it through
   `tokio::task::spawn_blocking` -- and the operator overturned
   it at the back brief on 2026-11-25. The *Execution notes*
   record the change; what follows is the decision as it now
   stands, because a brief that cites "decision 3" must find the
   decision it is being run against.

   So: the trait's methods are `async fn`, every front-end from
   phase 4 onwards awaits them, and no call site outside this
   crate ever names `spawn_blocking`.

   **That is a design and not an annotation**, which is the part
   worth being explicit about. SQLite is a synchronous library;
   rusqlite's `Transaction` borrows its `Connection`, and
   `Connection` is `Send` but not `Sync`, so a `Transaction` is
   not `Send` and cannot be held across an `await` in any future
   a multi-threaded runtime will accept. Demand 2 asks for
   exactly that: "a read handle that can be held and that
   reports its cursor", held across several of the caller's
   awaits. The two cannot both be true in the caller's task, so
   the transaction has to live somewhere that is not a future.
   It gets a thread:

   - **One writer thread**, owning the write connection, fed by
     a channel. A write is a command; its reply carries the
     cursor it was assigned. This is also what makes decision 4
     true rather than hoped for: the single writer is structural,
     and the `fsync` happens on a thread the store owns.
   - **One thread per live read handle**, holding its read
     transaction on that thread's own stack for the handle's
     lifetime. WAL makes concurrent readers free. The handle is
     an async object whose methods are channel round-trips; the
     cursor it reports is the one captured when its transaction
     opened, and dropping the handle ends the transaction and the
     thread. Because a handle costs a thread, the number alive at
     once is bounded and the bound is the store's to enforce --
     3e names it.

   Note what this preserves from the decision it replaces, since
   that reasoning was sound and only its conclusion was wrong.
   The worry was a hidden `fsync` on a tokio worker thread
   stalling every other network's request scheduled on that
   thread -- the one-network-harms-another failure, invisible in
   a single-network test. Threads the store owns are *better* at
   that than `spawn_blocking` was: tokio's blocking pool is
   shared with every other blocking call in the process, and a
   dedicated writer thread cannot be crowded out of it. What the
   old decision bought was that the blocking was visible at each
   call site. What this one buys is that it is visible in one
   place, and that no caller can forget.

   Two mechanical consequences, both of which 3a must land:

   - **`#[async_trait]`, not native `async fn` in trait.** The
     native form returns an opaque future that is not declared
     `Send`, and bounding it as such in generic code needs
     `return_type_notation`, which is unstable; it is also not
     dyn-compatible, and phase 8 exists to put a second backend
     behind this trait. `async-trait` boxes one future per call,
     which is an allocation next to a channel round-trip and an
     `fsync`, so the cost is not worth measuring.
   - **The store depends on `tokio` for channels only**, with
     `default-features = false, features = ["sync"]`, and spawns
     `std::thread` rather than calling `spawn_blocking`.
     `tokio::sync` needs no runtime, so the store crate picks no
     runtime for the daemon and phase 4 stays free to. A runtime
     appears only in the store's dev-dependencies, to drive its
     tests.

   What would overturn it: a measurement in phase 5 or 6 showing
   the channel round-trip dominating the per-request cost, which
   would argue for a synchronous fast path beside the async
   trait rather than for reversing it; or a thread-per-handle
   count that phase 8's scale makes untenable, which argues for
   pooling handles rather than for changing the interface.

4. **WAL, `synchronous = FULL` on the writer, and the durability
   point is the commit before the ACK.** A lease that has been
   acknowledged to a client and then lost to a power cut is a
   duplicated address: the store will hand the same address to
   somebody else, and the first client will keep using it. That
   is the silent wrong answer in its purest form, so the order is
   fixed -- the store commits durably, and only then does the
   front-end put an ACK on the wire. Never the other way round,
   and never with the commit deferred to a batch.

   `FULL` rather than `NORMAL` because under WAL, `NORMAL` does
   not fsync at commit: it is durable against a process crash and
   not against losing power, and losing power is the case that
   produces the duplicate. The cost is one fsync per lease
   transaction, which the phase does not try to optimise; if it
   proves too expensive for Shaken Fist's rate, phase 8 is where
   that is measured, and the honest fix is batching ACKs rather
   than weakening the commit.

5. **An IPv4 address is one INTEGER.** Stored as its 32-bit
   value in an INTEGER column, not as a dotted quad in TEXT.
   "The lowest free address in this range" is then ordinary SQL
   and range containment is a comparison; as text, both are
   string operations that give the right answer for a /24 and the
   wrong one the first time a range crosses an octet. IPv6 is out
   of this plan, so the column is not widened for it; the
   migration that adds v6 is the place to decide between a
   16-byte BLOB and a second table, and guessing now would be
   decoration.

6. **`(namespace, interface)` is a primary key, not a checked
   constraint.** `docs/api-design.md:236-247` calls refusing a
   second network's claim on an already-attached interface "the
   single most important validation in the API ... the one place
   where a cross-network leak would be *configured* rather than
   coded". So the attachment table's primary key is
   `(namespace, ifname)` with the network as an ordinary column,
   which means the database refuses the second claim whether or
   not any code remembered to look. It is the cheapest structural
   guarantee in the phase, and it is the kind that survives a
   refactor.

7. **The sequence is `INTEGER PRIMARY KEY AUTOINCREMENT`; the
   epoch comes from SQLite's `randomblob`.** AUTOINCREMENT rather
   than a bare rowid because SQLite reuses rowids after a delete,
   and this table is *designed* to be truncated (demand 5). A
   reused sequence number hands a resuming consumer somebody
   else's history while looking entirely well-formed -- the
   failure the epoch exists to prevent, reintroduced one layer
   down. The epoch itself is 16 bytes of `randomblob` hex,
   generated by the engine at store creation, which costs no
   dependency and no code.

8. **A lease id is the sequence at which its holder acquired the
   address.** The API asks only for "an opaque lease id that
   changes whenever the address changes holder"
   (`docs/api-design.md:261-267`). Deriving it from the cursor
   satisfies that by construction, is deterministic in tests, and
   needs no random number generator. A renewal keeps the id
   because the holder has not changed; a reassignment gets a new
   one because acquiring is what issues it.

9. **Expiry is absolute and evaluated lazily; the sweep exists to
   emit events.** A lease carries an absolute expiry as a Unix
   timestamp, which is what dnsmasq does and what
   `docs/provenance.md`'s `lease-expiry-absolute` row records. No
   read and no allocation trusts a sweeper to have run: a lease
   whose expiry has passed is treated as expired at the moment it
   is read, so a sweeper that dies cannot cause a double
   allocation. The sweep then has exactly one job, which is to
   turn those expiries into events so that watching consumers
   hear about them -- it is an observability mechanism, not a
   correctness one, and saying so is what stops a later phase
   from relying on it.

   Time arrives through an injected `Clock`, with a test
   implementation that advances on command. Expiry tests must not
   sleep: a test suite with real waits in it gets made faster by
   somebody eventually, and the thing they shorten is the margin
   the test was measuring.

10. **The cross-network guard fails closed, and counts.** Every
    row the store returns carries the network it came from; the
    store compares that with the network the caller asked for,
    returns an error if they differ, and increments a counter.
    That counter is `docs/api-design.md:548-556`'s "one counter
    that should always be zero", implemented in the store rather
    than in the API because the store is the only layer that
    cannot be bypassed. The error is what makes it a guard rather
    than a dashboard: a mechanism that notices a cross-network
    leak and then serves it anyway has converted a bug into a
    metric.

    The cost is one comparison per returned row, which is not
    worth measuring. The test for it plants a mismatched row with
    raw SQL -- going behind the trait deliberately, which is the
    only way to produce a state the trait cannot -- and asserts
    both the error and the counter.

11. **The schema covers networks, attachments, ranges, bindings
    and leases, plus the event log, and nothing else.** Zones and
    static records wait for phase 6, content blobs for phase 10,
    tokens for phase 4. Each of those three is a resource whose
    real requirements are known only to its consumer: a blob
    capability model designed with nothing serving blobs would be
    a migration waiting to happen, and the API's own note says
    read access to blobs is a distinct capability from read
    access to bindings, which is an authorisation question phase
    4 owns.

    What makes the deferral safe is that the event log is generic
    over resource kind -- a kind is a string and a serialised
    post-state, not a column per resource -- so adding one is a
    migration and a variant rather than a redesign. That is a
    claim rather than a proof; the risk table carries it.

12. **Selector precedence is decided here, documented in
    `docs/api-design.md`, and gets no provenance row.** No RFC
    fixes an order among MAC, client-id, DUID/IAID, user-class
    and vendor-class, and `docs/provenance.md` permits exactly
    two sources: an RFC section, or a named observed-behaviour
    test. Teneral's precedence is neither -- it is this project's
    own choice, which `docs/api-design.md:276-287` already states
    is the API's, published, and not configurable per network.
    Writing a provenance row citing nothing would make that
    file's rule meaningless, and the rule is the only thing
    keeping the project able to say where its behaviour came
    from.

    Address *selection* is different and does have a source. RFC
    2131 section 4.3.1 specifies the order a server considers --
    the address the client already has, then the address it
    requested, then one from the pool -- and that gets the
    provenance file's first RFC-cited rows. The tie-break within
    the pool is unspecified, so teneral's choice of the lowest
    free address is recorded as its own with the RFC cited for
    the part the RFC covers and the tie-break named as ours.

13. **Migrations are forward-only, versioned in
    `PRAGMA user_version`, and a database written by a newer
    version refuses to open.** Opening a store whose schema is
    ahead of the code and interpreting it anyway is the
    silent-wrong-answer failure applied to storage: the columns
    the code knows will mostly still be there, and the ones it
    does not know about will be the ones that mattered. So the
    open path compares versions and refuses, naming both. No
    downgrade path is written, because a downgrade that drops a
    column drops bindings an operator configured.

## Step plan

Steps run **sequentially**. Every one writes to the shared
worktree, every one after 3a depends on the crate and schema 3a
creates, and two steps running `make` or `pre-commit` in one tree
at once would trip over each other's half-finished work. 3a is
also a gate: if the bundled SQLite cannot be compiled in the
offline container, decision 2 is wrong and the phase stops for the
operator rather than working around it.

| Step | Effort | Model | Isolation | Brief for sub-agent |
|------|--------|-------|-----------|---------------------|
| 3a | high | opus | none | The crate, the dependency and the schema's foundations. Read decisions 1, 2, 5, 7 and 13 and survey findings 4, 5 and 6 first. **Do the dependency gate before anything else:** add `teneral-store` as a second workspace member, raise `[workspace.package] rust-version` to `1.89` with a comment saying rusqlite 0.41.0+ is what needed it, add `rusqlite = { version = "0.43.0", default-features = false, features = ["bundled", "cache", "extra_check"] }`, and per decision 3 add `async-trait` and `tokio = { default-features = false, features = ["sync"] }` as dependencies plus a `tokio` dev-dependency carrying `["rt", "macros"]` to drive tests -- pick the current released version of each and say in the report what resolved. Then run `make lock` then `make build` and `make deny`, and confirm from `Cargo.lock` that the resolved rusqlite is 0.43.x rather than 0.40.2 -- the MSRV raise is the only reason it can be, so a 0.40.x in the lockfile means the raise did not take. The build must succeed through the `Makefile`, which compiles `--network none` against a read-only cargo cache, so the bundled SQLite's C compile has to work offline with no network and no writable cache; if it cannot, **stop and report** rather than switching to the system `libsqlite3`, because that choice is decision 2's and belongs to the operator. Then build the schema's foundations and nothing above them: `PRAGMA user_version` holding a schema version with forward-only migrations applied at open and a refusal (naming both versions) when the database is ahead of the code; a per-connection pragma set that includes `foreign_keys = ON` -- off by default in SQLite, per connection, and a test must assert it so that removing the line fails -- plus `journal_mode = WAL` and `synchronous = FULL` on the writer per decision 4; a single-row store identity table whose epoch is `lower(hex(randomblob(16)))`, stable across reopen; and the event log table as `seq INTEGER PRIMARY KEY AUTOINCREMENT` with a test that writes rows, deletes them, writes again, and asserts the new sequence exceeds every sequence ever issued. No trait, no resources, no lookup: 3b owns the trait. Verify with `make build`, `make test`, `make lint`, `make deny` and `pre-commit run --all-files`, and paste the rusqlite line from `Cargo.lock` into the report. |
| 3b | high | opus | none | The storage trait, and the first three resources over it. Read decisions 1, 3, 6 and 10 and `docs/api-design.md:776-820`, whose five numbered demands are the specification this step is written against -- the trait's doc comment should say, per method group, which demand it serves, so that phase 8 can tell a deliberate shape from an accident. The trait is **asynchronous** (decision 3, which the operator overturned at the back brief -- read it in full, because it is not merely `async fn` on the signatures: the writer is a thread the store owns and fed by a channel, each live read handle holds its transaction on a thread of its own, and the trait is declared with `#[async_trait]` rather than native `async fn` so that it stays dyn-compatible for phase 8). No `spawn_blocking` anywhere, and `tokio` is used for its channels only. It must give: write operations that return the cursor they were assigned, never a bare `Ok(())`; a read handle that can be held across the caller's awaits, that reports the cursor it reads at, and that gives a consistent view across several reads rather than independent getters -- that handle is the hardest thing in this step, and decision 3 says why the obvious implementation of it does not compile; compare-and-set forms for the operations the API says are single operations; and **a network parameter on every method**, with the deliberately store-global ones (the epoch, the schema version, history reads across a scope) enumerated in the trait's doc comment, because a reader needs to tell those from an oversight. Then implement networks, attachments and address ranges over it, including decision 6's primary key on `(namespace, ifname)` -- and prove the constraint rather than asserting it, with a test that bypasses the trait and inserts the conflicting attachment with raw SQL so the database is what refuses. Every resource write emits an event into 3a's log in the same transaction as the write, never after it. Two networks sharing a subnet is the fixture every test uses from here on; write it once, in a test support module, and use it in this step's tests rather than deferring it to 3c. Keep the connection type and all SQL private to the crate (decision 1). |
| 3c | high | opus | none | Bindings, selector matching, and the two diagnostics that belong to lookup. Read decisions 10 and 12, `docs/api-design.md:195-296` for the resource model and `docs/api-design.md:499-556` for what a consumer needs in order to notice a wrong answer. A selector is a `(kind, value)` pair meaningful only inside a network and is **not** independently addressable, so a binding's identity is `(network, selector kind, selector value)` and there is no table a selector exists in on its own. Client identity is the union of MAC, client-id, DUID/IAID, user-class and vendor-class. Implement the lookup that answers "given these presented attributes in this network, which binding applies", decide the precedence order between kinds, and **return which selector matched** on every lookup result -- that return value is not a nicety, it is the only way a consumer can check that its understanding of a precedence teneral invented matches the daemon's. Document the order in `docs/api-design.md`, in the section that already says the precedence is the API's own, and write no `docs/provenance.md` row for it (decision 12 says why; a row citing nothing would break the file's rule). Also build the explain operation -- given a synthetic identity and an attachment, what would be answered, which selector matched, and in which network, with no packet involved -- because `docs/api-design.md:505-515` calls it the single most valuable diagnostic the API can offer and the front-ends need the same code path anyway. And build decision 10's guard: compare every returned row's network with the one asked for, error, and count. Test it by planting a mismatched row with raw SQL and asserting both the error and the counter; assert the counter is zero after the rest of the suite. Two networks with the same MAC bound to different addresses, each resolving to its own, is the test that matters most here. |
| 3d | high | opus | none | Leases: the lifecycle, allocation, and the durability point. The hardest step in the phase and the one where a defect is both silent and cross-network, so read decisions 4, 8, 9 and 12 in full first. A lease is keyed by `(network, address)` because within one network an address has at most one holder, and a client may legitimately hold two addresses during a renumber -- so do not key it by client, and make "what does this client hold" a first-class query within a network rather than a filter a caller applies to a list, because that query is the thing the existing consumer cannot do today (`docs/api-design.md:261-273`). Implement the state machine with its transitions and the compare-and-set forms the API names as single operations: "create this binding only if no lease holds the address" and "revoke that lease and create this binding" are each one operation, because a read followed by a write is correct only on a single-writer store. Expiry is absolute and lazy (decision 9): a lease past its expiry is expired when read, with no sweeper having run, and allocation must not hand out an address whose lease has expired but not been swept -- write the test that proves it by advancing the injected `Clock` and allocating without sweeping. Address selection follows RFC 2131 section 4.3.1's order -- the address the client already holds, then the address it requested if free and in range, then one from the pool -- with the lowest free address as the pool tie-break; add the provenance rows for the RFC-cited part and name the tie-break as teneral's own (decision 12). The lease id is the sequence at which the holder acquired the address (decision 8), so a renewal keeps it and a reassignment changes it; test both. The durability point is decision 4: the commit happens before anything could ACK, and the store must offer no API that lets a caller get a lease back before it is committed. Finally, run the `mutation-check` skill over the lease module: break each guard in turn, confirm a test fails and that it is the right test, restore from a copy rather than with `git checkout`, run each mutant under `timeout`, and count a timeout as killed. Report the helper's output and the script's path. |
| 3e | high | opus | none | The event history's semantics, and the per-network digest. Read `docs/api-design.md:336-497` in full -- it specifies this step almost completely -- and demand 5 at `docs/api-design.md:812-820`. Build the store half of the watch contract and none of the wire half: a snapshot read at a single cursor that is a consistent view rather than a walk that races mutations, ending with that cursor so a stream can resume at exactly it with no gap and no overlap; a history read after a cursor that returns events in order, including the intermediate states, because collapsing a create-then-release into nothing leaves a consumer holding a state it will never see end; and the three distinct answers a presented cursor can get -- resume, evicted, and epoch mismatch -- as distinguishable results the API layer can turn into its `resync` event. Retention is bounded by a count of events and by age, with the count as the operative bound, and the truncation is **durable and in the same transaction as the write that triggers it**, with a low-water mark recorded so that "evicted" is distinguishable from "never existed" rather than inferred from an empty result. Every event carries its network, including on a single-network read, and the total order must not be presentable as causality between networks -- say so in the doc comment, because a consumer that reads cross-network causality from the cursor is a bug this layer can prevent only by documenting. Also build the per-network digest from `docs/api-design.md:535-543`: a cheap value per network that changes when its configured set changes, so a consumer compares one value instead of diffing everything. Test that it changes on a binding change in its own network and does **not** change on one in the other network -- that second assertion is the whole point of it. |
| 3f | medium | sonnet | none | Documentation and fold-back. This step writes no store code. Six things. (1) Write `docs/binding-store.md`: the schema with its keys and why each identity is what it is, the five demands and how the trait meets each, the lease lifecycle and its durability point, the retention bound, and the three diagnostics -- a reader who needs the store should not have to read `api-design.md` and the plan to find it. (2) Update `ARCHITECTURE.md` only where the shape changed: the binding store is now real, and the `Related documents` list gains the new page. Do not move reference material into it. (3) Update `AGENTS.md` only if a *convention* changed -- the two-network fixture being mandatory for store tests is one, and so is decision 3's rule in its final form -- the store's interface is async, the store owns the threads its blocking happens on, and a caller that reaches for `spawn_blocking` around a store call has misread it; nothing else. (4) `docs/development.md` must not still describe the store as to come. (5) Add this phase's provenance rows if 3d did not, and check every row the phase added carries its `Row id` and either an RFC section or a named test. (6) Correct the master plan where this phase proved it wrong, add anything declined to its *Future work*, and set phase 3's `Status` to `Complete` with its `Merged` cell; `docs/plans/index.md` stays `In progress`. Run `pre-commit run --all-files`, `make test`, `make lint` and `make deny`, and fix what they report. |

## Risks and mitigations

| Risk | Mitigation |
|------|------------|
| The bundled SQLite's C compile fails in the offline container -- no network, a read-only cargo cache -- and the step works around it by linking the system `libsqlite3`, losing the version pin decision 2 exists for. | 3a's brief makes this the first thing it does and tells it to stop and report rather than substitute. The management session re-runs the one command. The fallback is the operator's decision, not a sub-agent's, because it changes what the storage engine's version is a property of. |
| The trait fits SQLite and only SQLite, which open question 5 already accepts as the risk being taken, and phase 8 has to reopen it at the worst possible moment. | Partly accepted, and narrowed two ways. The trait is written against the five demands rather than against the implementation, and 3b's brief requires the doc comment to say which demand each method group serves -- so phase 8 can tell a deliberate shape from an accident. Demand 2 in particular (a held read handle that reports its cursor) is the one a SQLite-only design forgets, because SQLite gives it away for free; it is called out in the brief for that reason. |
| Decision 3's async trait hides blocking rather than removing it, and an `fsync` ends up on a thread some other part of the daemon needed -- the one-network-harms-another failure, invisible in a single-network test. | This is the risk the decision's first form existed to avoid, and it survives the reversal. The mitigation is that the store spawns its own `std::thread`s rather than using tokio's shared blocking pool, so the writer thread cannot be crowded out of it, and `tokio` enters the store with `features = ["sync"]` only -- which a reviewer can check in one line of `Cargo.toml` and `make deny`'s lockfile diff. A store that acquires `features = ["rt"]` outside its dev-dependencies has broken this and should fail review. |
| Decision 3's read handle is implemented as something that compiles rather than something that satisfies demand 2 -- independent getters that agree most of the time, or a handle that reopens a transaction per call. Either passes a casual test and loses the snapshot contract phase 4 is built on. | Definition-of-done item 18 is the test that catches it: a snapshot taken while a writer commits must not contain the later write. 3b's brief says the handle is the hardest thing in the step and points at decision 3 for why the obvious implementation does not compile, so an agent that finds an easy one has found a wrong one. The management session reads the handle's implementation rather than its tests. |
| Every resource write emits an event, so a step that adds a write path and forgets the event produces a store that is correct and a watch stream that silently misses changes -- the project's signature failure mode, in the layer that is supposed to prevent it. | 3b's brief requires the event in the same transaction as the write, which makes a forgotten event a visible asymmetry in one function rather than a missing call elsewhere. The definition of done asserts that every mutating trait method returns a cursor, which a write that emitted no event cannot do honestly. |
| The schema defers three resources (decision 11) and the event log's generic-over-kind claim turns out false, so phase 6 or 10 has to reshape the log every consumer reads. | Stated as a claim rather than a proof, deliberately. What makes it cheap to test early: the log stores a kind as a string and a serialised post-state, so the first new kind is a migration plus a variant. Phase 6 is the first to try it and should say in its own plan whether it held. |
| Mutation testing on the lease module is run, reports everything killed, and the result is believed without the script being checkable later. | 3d's brief requires the helper's output and the script's path in the report, and the definition of done asks for the same, so the claim is reproducible rather than reported. The management session re-runs one mutant. |
| The MSRV raise to 1.89 silently unblocks newer versions of other crates at the next `make lock`, and an unrelated dependency jumps several releases inside this phase. | The lockfile diff is read in review, not just its exit code. Today the only runtime dependencies are the ones rusqlite brings, so the surface is small; the point of reading it now is that it will not be small later. |

## Definition of done

Each item is checkable by running something, and the check is
named.

1. `Cargo.lock` resolves `rusqlite` to 0.43.x and
   `libsqlite3-sys` to 0.40.x. A 0.40.x rusqlite means the MSRV
   raise did not take effect and survey finding 4's silent pin is
   live: `grep -A1 'name = "rusqlite"' Cargo.lock`.
2. `[workspace.package] rust-version` is `1.89`, and the value
   appears exactly once in the tree outside `Cargo.lock`.
3. `make build` succeeds, which proves the bundled SQLite's C
   compile works with `--network none` against a read-only cargo
   cache, because that is what the `Makefile` does. Record the
   command and its exit status.
4. `make deny` exits 0 with all four checks ok, and
   `deny.toml`'s `exceptions` is still empty -- a licence that
   needed an exception is a decision for the operator, not a
   definition-of-done item to satisfy.
5. `PRAGMA foreign_keys` is on for every connection the store
   hands out, asserted by a test that fails when the pragma line
   is removed. Demonstrate the failure and restore.
6. `PRAGMA journal_mode` is `wal` and `synchronous` is `FULL` on
   the write connection, both asserted by tests.
7. Opening a database whose `user_version` exceeds the code's
   schema version refuses to open and names both versions.
8. The event sequence is never reused: a test writes events,
   deletes them, writes more, and asserts the new sequence
   exceeds every sequence previously issued.
9. Every method on the storage trait takes a network parameter,
   except those the trait's doc comment enumerates as
   store-global. Checkable mechanically: extract the trait's
   method signatures -- which per decision 3 are `async fn`, so
   the extractor must match both forms -- and assert each either
   names a network parameter or appears in the enumerated list.
   Write that script into `scripts/` so it can be re-run, and
   record its output.
10. Every mutating trait method returns the cursor it was
    assigned, rather than `()`. Same script, same run.
11. Claiming an already-attached interface for a second network
    is refused by the database: a test inserts the conflicting
    row with raw SQL, bypassing the trait, and asserts the
    constraint fires.
12. A read that would return a row belonging to another network
    errors and increments the cross-network counter. Test plants
    the row with raw SQL; the counter is zero across the rest of
    the suite.
13. Each of these five surfaces has a test with **two** networks
    holding the **same** address and asserting each gets its own
    answer: binding lookup, the explain operation, lease
    allocation, the per-network digest, and a history read scoped
    to one network. Name the five tests in the commit message.
14. A lease past its expiry is expired when read with no sweeper
    having run, and allocation will not hand out its address:
    proved by advancing the injected clock and allocating,
    without calling the sweep.
15. A renewal keeps the lease id and a reassignment changes it.
    Two tests.
16. Mutation testing over the lease module kills every mutant,
    with the script's path and the helper's output recorded, each
    mutant run under `timeout`, and a timed-out mutant counted as
    killed.
17. Resume, evicted and epoch mismatch are three distinguishable
    results from a presented cursor, each with its own test, and
    the low-water mark is what distinguishes evicted from never
    existed rather than an empty result.
18. A snapshot taken while a writer commits does not contain the
    later write, and resuming from the snapshot's cursor delivers
    it exactly once. One test, and it is the one that proves
    demand 2.
19. The per-network digest changes on a configuration change in
    its own network and does not change on one in another
    network.
20. Every provenance row this phase adds carries a `Row id` and
    either an RFC section or a named test, and at least one cites
    RFC 2131 section 4.3.1 -- the file's first RFC-cited row.
    No row cites the selector precedence (decision 12).
21. `make test`, `make lint` and `pre-commit run --all-files` all
    pass, and the differential rig still reports
    `1 case(s) checked, none failed.`
22. No sentence in `README.md`, `ARCHITECTURE.md` or
    `docs/development.md` still says nothing is built, that the
    namespace-socket assumption is unconfirmed, or that the
    binding store is to come.
23. No container or process this phase started is still running.
24. Decision 3's boundary holds, checked three ways. `grep -rn
    spawn_blocking teneral-store/src` finds nothing; the
    `tokio` dependency in `teneral-store/Cargo.toml` carries
    `default-features = false` with `features = ["sync"]` and
    the runtime features appear only under
    `[dev-dependencies]`; and the store's own tests drive it
    through the async interface rather than a synchronous back
    door, so no test reaches a blocking call the trait does not
    expose.

## Back brief

Three gates, each cheap to answer now and expensive to change
once code is built on it. The first two are the ones that would
cost a rewrite. **All three were put to the operator on
2026-11-25 and answered; the answers are recorded here and the
decisions above are written as they now stand.**

1. **Decision 3: the trait is synchronous, and async front-ends
   call it through `spawn_blocking`.** The alternative is an
   async trait, which reads better at every call site in phases 4
   to 6 and hides the blocking rather than removing it. This is
   the decision most likely to be argued with and the most
   expensive to reverse later.

   **Answered: make it async.** Decision 3 is rewritten
   accordingly, and the rewrite is a design rather than a change
   of signature, for the reason it gives: rusqlite's
   `Transaction` is not `Send`, so demand 2's held read handle
   cannot live in the caller's future and has to live on a thread
   the store owns. The second half of the question -- what the
   blocking is then confined by -- was not put to the operator,
   because once the interface is async only one arrangement
   satisfies demand 2 with rusqlite. It is written down rather
   than inferred for that reason.

2. **Decision 1: the store is a separate crate now.** It follows
   from the 2026-11-23 decision that this repository is teneral's
   core libraries, but it is also the thing phase 2 deliberately
   deferred, so it is worth confirming rather than inferring.

   **Answered: split now**, as written.

3. **Decision 2: the workspace MSRV rises from 1.85 to 1.89**, to
   make the current rusqlite selectable. Phase 2 reasoned about
   that floor carefully and documented what should raise it; this
   is that mechanism firing for the first time, and the operator
   should see it rather than find it in a diff.

   **Answered: raise to 1.89**, as written. Note that confirming
   this also confirms rusqlite as the engine, which is what keeps
   decision 2 intact under decision 3's reversal: the async trait
   is a facade over a synchronous rusqlite, not a reason to go
   looking for an async-native SQLite binding.

Nothing in this phase needs a secret, a token, or access to a
live system. Nothing in it runs dnsmasq or touches the lab.

## Execution notes

Kept as the phase runs, for departures from the plan and for the
things a later reader would otherwise have to reconstruct.

**2026-11-25 (AEDT), before any step ran: decision 3 reversed at
the back brief.** The operator was asked the three gates above
and answered the second and third as written. On the first he
chose the async trait, against the plan's recommendation. The
decision, the two step briefs that cited it (3b for the trait,
3f for the `AGENTS.md` convention), 3a's dependency list, the
risk table and definition-of-done items 9 and 24 are all
rewritten in this commit, at source, so that no brief cites a
decision that no longer says what the brief assumes.

What the reversal costs, recorded now so that a later phase does
not rediscover it as a surprise: the store gains `tokio` (for
`sync` only) and `async-trait` as runtime dependencies, where
the plan had it depending on rusqlite alone; the writer becomes
a thread fed by a channel rather than a borrowed connection; and
a held read handle costs a thread for its lifetime. What it
buys: no front-end in phases 4 to 6 has to remember
`spawn_blocking`, and the store's blocking is confined in one
reviewable place rather than at every call site.

The reasoning the old decision rested on was not discarded,
because it was about where an `fsync` lands rather than about
syntax. It is restated in the new decision and in the risk
table, and definition-of-done item 24 is what now holds the line
it drew: threads the store owns, `tokio` with `features =
["sync"]`, and no `spawn_blocking` in the crate.
