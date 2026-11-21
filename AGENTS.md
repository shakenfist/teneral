# Agent guidance for teneral

Teneral is a daemon that answers a request from an identified
client on a known interface with the content configured for that
client. See `ARCHITECTURE.md` for the shape of the system and
`docs/plans/` for what is being built and in what order.

Nothing is built yet: there is no Cargo workspace, so the Rust
half of the build conventions below describes the intended state
rather than the current one. The repository's own automation --
`pre-commit`, CI, Renovate, the review and retest bots -- is in
place and does apply. See `docs/development.md`.

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
  that is to measure it.
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
- **A panic is a cluster-wide outage.** One process serves every
  network, so `unwrap()`, `expect()`, slice indexing and
  unchecked arithmetic on anything derived from a packet are
  remote denial-of-service bugs against all networks at once, not
  local untidiness.

## Working here

- Rust, Apache-2.0. Rust toolchains are not installed on the
  development host by preference; build in Docker, following the
  `ryll` and `instar` pattern in this organisation.
- Before proposing a commit, run `pre-commit run --all-files`.
  `ci.yml` runs exactly that command, so a hook added to
  `.pre-commit-config.yaml` is enforced without touching CI, and
  CI and a developer's commit cannot check different things.
  Once the workspace exists, also `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test --all`
  and `cargo deny check`. Clippy warnings are errors; a genuine
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
