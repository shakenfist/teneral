# Development

How to build, lint and test teneral, what CI runs, and which parts of
the arrangement live in GitHub's settings rather than in this tree.

The build, the lint suite, the licence gate and the CI lanes are all
in the tree. What is not in place is the GitHub-side half of the
arrangement, because this repository has no remote yet; the last two
sections say what is waiting on that. The
[implementation plan](plans/PLAN-teneral-implementation.md) tracks
the rest.

The workspace has four members. `teneral-store` is the binding
store; [binding-store.md](binding-store.md) describes what it holds
and why. `teneral-api` is the control plane over it -- the schema,
the server and the client -- and [control-api.md](control-api.md)
describes it. `teneral-dhcp` is the DHCPv4 front-end, depending on
the store and not on the API, and [dhcpv4.md](dhcpv4.md) describes
it. `teneral` is the daemon, with client subcommands that reach every
operation in the schema, and it carries the differential rig. Every
`make` target below acts on the whole workspace, so none of this is
per-crate.

`make test` runs in a container with `--network none`, as the invoking
uid and with no added capabilities. It cannot open a socket below the
IP layer -- `socket(AF_PACKET, ...)` returns `EPERM` -- and it has one
interface, loopback. So the half of the datapath that reaches a client
with no address is not exercised by the repository's own test target:
everything that decides what a client is told sits above that line and
is tested, and the reply path is proved in the lab with a real client.
What the test target *can* do is bind a privileged port. The container
has a network namespace of its own, where
`net.ipv4.ip_unprivileged_port_start` is zero, so an unprivileged uid
binds port 67 there. The daemon's `--dhcp-port` exists because a test
must not take a well-known port -- it would collide with the next test
and with whatever else is on the machine -- and not because it may not.
Do not read the host's setting of that sysctl as the container's.

## Rust is built in Docker, not on the host

A rustup-managed toolchain is deliberately not installed on the
development machine, which runs several Rust projects at different
toolchain versions. (A distribution Rust happens to be there; it is
not what this project builds with, and no `cargo` command here is
meant to find it.) The build happens inside a container image the
repository defines, following the pattern
[ryll](https://github.com/shakenfist/ryll) and
[instar](https://github.com/shakenfist/instar) already use: a
`Makefile` wraps `docker run` so that `make lint`, `make build` and
`make test` work the same on a laptop and on a CI runner.

`make help` lists the targets. `make image` builds the image,
`make fetch` populates the dependency cache with the network on, and
every compile target then runs `--frozen` and `--offline` with that
cache mounted read-only, because a dependency's `build.rs` runs
arbitrary code. `make lock` is the only target allowed to write
`Cargo.lock`, so taking a new dependency is a deliberate step rather
than a side effect of a build. In CI the cache has to live outside
the checkout, which `actions/checkout` cleans on every run; override
`CARGO_CACHE` to move it.

The toolchain is pinned once, by the image tag in the `Makefile`.
There is deliberately no `rust-toolchain.toml`: it makes rustup try
to sync a channel, which fails in a container with no network. The
language floor is `rust-version` in `[workspace.package]`, so a
too-old toolchain fails with cargo's own MSRV error instead.

The image also carries `protobuf-compiler`, because
`teneral-api/build.rs` runs `protoc` over
`teneral-api/proto/teneral.v1.proto`. It is installed when the image
is built -- the one place allowed to reach the network -- since every
compile target runs with no network and a build script that had to
fetch a compiler would fail there. **The generated Rust is never
committed.** It is written to `OUT_DIR` on every build, and cargo
reruns the script when the schema changes, so editing the schema and
running `make build` or `make test` is all that regenerates it; a
second copy in the tree would be a copy that can disagree with the
first. After changing the Dockerfile, `make image` rebuilds the
image. To check that the reference page still lists every operation
in the schema, and no others, run `scripts/check-api-docs.sh`; like
`scripts/check-store-trait.sh` it is run by hand rather than from
pre-commit.

`unwrap_used` is `deny` in `[workspace.lints.clippy]`, and every
crate inherits it with `[lints] workspace = true`. A root
`clippy.toml` sets `allow-unwrap-in-tests = true`, so test code is
exempt. `deny` rather than `warn` is chosen so that a bare
`cargo clippy`, and an editor running clippy as you type, report the
call as an error the moment it is written rather than as one more
line of yellow met at push time. It is a clippy lint, so no
`cargo build` evaluates it at any level.

`make test` runs the workspace's tests, among them the differential
rig that compares teneral's bytes with a captured dnsmasq exchange.
[The differential rig](differential-rig.md) describes what a case
asserts, why each one names a row in the
[provenance record](provenance.md), and why its corpus is captured
in a lab rather than from a dnsmasq the rig runs itself.

## Linting

```
pre-commit run --all-files
```

That is the whole gate, and it is what `.github/workflows/ci.yml`
runs. No gating lane names a linter directly, so a hook added to
`.pre-commit-config.yaml` is enforced on the next pull request without
a CI change, and CI cannot end up checking something different from
what a developer's commit checked. The one check CI runs that a
developer's commit does not is the advisory half of `cargo deny`,
which is in a scheduled lane that gates nothing, for the reason
below.

The hooks today:

| Hook | What it reads |
|------|---------------|
| `rust-check` | The workspace, when a `.rs` file changed: `cargo fmt --all --check`, then `cargo clippy --workspace --all-targets -- -D warnings`. |
| `cargo-deny` | `Cargo.toml`, `Cargo.lock` and `deny.toml`, for licences, bans and sources -- so a change to the policy is checked by the policy. |
| `wrapped-strings` | The `.rs` files that changed, for a string literal wrapped across source lines without the `\` that joins them -- which leaves a run of spaces in the middle of a user-visible message that nothing else in this suite can see. |
| `actionlint` | The workflow files, including their embedded shell. Its runner-label list is `.github/actionlint.yaml`. |
| `trailing-whitespace`, `end-of-file-fixer`, `check-yaml`, `check-merge-conflict` | Everything, except that the two rewriting hooks skip weAudit's review marks under `.vscode/`, which are generated without a trailing newline. |
| `shellcheck` | Shell under `scripts/` and `tools/`, with `-x` so a sourced sibling is followed. |
| `gitleaks` | The working tree, for credentials. |
| `skillsaw` | The agent context -- `AGENTS.md` and anything else an agent is handed -- for malformed structure, smuggled instructions and pasted secrets. |

The two Rust hooks call the `Makefile` through
`scripts/check-rust.sh`, so they run in the build image rather than
on the host and there is one description of how a container is
started. Each is gated on the files it has an opinion about: starting
a container costs seconds, and a hook that does it on every unrelated
commit is a hook people learn to skip.

`wrapped-strings` is the one hook here that reads neither Rust as a
language nor a file as a whole: it looks for a run of spaces between
two word characters inside a `.rs` file, which is what a literal
wrapped without its `\` leaves behind, and which no formatter
produces. Two such literals reached one pull request -- the detail of
an API refusal and a line of the daemon's log, both user-visible text
in a stream documented as one line per message -- and neither rustfmt,
clippy nor the tests over those messages noticed, because a test that
asserts a message `contains` a value is satisfied by a message with a
gap in it. A line that aligns something inside a literal on purpose
and trips the check anyway says `aligned-on-purpose` in a comment on
the same line.

One check is run by hand rather than hooked.
`scripts/check-store-trait.sh` reads the binding store's traits and
asserts the two things about their shape that fail silently when they
are broken: that every method names the network it applies to unless
the trait's own doc comment enumerates it as store-global, and that
every mutating method returns the position it was assigned rather than
nothing. It reads the enumerated exceptions out of the doc comment
itself, so the list a reader sees and the list the script trusts
cannot drift apart, and it reports an enumerated name that is no
longer a method there. It parses Rust with `awk`, which is adequate
for a trait of declarations and is not a thing to put in front of
every commit in the repository; run it when the traits change.

`cargo deny`'s advisory check is deliberately not a hook. Its answer
changes when somebody publishes to the RustSec database, so as a
commit hook it would fail a commit touching no Rust at all, for a
reason the committer did not cause. `make deny-offline` is the half
that is a function of `Cargo.lock` alone and so is reproducible; the
full `make deny` runs in `supply-chain.yml` instead.

## What CI runs

Every workflow runs on the Shaken Fist self-hosted runner pools. The
labels this repository uses are declared in `.github/actionlint.yaml`,
so a typo or a retired image fails the lint rather than leaving a job
queued forever.

| Workflow | Trigger | What it does |
|----------|---------|--------------|
| `ci.yml` | Pull requests to `develop`, `merge_group`, and `workflow_dispatch` | Two tiers, described below: the `pre-commit` suite and a debug build on a pull request, the test suite on a merge-queue entry, and the automated reviewer once the lint passes. |
| `supply-chain.yml` | Weekly, pull requests, and `workflow_dispatch` | `make deny` -- every cargo-deny check, advisories included. It appears in no `needs:` list and gates nothing: an advisory published between a pull request opening and its merge is not that pull request's fault, and a gate on it would eject a queue entry for something the author did not cause. |
| `codeql-analysis.yml` | Pushes to `develop`, pull requests, and weekly | GitHub's code scanning. The file is the fleet template verbatim and pins no languages; CodeQL's own autodetection finds Rust. |
| `release.yml` | Tags matching `v*` | The release gate and build. See [releasing](releasing.md). |
| `prune-reviews.yml` | Pushes, and daily | Retires review marks whose file has changed since it was read, and imports shakenfist/development's reviews of the template files copied from there. Needs GitHub-side configuration that does not exist yet; see below. |
| `secret-scan.yml` | Pull requests, pushes to `develop`, and `workflow_dispatch` | `gitleaks` over the branch and the history behind it. Separate from `ci.yml` because it needs the push trigger, and because it must read the prose a path filter would skip. |
| `renovate.yml` | Hourly | Dependency updates, from the policy in `renovate.json`. |
| `export-repo-config.yml` | Daily | Exports this repository's GitHub settings and opens a pull request when they have changed, so the settings below are reviewable in version control. |
| `pr-re-review.yml` | Comment `@shakenfist-bot please re-review` | Another pass of the automated reviewer on a pull request. |
| `pr-retest.yml` | Comment `@shakenfist-bot please retest` | Re-dispatches the test workflow without a new commit. |

`ci.yml`'s two tiers are `lint` and `build` on `pull_request`,
gating `Can enqueue`, and `test` on `merge_group`, gating
`Can merge`. `Can see status` runs on every event and depends on
nothing, so a required check always reports. `workflow_dispatch`
runs both tiers, which is what makes `pr-retest.yml` a full retest.
`test` is in the merge tier for what the suite becomes rather than
what it is: `build` and `lint` already compile the workspace and
`clippy --all-targets` type-checks the test targets, so what `test`
adds is *running* them, and the growth in it is the differential
comparisons against a captured corpus.

A `check_paths` job detects changes that cannot affect the build, and
`build` and `test` skip them -- a skipped job satisfies a required
check, which a trigger-level filter would not. `lint` deliberately
does not consume the filter: it runs the whole `pre-commit` suite,
most of which reads human-written text, so a documentation-only
change is exactly the change those hooks exist to catch, and
filtering it would skip the only check such a change has. The
reasoning behind the fleet's shape is in shakenfist/development's
[expensive-lane-path-filter](https://github.com/shakenfist/development/blob/main/docs/audits/expensive-lane-path-filter.md)
and
[merge-group-cancellation](https://github.com/shakenfist/development/blob/main/docs/audits/merge-group-cancellation.md)
criteria.

None of these lanes has ever run. There is no remote, so every
workflow here is unexercised; `actionlint` checks their structure,
their embedded shell and their runner labels, and nothing checks
their logic until the first push.

## Settings that are not in this tree

These are configured on GitHub. `export-repo-config.yml` snapshots
them daily, which is how a change to one becomes visible here.

| Setting | Value | Why |
|---------|-------|-----|
| Default branch | `develop` | The fleet convention for an active project. |
| `RETEST_WORKFLOW` repository variable | `ci.yml` | `pr-retest.yml` is copied verbatim from the fleet template and defaults to a `functional-tests.yml` this repository does not have. Without the variable, a retest request fails and says so on the pull request. |
| `RENOVATE_TOKEN` secret | An organisation token | `renovate.yml` pushes branches with it. |
| Delete branch on merge | On | |
| Secret scanning and push protection | On | |
| Branch ruleset on `develop` | Required reviews and required status checks | The checks to require are `Can see status`, `Can enqueue` and `Can merge`, which is what makes the merge queue work. |

Three of those -- the Renovate token, secret scanning, and the ruleset
-- are only visibly working once something has run. A scheduled
workflow that has never succeeded looks exactly like one nobody has
needed yet, and the fleet has lost a week to that before, so after the
repository is created dispatch `renovate.yml` and
`export-repo-config.yml` by hand once each and confirm they pass. The
reasoning is shakenfist/development's
[scheduled-workflow-health](https://github.com/shakenfist/development/blob/main/docs/audits/scheduled-workflow-health.md)
criterion, which reports a workflow that has never succeeded on the
default branch and has failed three times unattended.

A merge queue is required of a repository that publishes releases,
and `ci.yml` reports through the gates it needs, so enabling it on
`develop` is one of the steps waiting below. `max_entries_to_build`
and `min_entries_to_merge` are both `1`; the
reasoning is in shakenfist/development's
[merge-queue-config](https://github.com/shakenfist/development/blob/main/docs/audits/merge-queue-config.md)
criterion.

## Review tracking is adopted in the tree and not on GitHub

Human review tracking, from shakenfist/development's
[`review-tracking`](https://github.com/shakenfist/development/tree/main/templates/review-tracking)
template, records which source files a person has actually read, as
distinct from which ones a tool has checked. The file half is here:
`.github/workflows/prune-reviews.yml` and the two scripts under
`tools/` are byte-for-byte copies of the template, the scope policy
in `.vscode/review-scope.toml` and the `.gitignore` entry for
weAudit's per-session day log are this repository's own, and
`REVIEWS.md` is generated by `./tools/review-tracking.sh regen`. It
currently reports that none of the in-scope files has been reviewed
by anybody, which is correct.

The other half is configuration on GitHub, and none of it can exist
while there is no repository: an environment whose deployment-branch
policy is what keeps the push credential unreadable from any other
branch, and -- only where the default branch's ruleset requires a
pull request -- an App, its key in that environment, and a bypass
for it on the ruleset. Until they exist `prune-reviews.yml` cannot
run, so no stale mark is pruned and shakenfist/development's own
reviews of the template files copied from there are not imported;
`REVIEWS.md` is only as current as the last hand-run of the script.
The steps and their exact commands are in shakenfist/development's
[code-review-tracking](https://github.com/shakenfist/development/blob/main/docs/code-review-tracking.md).

## Waiting on the repository existing

There is no remote, so nothing on the GitHub side of any of this is
set up, and none of the ten workflows has ever run. Whoever creates
`shakenfist/teneral` does, as well as the settings above: the
`review-marks` environment and the rest of the review-tracking
configuration just described; the merge queue on `develop`; and, for
releases, a `release` environment with required reviewers deploying
only from `v*` tags and a tag ruleset restricting who may create
one. [Releasing](releasing.md) links the fleet's setup document for
the last two. None of it is an oversight and none of it has been
worked around.

## The fleet conventions themselves

Teneral is not yet in scope for shakenfist/development's weekly
consistency audit. The criteria it will be measured against are
specified in
[`docs/audits/`](https://github.com/shakenfist/development/tree/main/docs/audits)
there, one page per criterion, and the templates the fleet copies from
are in
[`templates/`](https://github.com/shakenfist/development/tree/main/templates).
