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
     thread.

     Because a handle costs a thread, the number alive at once
     wants bounding. This sentence first said the bound was the
     store's to enforce and that step 3e would name it, which was
     an error in writing this decision: 3e's row says nothing
     about it, so the promise was made to a step that was never
     told to keep it, and 3e correctly reported the
     inconsistency rather than inventing a policy. **Settled as
     phase 4's**, with the store doing the half that is
     genuinely its own: `live_views()` reports the count, so the
     bound is enforceable from above. Refusing to open a view
     needs a policy and an error a consumer expects, and both of
     those are the control plane's vocabulary rather than the
     store's -- phase 4 is also the layer that knows how many
     concurrent consumers there are. Recorded in the master
     plan's *Future work* so it cannot be lost between the two.

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
   produces the duplicate.

   One thing step 3a established that this decision assumed the
   other way. The bundled engine already defaults to `FULL`,
   under WAL as well as under rollback, because
   `libsqlite3-sys` 0.40.0 moves neither
   `SQLITE_DEFAULT_SYNCHRONOUS` nor
   `SQLITE_DEFAULT_WAL_SYNCHRONOUS` off 2. That makes the pragma
   line defensive rather than corrective, and it makes the
   obvious test of it worthless: it passes with the line deleted.
   The line stays, because the alternative is an invariant
   resting on a compile flag inside a dependency's build script
   that nobody reads and that `bundled` is what pins -- and
   decision 2's own risk row contemplates dropping `bundled`,
   which is the day this would go quiet. The guard is a test that
   turns a WAL connection's `synchronous` down and asserts the
   pragma set brings it back; the ordering that makes it a real
   guard is in the test's own comment. The cost is one fsync per lease
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

12. **Selector precedence is decided here and documented in
    `docs/api-design.md`. Most of it gets no provenance row; one
    step of it does.** This decision was first written as a flat
    claim that no RFC fixes an order among MAC, client-id,
    DUID/IAID, user-class and vendor-class. **That claim is
    wrong, and step 3c's review is where it was caught** -- which
    is worth recording as a lesson about a premise that was
    convenient and went unchecked for two phases.

    RFC 2131 section 4.2 is normative on one of the pairs: where
    a client supplies a client identifier, the server MUST use
    that identifier to identify the client, and the contents of
    `chaddr` are the MUST-use fallback only where no client
    identifier was supplied. So the client identifier outranking
    the hardware address is a specified rule rather than
    teneral's choice, and it is owed a provenance row.
    `selector-client-id-over-chaddr` is that row, and it is the
    file's first RFC-cited one. RFC 2131 section 4.3.1
    additionally requires its configuration-selection rules to be
    applied in the order given, with the client-specific rule
    ahead of the vendor-class rule; that supports
    individual-before-class without settling whether a later rule
    overrides an earlier one, so it is cited for what it covers
    and no further.

    The rest is genuinely this project's own and gets no row:
    where DUID/IAID sits relative to the client identifier,
    whether the user class outranks the vendor class, and the
    override semantics that make this a precedence at all rather
    than a list of rules applied in turn. `docs/provenance.md`
    permits exactly two sources, an RFC section or a named
    observed-behaviour test, and measuring another implementation
    would record that implementation's choice rather than a fact
    about the protocol. A row citing nothing would make that
    file's rule meaningless, and the rule is the only thing
    keeping the project able to say where its behaviour came
    from.

    Address *selection* is different and does have a source. RFC
    2131 section 4.3.1 specifies the order a server considers,
    and it is **four** steps rather than the three first written
    here: the address the client already holds, then the address
    it held under a now-expired or released lease if that address
    is in the pool and not already allocated, then the address it
    requested if valid and not already allocated, then one from
    the pool. The keyword introducing the list is SHOULD, not
    MUST. Step 3d found the omission by reading the section
    rather than this paragraph, which is the second time in this
    phase that a premise about what an RFC says has turned out
    wrong -- see decision 12 above for why that class of claim
    wants checking precisely when it is load-bearing.

    That order gets a provenance row. Two things in teneral's
    implementation of it do not, because the section does not
    settle them: the tie-break within the pool, where teneral
    takes the lowest free address and section 2.2's "least
    recently assigned" is an example rather than a rule; and the
    reading of the section's "valid" as "inside one of the
    network's configured ranges". Both are named as teneral's own
    inside the row rather than left to look RFC-cited.

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
| 3a | high | opus | none | The crate, the dependency and the schema's foundations. Read decisions 1, 2, 5, 7 and 13 and survey findings 4, 5 and 6 first. **Do the dependency gate before anything else:** add `teneral-store` as a second workspace member, raise `[workspace.package] rust-version` to `1.89` with a comment saying rusqlite 0.41.0+ is what needed it, add `rusqlite = { version = "0.43.0", default-features = false, features = ["bundled", "cache", "extra_check"] }`, and per decision 3 add `async-trait` and `tokio = { default-features = false, features = ["sync"] }` as dependencies plus a `tokio` dev-dependency carrying `["rt", "macros"]` to drive tests -- pick the current released version of each and say in the report what resolved. Then run `make lock` then `make build` and `make deny`, and confirm from `Cargo.lock` that the resolved rusqlite is 0.43.x rather than 0.40.2 -- the MSRV raise is the only reason it can be, so a 0.40.x in the lockfile means the raise did not take. The build must succeed through the `Makefile`, which compiles `--network none` against a read-only cargo cache, so the bundled SQLite's C compile has to work offline with no network and no writable cache; if it cannot, **stop and report** rather than switching to the system `libsqlite3`, because that choice is decision 2's and belongs to the operator. Then build the schema's foundations and nothing above them: `PRAGMA user_version` holding a schema version with forward-only migrations applied at open and a refusal (naming both versions) when the database is ahead of the code; a per-connection pragma set that includes `foreign_keys = ON` -- per connection rather than a property of the file, and a test must assert it so that removing the line fails -- plus `journal_mode = WAL` and `synchronous = FULL` on the writer per decision 4. **Note what the step found about the last two**, because the brief originally said `foreign_keys` was "off by default in SQLite" and that is not true of this engine: `libsqlite3-sys` 0.40.0's `build.rs:171` compiles the bundled amalgamation with `-DSQLITE_DEFAULT_FOREIGN_KEYS=1`, and it moves neither `SQLITE_DEFAULT_SYNCHRONOUS` nor `SQLITE_DEFAULT_WAL_SYNCHRONOUS` off 2. So both pragmas are already in force before the store asks, a behavioural test of either passes with its pragma line deleted, and each needs a second test that builds a connection with the setting *off* and asserts the pragma set turns it on. `journal_mode` is the exception and is genuinely guarded, WAL not being a default; a single-row store identity table whose epoch is `lower(hex(randomblob(16)))`, stable across reopen; and the event log table as `seq INTEGER PRIMARY KEY AUTOINCREMENT` with a test that writes rows, deletes them, writes again, and asserts the new sequence exceeds every sequence ever issued. No trait, no resources, no lookup: 3b owns the trait. Verify with `make build`, `make test`, `make lint`, `make deny` and `pre-commit run --all-files`, and paste the rusqlite line from `Cargo.lock` into the report. |
| 3b | high | opus | none | The storage trait, and the first three resources over it. Read decisions 1, 3, 6 and 10 and `docs/api-design.md:776-820`, whose five numbered demands are the specification this step is written against -- the trait's doc comment should say, per method group, which demand it serves, so that phase 8 can tell a deliberate shape from an accident. The trait is **asynchronous** (decision 3, which the operator overturned at the back brief -- read it in full, because it is not merely `async fn` on the signatures: the writer is a thread the store owns and fed by a channel, each live read handle holds its transaction on a thread of its own, and the trait is declared with `#[async_trait]` rather than native `async fn` so that it stays dyn-compatible for phase 8). No `spawn_blocking` anywhere, and `tokio` is used for its channels only. It must give: write operations that return the cursor they were assigned, never a bare `Ok(())`; a read handle that can be held across the caller's awaits, that reports the cursor it reads at, and that gives a consistent view across several reads rather than independent getters -- that handle is the hardest thing in this step, and decision 3 says why the obvious implementation of it does not compile; compare-and-set forms for the operations the API says are single operations; and **a network parameter on every method**, with the deliberately store-global ones (the epoch, the schema version, history reads across a scope) enumerated in the trait's doc comment, because a reader needs to tell those from an oversight. Then implement networks, attachments and address ranges over it, including decision 6's primary key on `(namespace, ifname)` -- and prove the constraint rather than asserting it, with a test that bypasses the trait and inserts the conflicting attachment with raw SQL so the database is what refuses. Every resource write emits an event into 3a's log in the same transaction as the write, never after it. Two networks sharing a subnet is the fixture every test uses from here on; write it once, in a test support module, and use it in this step's tests rather than deferring it to 3c. Keep the connection type and all SQL private to the crate (decision 1). |
| 3c | high | opus | none | Bindings, selector matching, and the two diagnostics that belong to lookup. Read decisions 10 and 12, `docs/api-design.md:195-296` for the resource model and `docs/api-design.md:499-556` for what a consumer needs in order to notice a wrong answer. A selector is a `(kind, value)` pair meaningful only inside a network and is **not** independently addressable, so a binding's identity is `(network, selector kind, selector value)` and there is no table a selector exists in on its own. Client identity is the union of MAC, client-id, DUID/IAID, user-class and vendor-class. Implement the lookup that answers "given these presented attributes in this network, which binding applies", decide the precedence order between kinds, and **return which selector matched** on every lookup result -- that return value is not a nicety, it is the only way a consumer can check that its understanding of a precedence teneral invented matches the daemon's. Document the order in `docs/api-design.md`, in the section that already says the precedence is the API's own, and write no `docs/provenance.md` row for it (decision 12 says why; a row citing nothing would break the file's rule). Also build the explain operation -- given a synthetic identity and an attachment, what would be answered, which selector matched, and in which network, with no packet involved -- because `docs/api-design.md:505-515` calls it the single most valuable diagnostic the API can offer and the front-ends need the same code path anyway. And build decision 10's guard: compare every returned row's network with the one asked for, error, and count. Test it by planting a mismatched row with raw SQL and asserting both the error and the counter; assert the counter is zero after the rest of the suite. Two networks with the same MAC bound to different addresses, each resolving to its own, is the test that matters most here. |
| 3d | high | opus | none | Leases: the lifecycle, allocation, and the durability point. The hardest step in the phase and the one where a defect is both silent and cross-network, so read decisions 4, 8, 9 and 12 in full first. A lease is keyed by `(network, address)` because within one network an address has at most one holder, and a client may legitimately hold two addresses during a renumber -- so do not key it by client, and make "what does this client hold" a first-class query within a network rather than a filter a caller applies to a list, because that query is the thing the existing consumer cannot do today (`docs/api-design.md:261-273`). Implement the state machine with its transitions and the compare-and-set forms the API names as single operations: "create this binding only if no lease holds the address" and "revoke that lease and create this binding" are each one operation, because a read followed by a write is correct only on a single-writer store. Expiry is absolute and lazy (decision 9): a lease past its expiry is expired when read, with no sweeper having run, and allocation must not hand out an address whose lease has expired but not been swept -- write the test that proves it by advancing the injected `Clock` and allocating without sweeping. Address selection follows RFC 2131 section 4.3.1's order, which this brief originally described as three steps and which **has four**: the address the client already holds, then the address it held under a lapsed lease if that address is in the pool and unallocated, then the address it requested if valid and unallocated, then one from the pool -- introduced by SHOULD rather than MUST. The step found that by reading the section, and the correction is kept here rather than only in the notes so that nobody re-derives the wrong list. The lowest free address is the pool tie-break and is **not** from that source; add the provenance row for the RFC-cited order and name the tie-break, and the reading of "valid", as teneral's own (decision 12). The lease id is the sequence at which the holder acquired the address (decision 8), so a renewal keeps it and a reassignment changes it; test both. The durability point is decision 4: the commit happens before anything could ACK, and the store must offer no API that lets a caller get a lease back before it is committed. Finally, run the `mutation-check` skill over the lease module: break each guard in turn, confirm a test fails and that it is the right test, restore from a copy rather than with `git checkout`, run each mutant under `timeout`, and count a timeout as killed. Report the helper's output and the script's path. |
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
   is removed. Demonstrate the failure and restore. Note that
   the test which merely observes enforcement cannot be that
   test -- the bundled engine enforces by default -- so the
   guard is one that starts from a connection with enforcement
   off.
6. `PRAGMA journal_mode` is `wal` and `synchronous` is `FULL` on
   the write connection, both asserted by tests, and each of the
   three settings has a test that **fails when its pragma line
   is removed**. `journal_mode` is guarded by the obvious test;
   `foreign_keys` and `synchronous` are not, for the reason in
   item 5 and decision 4, and each needs the guard that
   constructs the precondition the engine's defaults hide.
   Demonstrate all three failures and restore from a copy.
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
    an RFC section -- the file's first RFC-cited rows. Two are
    expected: `selector-client-id-over-chaddr` citing RFC 2131
    section 4.2, added by 3c, and the address-selection row
    citing RFC 2131 section 4.3.1, owed by 3d.

    No row cites the *unspecified* part of the selector
    precedence: where DUID/IAID sits, user class against vendor
    class, or the override semantics. This item used to say no
    row cites the selector precedence at all, which was wrong for
    the same reason decision 12 was -- see that decision for what
    RFC 2131 section 4.2 actually settles.
21. `make test`, `make lint` and `pre-commit run --all-files` all
    pass, and the differential rig still reports
    `1 case(s) checked, none failed.`
22. No sentence in `README.md`, `ARCHITECTURE.md` or
    `docs/development.md` still says nothing is built, that the
    namespace-socket assumption is unconfirmed, or that the
    binding store is to come.
23. No container or process this phase started is still running.
24. Decision 3's boundary holds, and the check that matters is a
    compile-time one rather than a grep. The `tokio` dependency
    in `teneral-store/Cargo.toml` carries
    `default-features = false` with `features = ["sync"]`, and
    the runtime features appear only under `[dev-dependencies]`;
    `spawn_blocking` does not exist without a runtime feature,
    so a call to it in the crate fails to build with
    `cannot find function spawn_blocking in module tokio::task`.
    Demonstrate that once and restore. The store's own tests
    must also drive it through the async interface rather than a
    synchronous back door, so that no test reaches a blocking
    call the trait does not expose.

    This item was first written as a grep for the token, which
    was a worse check for a reason worth keeping: a grep cannot
    tell a call from a doc comment explaining the rule, so it
    put pressure on the documentation to avoid naming the
    function it was about. The feature list is the real
    guarantee and the prose is then free to be plain.

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

**2026-11-25 (AEDT): step 3a. The dependency gate passed.** The
bundled SQLite's C compile works in the offline container --
`--network none`, read-only cargo cache, rustc 1.100.0 -- so
decision 2 stands as written and no fallback was needed. The
lockfile resolves `rusqlite` 0.43.0 and `libsqlite3-sys` 0.40.0,
which is the MSRV raise having taken effect, and the diff is
purely additive at 148 insertions and no deletions: nothing
already in the tree moved, which is the answer to the risk that
raising the floor would quietly unblock newer versions of
everything else. `async-trait` resolved to 0.1.92 and `tokio` to
1.54.1, both tightened in the manifest to what resolved.

**Two of the engine's defaults are not what this plan assumed,
and the second one was found in review rather than by the
step.** `libsqlite3-sys` 0.40.0 compiles the amalgamation with
`-DSQLITE_DEFAULT_FOREIGN_KEYS=1` (`build.rs:171`), so
enforcement is on before the store asks for it. The step found
that, reported it, and added a guard test that starts from a
connection with enforcement off -- the right fix, because a test
that merely observes enforcement passes with the pragma line
deleted.

What the step did not do was ask the same question of the other
two pragmas, so the management session did. `journal_mode` is
genuinely guarded, WAL not being any build's default.
`synchronous` is not: the same build moves neither
`SQLITE_DEFAULT_SYNCHRONOUS` nor
`SQLITE_DEFAULT_WAL_SYNCHRONOUS` off 2, and removing the
`PRAGMA synchronous = FULL` line leaves all eleven tests
passing. That is the durability point of decision 4 -- the thing
standing between an acknowledged lease and a duplicated address
after a power cut -- resting on an undefended line. Fixed in
this commit with `the_pragma_set_turns_durable_commits_on`,
which turns a WAL connection's `synchronous` down and asserts
the pragma set brings it back; with the line removed it fails
`left: 1, right: 2` while the observing test still passes, which
is the asymmetry that shows which of the two is the guard.
Decision 4, 3a's brief and definition-of-done items 5 and 6 are
corrected at source. The lesson generalises and is worth
carrying into later steps: a pragma this engine already sets is
a line whose test proves nothing unless the test builds the
precondition the default hides.

**Three `unused dependency` warnings are expected until 3b.**
Cargo 1.100 warns that `async-trait` and `tokio` are declared
and unused, because 3a adds them and 3b is what writes the trait
that uses them. They are left unsilenced deliberately: the only
way to quiet them is `[lints.cargo]` across the workspace, which
would hide a real signal permanently to tidy a two-step
transient. `make build` and `make lint` still exit 0, the
warnings being cargo manifest lints that `-D warnings` does not
reach. 3b's review checks they have cleared.

**2026-11-25 (AEDT): step 3b. Decision 3 works as written, which
is the finding that mattered most.** The async trait over
threads the store owns compiles and behaves: `blocking_recv`
needs no runtime feature, so `tokio` stays at
`features = ["sync"]`, and the writer thread and the
thread-per-view both do what the decision said they would. One
mechanical point the decision does not state, found in the
building: a per-view job closure cannot take a
`Transaction<'_>`, because that means naming two higher-ranked
lifetimes inside a boxed `FnOnce`. It takes `&Connection`
instead and the view's thread passes `&*transaction`, so
statements still run inside the view's transaction. Worth
recording because the next person to touch that type will
otherwise try the obvious thing first, as this step did.

Two traits rather than one: `BindingStore` with eleven methods
and `ReadHandle` with seven, a store and a view of a store
being different things with different lifetimes. The
store-global exceptions to demand 4 are `epoch`,
`schema_version`, `read` and `interface_owner` on the first, and
`cursor` and `networks` on the second. `interface_owner` is the
interesting one and its justification is worth repeating here:
its *answer* is a network, so there is no network to ask within,
and it is the resolution step the datapath performs before
anything is scoped. It is also the opposite of the shape demand
4 warns against -- it takes no network and returns the one it
found, so a caller cannot ask it about one network and be handed
another's answer.

**The review made two corrections, both at the root rather than
the symptom.** The first was mine to make because the step had
been put in an awkward position by a check this plan got wrong.
Definition-of-done item 24 was a grep for `spawn_blocking` in
the crate's sources, and a grep cannot tell a call from a doc
comment explaining the rule -- so documenting the rule tripped
the check, and the step contorted the sentence to avoid naming
the function it was about. The real guarantee is better than the
grep anyway: `spawn_blocking` does not exist without a tokio
runtime feature, and the crate has only `sync`, so the call does
not compile. Demonstrated: `cannot find function spawn_blocking
in module tokio::task`. Item 24 is rewritten around that, and
the crate's documentation now names the function plainly.

The second was a tension between two documents that the step
found and flagged rather than quietly picking a side, which was
the right call. `docs/api-design.md`'s resource table gives an
attachment's identity as `(network, namespace, interface name)`,
while decision 6 requires a primary key of `(namespace, ifname)`
with the network an ordinary column. The step implemented
decision 6. Both are right and the table was the loose one: the
network is part of how an attachment is addressed, and the
uniqueness key is deliberately narrower, because a key including
the network would permit exactly the second claim that the
paragraph immediately below the table calls the single most
important validation in the API. `docs/api-design.md` now says
so, in the only row of that table where identity and uniqueness
key differ.

**One thing the module's own claim overstated, now corrected in
place.** The design says the blocking is confined to threads the
store owns. The work is, but both `Drop` implementations end a
thread and join it, and a join blocks whoever is dropping --
which for a view is once per request in phases 4 to 6. What it
waits for is a parked thread waking, a read transaction ending
and a stack unwinding, none of which touches a disk, so it is
not in the class of thing decision 3 was worried about. The
exception is a view dropped while one of its own queries is
still running, which only a cancelled future can arrange, and
that join waits for the query. The join cannot be dropped: the
transaction lives on that thread's stack, so the thread
returning is the only thing that can end it. `sqlite.rs` now
says all of this where a reader of the design will meet it.

**Extras kept, with the reasoning for each.** Range overlap
within a network is refused, which was not asked for and is
right: two ranges in one network that both contain an address
make "the lowest free address" a question with two answers.
Identical ranges in *different* networks are permitted and the
two-network fixture depends on it. Name validation refuses empty
names, names over 255 bytes, control characters, and a `/` in a
namespace or interface name, that being the character joining
them into an event's resource id. Event state is built by
SQLite's `json_object`, so the escaping is the engine's and no
serialisation dependency was added. A `Clock` arrived a step
early because `event_log.recorded_at` has no `DEFAULT`, so
writing any event needs one; minimal, and 3d was going to need
it regardless. `address_range` carries `CHECK` constraints
bounding its addresses to 32 bits, which is decision 5 enforced
by the database rather than by the code that writes it.

Point reads -- only `interface_owner` today -- run on the writer
thread rather than opening a view, so they wait behind whatever
write is in progress. Documented at the call with its cost and
its fix, which is a small pool of reader threads and not a
change to any signature.

Verified in review rather than taken on report: the trait script
catches a write that loses its network parameter
(`create_network ... FAIL takes no network and is not enumerated
as store-global`) and exits 1 on a mutant and 0 on a clean tree,
so it is usable as a gate; 39 tests pass; the attachment
constraint is refused by the database with `UNIQUE constraint
failed: attachment.namespace, attachment.ifname`; and the three
`unused dependency` warnings from 3a have cleared.

**2026-11-25 (AEDT): step 3e. The history's read semantics, the
retention bound, and the digest.** Four answers to a presented
cursor rather than the three the row asked for, and the fourth
is a finding rather than scope creep. `docs/api-design.md` said
the epoch changes when a store is restored from a backup or when
its history is truncated. Neither is implementable here and the
second would be actively wrong: the epoch lives in the database
file, so a file-level restore brings it back with a rewound
sequence counter, and changing it on truncation would resync
every *present* consumer because one absent one fell behind --
against that document's own rule that retention is per daemon
and not per client. So a restore is visible only as a consumer
presenting a position the store never issued, which is a fourth
answer (`NeverIssued`) rather than eviction; reporting it as
eviction would claim something was discarded when nothing was
there, which is the distinction the low-water mark exists for.
Corrected at source in `docs/api-design.md`.

**The review took that finding one step further, and it is the
uncomfortable one.** `NeverIssued` catches a consumer whose
position is ahead of the restore point. A consumer at or below
it is caught by nothing: its cursor is one the restored store
has issued, so it resumes, and it is served different events
under numbers it has already seen. That is exactly the failure
the epoch is named for -- migration `0001`'s own comment claimed
the epoch prevents it -- and against a restore of the same file
it does not, because the restore brings the epoch back. Verified
by reading the code: the epoch is generated once, in that
migration, and only ever read. The comment is corrected to say
what the value does and does not cover, and
`docs/api-design.md` now carries the hole and its remedy, which
is an operator action: after a restore, mint a new epoch before
any consumer reconnects. Nothing offers that operation yet, so
it goes to *Future work* as the fourth item below.

Retention is 50,000 events and seven days, count operative,
parameterised at open with that default. The count leads because
the event rate between a home network and a hypervisor fleet
differs by orders of magnitude: 50,000 is hours at a fleet's
rate and months at a home network's, and 50,000 rows of this
shape is tens of megabytes. The week is a ceiling on a quiet
store's log rather than a resume promise. Truncation is a
sequence floor only, so the retained log is always a suffix and
one number describes what is gone; it runs inside the appending
write's transaction, and a write cannot learn its cursor without
it having run. Parameterising it was a judgement call and the
right one -- otherwise the bound is testable only by writing
50,000 events, and the window is genuinely a property of a
deployment.

The digest is the highest cursor sequence among a network's
*configured* rows plus a count of them, which cannot move when
another network changes because every statement behind it is
scoped by network. Leases are excluded deliberately: the digest
is for comparing configuration a consumer wrote, and a lease
changing on its own would make it change for a consumer that
had written nothing. Returning a configuration to a previous
state returns the digest to its previous value, which is
tested, and two networks' digests are not comparable with each
other, which is documented.

Definition-of-done item 18 is met and better than it asked.
`a_snapshot_is_a_consistent_view_and_a_resume_joins_it_exactly_once`
proves the snapshot does not move while a writer commits, that
the view's own history after its own position is empty, that a
resume from the snapshot's cursor delivers the write exactly
once, that there is no gap (the delivered event's sequence is
the snapshot's plus one), and that resuming again delivers
nothing. Re-run in review against the mutant that matters:
changing the history's `seq > ?` to `seq >= ?` fails it, along
with the single-network scoping test.

**A promise this plan made to a step that was never told to keep
it.** Decision 3, as rewritten after the back brief, said a
bound on concurrently live read handles was the store's to
enforce and that 3e would name it. 3e's row says no such thing.
The step reported the inconsistency instead of inventing a
policy, which was right. Settled in decision 3: the bound is
phase 4's, the store keeps the half that is its own in
`live_views()`, and refusing to open a view needs an error a
consumer expects, which is the control plane's vocabulary. In
*Future work* as the fifth item.

**Departures.** SQL now lives in a third module, `history.rs`,
with `ops.rs`'s rule amended as 3d amended it. `ops::kind` is
replaced by a public `EventKind`, so the log's kind strings are
written and parsed from one list rather than two. Snapshot
states are built by `json_object` a second time, so that a
snapshot event and a live event are interchangeable to a
consumer; that duplication is held by a test comparing them
across all five kinds rather than by construction, which the
step named as the one place here where that is true. `snapshot`
and `digest` refuse an unknown network rather than answering
empty, because a complete snapshot is what a consumer deletes
local state against. The brief's line numbers into
`docs/api-design.md` were stale by then, 3c and 3d having grown
the file -- worth remembering when writing 3f's.

Verified in review rather than taken on report: 113 tests, 40
trait methods with `history_everywhere` correctly enumerated as
store-global, 28 provenance rows, and every gate clean.

**2026-11-25 (AEDT): step 3d. Leases, and a second plan premise
corrected -- this time by the step, before it could become a
wrong provenance row.** The row described RFC 2131 section
4.3.1's address selection as three steps: the address the client
holds, then the address it requested, then one from the pool.
**The section has four.** Between the first and the "requested"
step sits the address the client held under a now-expired or
released lease, if that address is in the pool and not already
allocated. The keyword introducing the list is **SHOULD**, not
MUST. The step read the RFC rather than the plan, implemented
four steps, and said so; the management session verified the
whole passage independently before the row was committed,
because a provenance row asserting a normative order is exactly
the kind of claim this phase has already been wrong about once.

That verification also settled the tie-break. Section 4.3.1 is
silent on which free address the fourth step takes, and section
2.2's "least recently assigned" is introduced with a lowercase
"may", as an example of choosing an address to *reuse* when a
pool is exhausted -- not a rule, and not about this question. So
no RFC is cited for the lowest-free-address tie-break, and the
`lease-address-selection-order` row names the two things that
are teneral's own: that tie-break, and the reading of the RFC's
"valid" as "inside one of the network's configured ranges",
there being nothing else in this daemon for an address to be
valid against.

The state machine has three states and only two of them are
rows: unleased is the absence of a row, held is a row whose
instant is ahead of the clock, and expired is *the same row read
later*, which no write produces. Two transitions are absent on
purpose: no silent change of holder, and no renewal of a lapsed
lease, the address having been free in the interim. Both
compare-and-set forms are genuinely single operations -- "create
this binding only if no lease holds the address" is a property
of `create_binding` itself rather than a second method a caller
could route around, and the revoking form's token is the lease
id, so a renewal in between does not invalidate the instruction
while a reassignment does.

Verified in review rather than taken on report: 98 tests, 36
trait methods, 28 provenance rows, every gate clean. The
mutation run is 21 mutants, all killed, and its log names the
killing tests against an `expect` field naming the test that
*should* kill each, so it establishes that the right test fails
rather than merely that some test does. I re-ran the durability
mutant -- removing the commit from the allocation path -- and
`a_lease_is_committed_before_the_caller_is_told_about_it` fails
as named, along with three others. That test is better than the
row asked for: it reads the lease back through a *second
connection to the same database*, which can only see commits, so
it proves the commit had already happened when the value a
front-end would acknowledge was returned.

**Three things for 3f to promote into the master plan's *Future
work*,** recorded here so that they survive this file:

1. **The sweep interval is also an address-stickiness knob, and
   the two cannot be set independently.** The RFC's second step
   remembers a client's address in the lapsed lease row, and the
   sweep deletes that row -- so however often a later phase
   sweeps is exactly how long a client keeps getting its old
   address back. Phase 5 chooses that interval and should know
   it is choosing both things. Documented in
   `docs/api-design.md`; it wants to be somewhere phase 5 will
   actually look.
2. **A client that stops presenting its client identifier is not
   recognised as its lease's holder.** A lease is recorded
   against the highest-precedence attribute presented, so a
   client that supplied an identifier and later stops supplying
   one looks like a different client. This is honest -- the
   store cannot know that two identities are one machine -- and
   it is the protocol's own warning about keying on a hardware
   address, in reverse. Asserted by a test today and explained
   only in that test's comment. Phase 5 should decide whether to
   mitigate it, and record the decision either way.
3. **There is no `update_binding_revoking_lease`.** The
   specification names only the create form, so the step built
   only that and documented the gap rather than hiding it. Fill
   it when a consumer needs it.

**Departures accepted.** Lease SQL lives in `lease.rs` rather
than `ops.rs`, which breaks that module's "all SQL here or in a
migration" rule; `ops.rs`'s doc comment is amended to say why,
and the lease statements are numerous and cohesive enough that
the split reads better than the rule did. `renew_lease`'s
compare-and-set is on the holder rather than a cursor, because a
renewal arrives from the wire with no position to present. A view
fixes `now` when it opens, so a lease's `expired` is that
instant's reading while `expires_at` is the fact -- the right way
round for a consistent view, and worth knowing before somebody
reads `expired` from a view held open for a long time. The pool
refusing an address a binding reserves was not asked for, is not
specified, and is right: an address held for a machine that is
switched off must not go to somebody else. `acquire` and `sweep`
were added to the trait script's list of mutating verbs, which
strengthens that gate.

**2026-11-25 (AEDT): step 3c. The precedence chosen, and a
decision of this plan's overturned by checking it.** The order
is client identifier, then DUID/IAID, then hardware address,
then user class, then vendor class, generated by one principle:
an attribute naming a single client outranks one naming a class;
among the individual ones the identity the client asserts
outranks the identity of the hardware it happens to be in; among
the class ones the class the deployment set outranks the class
the vendor shipped. The reasoning is in `docs/api-design.md`
along with the argument against it that an operator will make --
that nearly every reservation is written against a MAC, so the
MAC should win -- and why that is rejected.

**The review's substantive finding is that decision 12 was
wrong, and it was wrong in the direction that flatters the
author.** The decision asserted that no RFC fixes an order among
the five selector kinds, and so forbade any provenance row for
the precedence. RFC 2131 section 4.2 says otherwise, normatively
and with a MUST: where a client supplies a client identifier the
server MUST use it to identify the client, and `chaddr` is the
MUST-use fallback only in its absence. So the top pair of this
order was never teneral's to choose, and the plan had instructed
the step not to cite the one source it actually had. Checked by
reading RFC 2131 and RFC 6842 rather than from memory; RFC 6842
turns out to change only whether the server echoes the option
back, not the identification rule.

Three things follow, all done in the review commit.
`docs/provenance.md` gains `selector-client-id-over-chaddr`,
which is the file's first RFC-cited row -- and that file's
header, which said RFC rows would arrive with the first
front-end implementing a specified algorithm, is corrected,
because client identification is settled in the store several
phases before anything parses a packet. Decision 12 now
distinguishes the specified step from the unspecified ones, and
definition-of-done item 20 with it. `docs/api-design.md` cites
RFC 2131 section 4.2 for the pair it settles and section 4.3.1
for the weaker individual-before-class support it gives, and
keeps the "this project's own" claim for what is genuinely
unspecified: DUID/IAID's position, user class against vendor
class, and the override semantics.

The lesson is narrower than "check the RFCs" and worth stating
as what it is: the premise was convenient, it made the decision
cleaner, and it survived two phases unexamined because nothing
in the plan asked anyone to look. A claim that *no* source
exists is the kind that should be checked precisely when it is
load-bearing, and this one was load-bearing twice over -- once
for the precedence and once for a file whose whole purpose is to
say where behaviour came from.

**The step's own best finding: the cross-network guard is
unfalsifiable from data on correctly-scoped SQL.** Every scoped
statement filters on the network, so comparing the returned
row's network with the one asked for is a tautology *while the
statement is right*. That does not make the guard pointless, and
`guard.rs` now says why better than this plan did: the
comparison exists for the statement that stops being right -- a
`WHERE` clause dropped in an edit, a parameter order rotated by
one, an index keyed on an address instead of on a network and an
address. Each is a one-character change that every
single-network test passes, and each now becomes a refused read
and a non-zero counter instead of a wrong answer. Demonstrated
in review by dropping `network = ?1` from the binding lookup:
five tests fail and the guard fires `asked: "beta", found:
"alpha"`, which without it would have been a silent
cross-network answer. Restored from a copy.

Decision 10's raw-SQL test needed a read whose scoping is a
*check* rather than a filter, and the step built the one read
with that shape rather than contriving one:
`lookup_binding_on_interface`, which takes both a network from
the caller and an arrival interface whose network comes from the
store, so the two can genuinely disagree. That is exactly the
quantity `docs/api-design.md` asks to be counted -- a request
whose arrival interface resolved to one network while the answer
came from another -- and it is the shape the API's own URL
implies. Kept, as an addition the row did not ask for and should
have.

**A second document corrected at source.**
`docs/api-design.md` said a binding whose address falls outside
every range in its network is refused. The step did not
implement it and flagged it, correctly: the same document calls
a static-only network with no ranges at all Shaken Fist's
ordinary case, so that rule would refuse every binding in such a
network, and even with a pool a reservation outside it is the
usual way to hold an address the pool can never hand out. The
rule is struck with the reasoning left in place, and the
decidable conflict in that area -- a binding against an address
a lease holds -- is named as 3d's.

**Extras and limits accepted.** No uniqueness on `(network,
address)`, because the store cannot distinguish two selectors
for one machine, legitimately sharing an address, from two
machines colliding on one. Only MAC values are normalised, the
other four kinds being opaque byte strings with no canonical
text form, so normalising them would merge values an operator
meant to keep apart. `selector_kind` is a SQL `CHECK` list
duplicating the Rust enum, held together by a raw-SQL test over
every known kind, because a misspelled kind would otherwise be a
configured answer that is silently never consulted.
`delete_network`'s refusal now counts bindings among the
children in the way. Schema version is 3.

Verified in review rather than taken on report: 67 tests pass,
the trait script passes at 27 methods across both traits with
`explain` correctly enumerated as store-global, the differential
rig sees 27 provenance rows and still reports one case checked
and none failed, and `make deny` is clean on all four checks.

**Earlier extras, from 3a.** `STRICT` on both
tables, which was not asked for and should have been: without it
an INTEGER column stores the text `10.0.0.5` happily and every
comparison against it answers something other than what the
query meant, which matters directly to decision 5. And
`event_log` carrying `network`, `kind`, `resource_id`, `state`
and `recorded_at` with a `(network, seq)` index, rather than the
bare sequence the brief asked for -- taken from the watch
specification so that 3b adds no migration on its first day.
`recorded_at` deliberately has no `DEFAULT`, so that the
injected clock stays the only clock. `teneral` does not yet
depend on `teneral-store`, nothing in the binary having anything
to call; that wiring is 3b's.
