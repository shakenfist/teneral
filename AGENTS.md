# Agent guidance for teneral

Teneral is a daemon that answers a request from an identified
client on a known interface with the content configured for that
client. See `ARCHITECTURE.md` for the shape of the system and
`docs/plans/` for what is being built and in what order.

The Cargo workspace holds four crates: `teneral-store`, the
binding store; `teneral-api`, the control plane over it, whose
schema, server and client are what another repository links;
`teneral-dhcp`, the DHCPv4 front-end, which depends on the store
and not on the API; and `teneral`, the daemon and its debugging
client. No front-end speaks DNS yet, and the repository has no
remote, so no CI lane has ever run. See `docs/development.md`,
`docs/binding-store.md` for the store and `docs/control-api.md` for
the API.

## The invariants

These are the things that cannot be inferred by reading the code,
and each of them has already been decided:

- **No GPL code enters this tree, and no GPL-licensed dependency
  either.** dnsmasq is GPL-2/3 and Technitium DNS Server is
  GPL-3.0. Both may be read to work out what to build. Neither may
  be copied from, including its documentation text. `deny.toml` is
  what proves the dependency half; `docs/provenance.md` is what
  documents the implementation half.
- **Every wire-visible behaviour cites an RFC section or a named
  observed-behaviour test.** Behaviour no RFC specifies is learned
  by running dnsmasq and recording what it does, never by reading
  its source. Add the row to `docs/provenance.md` in the same
  commit as the behaviour. "Match what dnsmasq does" is not an
  acceptable justification, because the only lawful way to know
  that is to measure it. Each row carries a `Row id`, and a case in
  the differential rig cites the ids of the rows it supports; the
  rig fails a case that cites none, or one that is not in the
  table. A wire behaviour with no row is not shipped, and that
  includes a length, an ordering or an addressing rule that merely
  seems incidental.
- **An interface in no network namespace is attached under the
  namespace `root`.** Not `/`, which the store refuses in a name.
- **There is no configuration file.** Teneral is configured
  through its API. This is load-bearing: it is what keeps the
  binding store the single source of truth, and it is what removed
  the need to reproduce dnsmasq's 286-directive option grammar. A
  runtime config-file parser is an architectural regression. (A
  state file the daemon itself wrote is a different thing and is
  fine.)
- **State is per-network unless there is a stated reason it is
  not.** Two networks may legitimately use the same addresses. Any
  cache, index, counter or table keyed by address or MAC rather
  than by `(network, identity)` is a cross-network bug waiting for
  a second network to exist. A single-network test cannot see it.
- **A test that touches lookup or the binding store uses two
  networks holding the same address.** This is the project's
  central correctness property, and one network proves nothing
  about it.
- **No test binds a listening socket in a test process that also
  starts child processes.** Tests are threads of one process and
  `Command::spawn` forks and execs inside one call, so for the
  moment between the two a child holds a copy of every descriptor
  that was open: a listener bound and dropped is still answering, in
  that child, until it execs. `stale_socket` in
  `teneral/tests/daemon.rs` makes a stale socket by killing a real
  daemon instead, and says why.
- **The store's interface is asynchronous, and the store owns the
  threads its blocking happens on.** A caller that reaches for
  `tokio::task::spawn_blocking` around a store call has misread the
  interface: there is nothing left for such a helper to move, and
  wrapping one would put the work on the runtime's shared blocking
  pool, which is the arrangement the design exists to avoid. Two
  things enforce it rather than anybody's vigilance, and it takes
  both: `spawn_blocking` does not exist without a tokio runtime
  feature and `teneral-store` has only `sync`, and `make lint`
  compiles that crate on its own as well as the workspace. The
  second exists because of the trap in the first -- cargo unifies a
  dependency's features across everything it builds at once, so a
  sibling crate's runtime feature puts `spawn_blocking` back within
  the store's reach in a `--workspace` build. The workspace build is
  therefore not where this rule is enforced. A runtime feature
  appearing in that crate's own `[dependencies]` has broken it and
  should fail review.
- **Every mutating store method takes the actor that made the
  write.** It is recorded on the event and on the resource row. A new
  write that does not take one is the one whose author nobody can
  name.
- **A credential narrows and never widens, and `admin` is a
  capability on a token, not a second socket.** One socket serves
  everyone; the only caller that needs no token is the process the
  daemon runs as. A minted token may not exceed its minter, and that
  is enforced in the store. Which capability a request needs is one
  exhaustive `match` in `teneral-api`; do not give it a default arm.
- **A generated protobuf file is never committed.** `build.rs`
  generates it from `teneral.v1.proto` into `OUT_DIR`, and the schema
  is the only copy.
- **A panic is a cluster-wide outage.** One process serves every
  network, so `unwrap()`, `expect()`, slice indexing and
  unchecked arithmetic on anything derived from a packet are
  remote denial-of-service bugs against all networks at once, not
  local untidiness. `unwrap_used` is therefore `deny` rather than
  `warn` in `[workspace.lints.clippy]`, which every crate inherits;
  test code is exempt through `clippy.toml`. Do not relax it to get
  a build through.

## Working here

- Rust, Apache-2.0. A rustup-managed toolchain is deliberately not
  installed on the development host, and the distribution Rust that
  is there is not what this project builds with; build in Docker,
  following the `ryll` and `instar` pattern in this organisation.
- Before proposing a commit, run `pre-commit run --all-files`.
  `ci.yml` runs exactly that command, so a hook added to
  `.pre-commit-config.yaml` is enforced without touching CI, and
  CI and a developer's commit cannot check different things.
  Also `make lint`, `make test` and `make deny`. Never run `cargo`
  on the host: every invocation goes through the `Makefile` into
  the build image, and `make lock` is the only target that may
  write `Cargo.lock`. Clippy warnings are errors; a genuine
  exception carries an `#[allow]` with a comment saying why.
- `PUSH-AUDIT.md` is the pre-push review runbook, and every
  plan's final phase runs it.
- `PLAN-TEMPLATE.md` is the template for a new master plan. Plans
  live in `docs/plans/` with one file per phase, and
  `docs/plans/index.md` carries one row per master plan.
- Shared blocks in `PLAN-TEMPLATE.md` and `PUSH-AUDIT.md` are
  maintained in shakenfist/development and arrive here by template
  re-sync. Do not edit them locally; raise the change upstream.

## Documentation

`docs/development.md` is the reference for the build, the lint
suite and the CI lanes, and for the repository settings that are
configured on GitHub rather than in the tree.

Document user-visible changes in `docs/`. Update this file only
when a *convention* changes, and `ARCHITECTURE.md` only when the
*shape of the system* changes -- a new front-end or datapath does,
a new DHCP option does not. `README.md` changes only when the
pitch, the install story or the documentation links change.
