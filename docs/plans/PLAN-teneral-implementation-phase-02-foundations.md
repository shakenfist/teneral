# Teneral implementation phase 2: repository foundations

## Planning effort

**High.** The phase is mostly pattern-following, which argues for
medium, but three things in it are decisions rather than copying:
how strict the panic lint is, how the Docker build image is built
given what phase 1d measured, and whether to write CI lanes that
cannot run until the repository has a remote. Each is cheap to
write and expensive to change once code is built on it.

## Scope

**In.** Everything in the foundations list that needs code to act
on: the Cargo workspace and its lints, the Docker build image and
the `Makefile` that wraps it, `clippy.toml`, `deny.toml`, the
Rust hooks in `pre-commit`, the build and test lanes, two-tier CI
and the merge queue, CodeQL, the `release-automation-rust` and
`review-tracking` templates, and the differential rig skeleton.

**Out.** Any daemon behaviour. No DHCP, no DNS, no binding store,
no API. The workspace compiles and its tests run, but what it
compiles is a skeleton: phase 3 builds the store and phase 4 the
API, and a crate split made now would prejudge both. Also out:
creating the GitHub repository, which is the operator's to do and
is the subject of a back-brief gate below.

## What the survey found

Checked against the tree on 2026-11-22, at `1337e79`.

1. **The "landed ahead of phase 1" half is accurate.** The master
   plan says `pre-commit`, the lint and reviewer CI lanes, the
   secret scanner, Renovate, the configuration export and the
   review and retest bots are already in place. All six workflows
   exist -- `ci.yml`, `export-repo-config.yml`, `pr-re-review.yml`,
   `pr-retest.yml`, `renovate.yml`, `secret-scan.yml` -- as do
   `.pre-commit-config.yaml`, `renovate.json` and
   `docs/development.md`. Nothing in that list was wrong.

2. **One claim is stale: `docs/provenance.md` is done.** The phase
   2 sketch lists "`docs/provenance.md` with its first rows from
   1b" as remaining work. Phase 1's step 1f shipped it with 26
   rows, each citing the lab write-up's experiment and heading.
   Corrected at source in the master plan as part of this
   planning commit.

3. **`docs/development.md` carries a fuller list than the master
   plan does**, under *Still to come* (lines 109-141), and it is
   the better specification of this phase. It names two things the
   master plan's sketch omits: the **panic lints**
   (`unwrap_used` in `[workspace.lints.clippy]`, every crate
   inheriting, and `allow-unwrap-in-tests = true` in a root
   `clippy.toml`) and **CodeQL** from shakenfist/development's
   template. The master plan's sketch in turn names the
   differential rig skeleton, which *Still to come* omits. This
   phase takes the union.

4. **Phase 1 already wrote a validated `deny.toml`.** It is in the
   phase 1 plan's 1d findings, as a complete file --
   `PLAN-teneral-implementation-phase-01-research-spikes.md` in
   this directory. This phase transcribes
   it rather than inventing one.

5. **`cargo-deny` rejects the obvious way to write the GPL
   denial.** Under `[licenses] version = 2` the `licenses.deny`
   key was removed as redundant (cargo-deny PR 611) and is a hard
   configuration error, not a warning. The denial is therefore
   expressed as an allow-list containing no copyleft licence,
   which is strictly stronger because it also denies the licences
   nobody thought to name. Phase 1 measured this; a step that
   writes `deny = [...]` from memory will fail.

6. **Nothing Rust exists yet.** No `Cargo.toml`, `Makefile`,
   `clippy.toml`, `deny.toml`, `rust-toolchain.toml` or
   `Dockerfile`.

7. **`ci.yml` has two jobs and no merge queue.** `lint` (running
   `pre-commit run --all-files` on `[self-hosted, static]`) and
   `automated_reviewer`. `merge_group` appears nowhere in
   `.github/`. So the two-tier shape is genuinely absent rather
   than half-present.

8. **ryll is a complete exemplar and differs from phase 1d in two
   ways that matter.** It has `Cargo.toml` with
   `[workspace.lints.clippy] unwrap_used = "warn"`, `clippy.toml`
   with `allow-unwrap-in-tests = true`, `deny.toml`, a `Makefile`
   whose `fetch` passes `--locked` and whose every compile passes
   `--frozen` with a separate `lock` target as the only lockfile
   writer, and a two-tier `ci.yml` with `merge_group` and
   `Can enqueue` / `Can merge`. But it builds a **devcontainer**
   image from `mcr.microsoft.com/devcontainers/base:debian` with
   rustup inside, and it has **no `rust-toolchain.toml`**,
   declaring its floor as `rust-version` in `[workspace.package]`
   instead.

9. **Phase 1d measured the plain-image path instead, and hit two
   traps.** It used `rust:1.100.0-trixie` directly, with `cargo
   fetch` networked and cache writable, then compile
   `--network none --offline` with the cache read-only, because a
   dependency's `build.rs` runs arbitrary code. The traps: the
   official `rust:` images carry no `cargo-fmt` or `cargo-clippy`
   binary, so `rustup component add rustfmt clippy` is needed;
   and a `rust-toolchain.toml` makes rustup try to sync the
   channel, which fails under `--network none` unless
   `RUSTUP_TOOLCHAIN` is set in the environment.

10. **ryll's `warn` is a `deny` in practice.** Its `make lint`
    runs `cargo clippy ... -- -D warnings`, so the manifest's
    `warn` is promoted at the gate. The choice between `warn` and
    `deny` in the manifest is therefore about where the lint
    bites, not whether -- which is decision 1 below.

11. **Both named templates exist** in shakenfist/development:
    `templates/release-automation-rust` and
    `templates/review-tracking`. So does `templates/codeql`.

12. **The differential rig has no code anywhere.** It is named in
    `docs/provenance.md` and the master plan and nowhere else.
    Phase 1's *Future work* adds a constraint on its eventual
    shape: phase 5's rig should redeploy the Linux-bridge lab
    playbook rather than invent a rig of its own.

13. **There is still no remote.** `git remote -v` is empty and the
    only branch is `develop`. Six workflows already sit in
    `.github/` having never run, so this phase adds to an existing
    stock of unexercised CI rather than starting one.

## Decisions

1. **`unwrap_used = "deny"` in `[workspace.lints.clippy]`, not
   ryll's `"warn"`.** Survey finding 10 is the honest framing:
   with `-D warnings` at the gate, both settings block a merge, so
   this changes only where the lint bites. That is exactly why it
   is worth changing. Under `warn`, a developer builds locally for
   an afternoon accumulating `unwrap()` calls and meets them all
   at push time; under `deny`, the first one is an error the
   moment clippy sees it -- a bare `cargo clippy` with no flags,
   and an editor running clippy as you type, both report it red
   rather than as one more line of yellow among many. Note what
   `deny` does *not* buy, because the first draft of this
   decision claimed it: `unwrap_used` is a clippy lint, so an
   ordinary `cargo build` never evaluates it at whatever level,
   and nothing makes a compile fail on one. `AGENTS.md` carries
   "a panic is a cluster-wide
   outage" as an invariant rather than a preference, and the
   phases that follow -- a packet parser, a lease table, an option
   encoder -- are precisely where an `unwrap()` is cheapest to
   write and most expensive to keep. `allow-unwrap-in-tests =
   true` keeps test code exempt, as `docs/development.md` already
   specifies. This is the decision most likely to be argued with;
   see the back brief.

2. **The build image is a thin derived image, not a devcontainer.**
   `FROM rust:1.100.0-trixie` plus `rustup component add rustfmt
   clippy` and `cargo install cargo-deny`, built by the `Makefile`
   and tagged locally, following what 1d actually ran rather than
   ryll's devcontainer. Two reasons: 1d measured this exact path
   on this exact host, and a devcontainer carries editor
   integration this project has no use for yet. If a devcontainer
   is wanted later it is additive.

3. **No `rust-toolchain.toml`.** Finding 9: it fights
   `--network none`, and the workaround is an environment variable
   that must be remembered in every invocation. The toolchain is
   pinned by the image tag in one place in the `Makefile`, and the
   language floor is declared as `rust-version` in
   `[workspace.package]` so a too-old toolchain fails with cargo's
   MSRV error. This is ryll's arrangement and it is the one that
   survives offline builds.

4. **`deny.toml` is transcribed from phase 1, not rewritten.**
   Finding 4. The file phase 1 produced was validated against a
   real dependency tree and carries the PR 611 explanation in its
   own comments. Rewriting it from memory is how finding 5's trap
   gets stepped in a second time.

5. **The workspace is one crate.** A root `Cargo.toml` with
   `[workspace]`, `[workspace.package]`, `[workspace.lints]` and a
   single binary crate. The architecture names several components,
   but which of them are crates is a phase 3 and 4 decision, and
   an empty crate created now is a liability: it has to be kept
   compiling, kept linted, and justified in review. The workspace
   machinery is what this phase is for, and it works with one
   member.

6. **The lockfile discipline is ryll's.** `fetch` passes
   `--locked`, every compile passes `--frozen`, and a separate
   `lock` target is the only thing allowed to write `Cargo.lock`.
   Without that target there is no in-container path to a lockfile
   update at all, and the first build after adding a dependency
   fails confusingly.

7. **CI lanes are written now, and knowingly cannot run.** Finding
   13: six workflows already sit unexercised, so writing more is
   consistent rather than novel, and a foundations phase that
   omits CI leaves the next phase to invent it alongside real
   code. The mitigation is that everything checkable locally is
   checked -- `actionlint` is already a pre-commit hook -- and the
   push-audit phase verifies the lanes on the first real push.
   The alternative, deferring until the repository exists, is put
   to the operator as a back-brief gate rather than taken here.

8. **The differential rig skeleton defines a shape, not a run.**
   It specifies where a captured corpus lives, what a comparison
   asserts, and how a case names its provenance row -- and it runs
   no dnsmasq, in CI or otherwise. Finding 12: the lab is the only
   lawful place dnsmasq runs, phase 1 destroyed it, and phase 5
   redeploys it. A skeleton that tries to run dnsmasq in a CI
   container would be both unlawful as provenance and untestable.

9. **This phase lands on `develop` as phase 1 did.** Decision 8 of
   phase 1 still holds: no remote, so no pull request, and the
   `Merged` cell takes a `first..last` range. The consequence is
   the same and is accepted knowingly -- no review pass and no CI
   run, so the management session's read of the diff is the only
   check.

## Step plan

Steps run **sequentially**. Every one writes to the shared
worktree, and 2b through 2e each depend on the workspace 2a
creates; two steps running `make` or `pre-commit` in one tree at
once would trip over each other's half-finished work.

| Step | Effort | Model | Isolation | Brief for sub-agent |
|------|--------|-------|-----------|---------------------|
| 2a | high | opus | none | The workspace, the build image and the `Makefile`. Read decisions 1, 2, 3, 5 and 6 and survey findings 8, 9 and 10 first. Create: a root `Cargo.toml` with `[workspace]`, `[workspace.package]` (edition, `rust-version`, `license = "Apache-2.0"`) and `[workspace.lints.clippy]` carrying `unwrap_used = "deny"`; one binary crate whose `Cargo.toml` says `[lints] workspace = true`; a root `clippy.toml` with `allow-unwrap-in-tests = true`; a `Dockerfile` for the build image per decision 2; and a `Makefile` wrapping `docker run` with targets `build`, `test`, `lint`, `fetch` and `lock`, following ryll's `Makefile` for the cache and flag discipline (decision 6) but not its devcontainer. The binary's `main` may do nothing but print its version -- do not invent daemon behaviour. Verify by running `make build`, `make test` and `make lint` and pasting their output into the commit message; `make lint` must run `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings`. Prove decision 1 bites: add an `unwrap()` to a non-test function, show the build fails, add one to a test, show it passes, then revert both. Nothing Rust may be installed on the host. |
| 2b | medium | opus | none | `deny.toml` and the Rust pre-commit hooks. Transcribe `deny.toml` from the 1d findings in `docs/plans/PLAN-teneral-implementation-phase-01-research-spikes.md` -- it is a complete file there, written against a real tree, and decision 4 says transcribe rather than rewrite. Do not write a `licenses.deny` list: survey finding 5 says cargo-deny rejects the key outright under `version = 2`. Add a `deny` target to the `Makefile` and add `rustfmt`, `clippy` and `cargo deny` to `.pre-commit-config.yaml` as local hooks that call the `Makefile`, so that CI keeps running nothing but `pre-commit run --all-files` as `docs/development.md` promises. Verify the denial actually denies: add a GPL-licensed crate as a dependency, show `make deny` fails and name the licence it reported, then remove it. Record that command and its output. |
| 2c | high | opus | none | Two-tier CI, the merge queue and CodeQL. Read decision 7 and survey findings 7 and 13: these lanes cannot run until the repository has a remote, which is deliberate and must not be worked around by inventing a trigger that would fire locally. Rewrite `.github/workflows/ci.yml` to ryll's two-tier shape -- read `ryll/.github/workflows/ci.yml` as the pattern -- with a path filter, a cheap tier gating `Can enqueue` on `pull_request`, an expensive tier gating `Can merge` on `merge_group`, and the build, test, lint and deny lanes from 2a and 2b placed in the right tier. Keep the existing `lint` and `automated_reviewer` jobs working. Add CodeQL from shakenfist/development's `templates/codeql`, with Rust in its language matrix. Everything must pass `actionlint`, which is already a pre-commit hook, and the runner labels must be ones `.github/actionlint.yaml` declares. Do not invent runner labels. |
| 2d | medium | opus | none | The two fleet templates. Copy `templates/release-automation-rust` and `templates/review-tracking` from `shakenfist/development` byte for byte where the template is byte-for-byte, and make the follow-on edits each template's own README says to make -- read both READMEs first. `review-tracking` brings exactly three files -- `prune-reviews.yml` and two scripts under `tools/` -- all byte-identical copies naming no repository or branch; `.vscode/review-scope.toml` and the `.gitignore` exception are adoption steps you write, and `REVIEWS.md` is generated by the script rather than copied. Its README is explicit that the workflow does not work without GitHub-side configuration: a `review-marks` environment carrying a specific default-branch policy, and where the default branch's ruleset requires a pull request, the `shakenfist-review-marks` App, its key and a bypass. None of that can be created while there is no repository, so do the file half, write what the adopter owns, and record the GitHub-side steps as blocked -- do not pretend them done and do not invent a substitute. `release-automation-rust` is five files whose scripts need `chmod +x` after copying, and its flow calls `make lock` and `make test`, which 2a has created. `docs/development.md` says review tracking is deployed once there are source files, which 2a has now created. Do not hand-adapt a template file that is meant to be copied verbatim: the fleet runs a consistency audit that compares them. |
| 2e | high | opus | none | The differential rig skeleton. Read decision 8: this defines a shape and runs no dnsmasq. Create the rig's directory, a README stating what it is for and why it cannot run yet, the corpus layout for captured request/response pairs, and the comparison harness with at least one case that runs against a recorded fixture rather than a live server. Every case names the `docs/provenance.md` row it supports, and the harness fails a case that names no row -- that check is the point of the skeleton, so write it and show it failing on a case with the row removed. Phase 1's *Future work* says phase 5 redeploys the Linux-bridge lab playbook rather than inventing a rig; say so in the README so phase 5 does not start over. |
| 2f | medium | opus | none | Fold back and close out. Four things, and nothing else -- this step writes no new code. (1) Update `docs/development.md`: its *Still to come* list (lines 109-141) must no longer name anything this phase shipped, and its "Teneral has no Cargo workspace yet" framing and the "None of that exists yet" line under *Rust is built in Docker* are both now false. (2) Update `AGENTS.md` only if a *convention* changed -- the panic lint being `deny` rather than `warn` is one, because it is a thing an agent cannot infer from the code; do not move reference material into it. (3) Correct the master plan where this phase proved it wrong, and set phase 2's `Status` to `Complete` with its `Merged` cell per decision 9; `docs/plans/index.md` stays `In progress`. (4) Add anything this phase declined to the master plan's *Future work*. Run `pre-commit run --all-files` and fix what it reports. |

## Risks and mitigations

| Risk | Mitigation |
|------|------------|
| `unwrap_used = "deny"` proves unworkable once real code arrives, and gets quietly relaxed to `warn` in a later phase without the reasoning being revisited. | 2a must demonstrate the lint biting and not biting (the brief requires it), so the behaviour is recorded rather than assumed. If a later phase wants `warn`, decision 1 is the thing to argue with, and `AGENTS.md` will carry it as a convention after 2f. |
| A step writes `deny = [...]` in `deny.toml` from memory, because that is what every other project's `deny.toml` looks like, and loses an afternoon to a configuration error. | Survey finding 5 and decision 4 both say it, and 2b's brief says it a third time with the reason. The file is transcribed from a validated original rather than composed. |
| The CI lanes are written, never run, and are wrong in a way nobody discovers until the first push -- at which point they are load-bearing. | `actionlint` runs as a pre-commit hook over every workflow, including embedded shell, and its runner-label list is `.github/actionlint.yaml`. That catches structure and labels but not logic, which is stated plainly rather than papered over: the push-audit phase verifies the lanes on the first real push, and decision 7 records that this was chosen rather than overlooked. |
| The differential rig skeleton grows a live dnsmasq, because that is the obvious way to make it useful, and the result is both unlawful as provenance and unrunnable in CI. | Decision 8 and 2e's brief forbid it, and the harness's one real check -- that a case names a provenance row -- works entirely on fixtures. |
| `cargo install cargo-deny` in the build image makes the image build slow and network-dependent, and a Renovate bump to the base image silently changes the toolchain. | The image tag is pinned in one place (decision 3) and the `Makefile` builds the image as an explicit target, so a change to either is a diff. If image build time becomes the complaint, the fix is a published image, which is additive. |
| Phase 2 lands with no review pass and no CI run, as phase 1 did, and a defect in the build machinery is carried into phase 3 where it is harder to see. | Decision 9 accepts this knowingly. The definition of done is written so that every claim about the build is something the management session can re-run in one command rather than read. |

## Definition of done

Each item is checkable by running something.

1. `make build` produces a binary, and builds it in the container
   rather than on the host. The host carries a distribution Rust
   1.85.1, so an absent host toolchain is the wrong test: instead,
   every `cargo` invocation in the `Makefile` is inside a
   `docker run`, and the build reports the image's compiler rather
   than the host's 1.85.1.
2. `make test` and `make lint` both exit 0, and `make lint` runs
   both `cargo fmt --all --check` and `cargo clippy --workspace
   --all-targets -- -D warnings`.
3. `unwrap_used = "deny"` appears in `[workspace.lints.clippy]`
   and the crate's `Cargo.toml` says `[lints] workspace = true`.
4. An `unwrap()` added to a non-test function fails `make lint`,
   and also fails a bare `cargo clippy` carrying no
   `-D warnings` -- the second is what distinguishes the
   manifest's `deny` from the gate's flag. Beware a false pass
   here: a construction that borrows a temporary fails with
   `E0716`, which looks like a lint failure in `make lint`'s
   output and is not one, so the added call must be on a
   `&'static str` or similar. The same call added to a `#[test]`
   function does not fail;
   the same call added to a `#[test]` function does not. Both
   demonstrated and recorded, both reverted.
5. `clippy.toml` exists at the root and contains
   `allow-unwrap-in-tests = true`.
6. `make deny` exits 0 with `advisories ok, bans ok, licenses ok,
   sources ok`, and the `[licenses]` table carries no `deny` key:
   `awk '/^\[licenses\]/{f=1} /^\[/ && !/^\[licenses\]/{f=0}
   f && /^deny/' deny.toml` prints nothing. A plain
   `grep 'deny *='` is the wrong check and this item used to name
   it: `[bans]` has a `deny` key of its own, deliberately empty,
   so the grep matches on a correct file.
7. A copyleft crate added as a dependency makes the licence check
   fail, naming the licence. Demonstrated, recorded, reverted from
   a copy rather than with `git checkout`. Prefer an **LGPL** crate
   over a GPL one: weak copyleft is the case someone is most
   likely to assume is acceptable, so denying it is the stronger
   demonstration, and the allow-list denies both by omission.
8. `Cargo.lock` is committed; `fetch` passes `--locked`, every
   compile target passes `--frozen`, and `lock` is the only target
   that can write the lockfile.
9. There is no `rust-toolchain.toml`, and the toolchain version
   appears exactly once outside `Cargo.lock`.
10. `pre-commit run --all-files` passes, and `rustfmt`, `clippy`
    and `cargo deny` are hooks in it.
11. `actionlint` passes on every file in `.github/workflows/`.
12. `ci.yml` declares a `merge_group` trigger, and ryll's three
    gate jobs all have counterparts: `Can see status`,
    `Can enqueue` and `Can merge`. The first was missed when this
    plan was written.
13. `.github/workflows/codeql-analysis.yml` is byte-identical to
    `shakenfist/development/templates/codeql/codeql-analysis.yml`,
    verified with `diff`, and CodeQL analyses Rust in it. There is
    deliberately **no** language matrix, which is what this item
    asked for when the plan was written: the template pins no
    `languages` and CodeQL's own autodetection finds Rust, so a
    matrix would be a divergence from the template that buys
    nothing. The evidence that it finds Rust is a real run rather
    than a reading -- ryll's CodeQL job logs `Autodetected
    languages: rust, ...` and `Running queries for rust` -- and no
    lane in this repository can be run until there is a remote.
14. `review-tracking`'s three files are present and byte-identical
    to the template: `.github/workflows/prune-reviews.yml`,
    `tools/ci-prune-reviews.sh` and `tools/review-tracking.sh`,
    each verified with `diff`. `.vscode/review-scope.toml` exists
    and `.gitignore` carries `.vscode/.weauditdaylog`, both being
    adoption steps the adopter writes rather than things the
    template carries. This item used to ask for a `.gitignore`
    *exception* for the review state, which is wrong here: an
    exception is only needed where `.gitignore` excludes
    `.vscode/`, and this repository does not, so the state files
    are already visible to git. What such a repository needs is
    the opposite entry, ignoring weAudit's per-session day log,
    which attests to nothing and would otherwise ride along in
    every review commit. The rewriting pre-commit hooks must also
    exclude `^\.vscode/.*\.weaudit`, scoped per hook so that the
    content scanners still read review prose. The GitHub-side
    adoption steps -- the `review-marks` environment with its
    default-branch policy, and the App, its key and the ruleset
    bypass -- are recorded as blocked on the repository existing,
    not ticked and not silently skipped.
15. The differential rig's harness fails a case whose provenance
    row is removed. Demonstrated and recorded.
16. `docs/development.md` contains no sentence saying the Cargo
    workspace, the `Makefile`, the build image, `deny.toml`, the
    panic lints, two-tier CI, CodeQL, release automation or review
    tracking is still to come.
17. The master plan has phase 2 `Complete` with a populated
    `Merged` cell, and `docs/plans/index.md` carries the plan as
    `In progress`.
18. No container or process this phase started is still running,
    and any one-off build image is either kept deliberately or
    removed.

## Back brief

Before executing any step, back brief the operator on your
understanding of this plan and how the work you intend to do
aligns with it.

Three gates, all cheap to ask about and expensive to get wrong:

- **Decision 1, before 2a.** `unwrap_used = "deny"` is stricter
  than ryll, the fleet's Rust exemplar, and survey finding 10
  shows the practical difference is narrower than it first looks.
  It will be felt in every phase after this one. Confirm or
  overrule before the workspace is built on it.
- **Decision 7, before 2c.** Whether to write CI lanes now that
  cannot run until the repository has a remote, or to defer them.
  Bound up with a question only the operator can answer: whether
  to create the GitHub repository at all yet. Phase 1 put the same
  question and the answer was to land locally for now; this phase
  asks again because the cost of the wrong answer has risen.
- **Decision 2, before 2a.** The build image diverges from ryll's
  devcontainer in favour of what 1d actually ran. If the fleet is
  meant to converge on one devcontainer pattern, this is the
  moment to say so, because the `Makefile` is written against
  whichever is chosen.

## Execution notes

Kept as the phase runs, for departures from the plan and for the
things a later reader would otherwise have to reconstruct.

**2026-11-22, AEDT. Back brief answered, all three gates as
planned.** Decision 1 stands: `unwrap_used = "deny"` rather than
ryll's `warn`. Decision 7 stands: the CI lanes are written now,
knowing they cannot run, and the GitHub repository is still not
created. Decision 2 stands: the build image is derived from the
plain `rust:` image that 1d measured, not ryll's devcontainer.

**A planning premise was wrong: this host has a Rust toolchain.**
`/usr/bin/cargo` and `/usr/bin/rustc` are a distribution Rust
1.85.1, from Debian packages rather than rustup. The operator's
preference is against *rustup-managed* toolchains on the host,
which is what the `ryll` and `instar` Docker pattern exists to
avoid, and it is unaffected. But definition-of-done item 1 was
written to assert `which cargo` fails, which would never have
passed. It now asserts the thing actually worth asserting --
that every `cargo` in the `Makefile` runs inside a container,
and that the build reports the image's compiler rather than
1.85.1. Corrected before step 2a ran, so no step was briefed
against it.

**2026-11-22, AEDT. Decision 1 claimed something clippy does not
do.** It said that under `deny` the first `unwrap()` "fails the
build that introduced it". It does not: `unwrap_used` is a clippy
lint, and `cargo build` invokes rustc, which never evaluates
clippy's lints at any level. Step 2a found this while
demonstrating the lint, and the decision is reworded to claim
what is true -- that `deny` makes a bare `cargo clippy`, and an
editor running clippy as you type, report the call as an error
rather than a warning. The decision stands on that narrower
ground, which is narrower than the back brief described it.

**The toolchain tag is invisible to Renovate.** It lives once, in
the `Makefile`, as `RUST_IMAGE` passed to a default-less `ARG` in
the `Dockerfile` -- one place, which is what decision 3 asked
for, but Renovate's dockerfile manager reads `FROM` lines and
cannot see a version arriving as a build argument. Automatic
base-image bumps therefore need a `customManagers` rule. Left for
2f to record in the master plan's *Future work* rather than
solved here, since Renovate is already configured and this is a
gap in its coverage rather than a defect in the build.

**Three premises in this plan were checked against the fleet
before their steps were briefed, and two were wrong.** The
`review-tracking` template brings three files, not the
`.vscode/review-scope.toml` and `REVIEWS.md` this plan first
credited it with, and its workflow needs GitHub-side
configuration that cannot exist without a repository; step 2d's
row and definition-of-done item 14 are rewritten accordingly.
ryll carries three merge gates rather than two, so item 12 named
only the last two. The third premise held: teneral's
`.github/actionlint.yaml` declares `static`, `claude-code`, `vm`,
`debian-13` and `s`, so 2c has a heavier runner available and
need invent no label.

**2026-11-22, AEDT. `cargo deny` goes into pre-commit split in
two, and the advisory half is deferred to a scheduled lane.**
Step 2b's row says to add `cargo deny` to `pre-commit`. Only
`licenses`, `bans` and `sources` went in, as the `deny-offline`
`Makefile` target: those three are a function of `Cargo.lock`
alone, so the same tree gives the same verdict today and next
month, and they are the half that enforces the licence
constraint the file exists for. `advisories` did not, because its
answer changes when somebody publishes to the RustSec database:
as a commit hook it would fail a commit touching no Rust at all,
for a reason the committer did not cause. ryll puts exactly that
check in a separate scheduled `supply-chain.yml` instead. **Step
2c therefore owns a scheduled supply-chain lane running
`make deny`**, which is the full check including advisories, and
which needs network where the compile lanes do not. The lane is
the one place CI runs something other than
`pre-commit run --all-files`, which `docs/development.md`
otherwise promises; it is scheduled rather than per-commit, so
the promise that a developer's commit and a pull request check
the same things is intact.

**`deny.toml` needed no change for cargo-deny 0.22.0.** The file
phase 1 wrote was transcribed as it stood and validated first
time, with no rejected or deprecated key -- including
`[advisories] unmaintained = "workspace"`, which has moved
between releases. Note for a future reader that definition-of-done
item 6's `grep -n 'deny *=' deny.toml` does match one line,
`deny = []` at line 74. That is `[bans] deny`, a different key
and a deliberately empty one; `[licenses] deny`, the key the item
is about and the one cargo-deny rejects outright, is absent.

**2026-11-22, AEDT. Step 2c's row stated the runner-label rule
too bluntly, and the correction matters.** The row said the labels
"must be ones `.github/actionlint.yaml` declares" and "do not
invent runner labels". But `actionlint.yaml` declares only the
labels this repository already uses, as its own comment says, so
read literally the rule would confine every new lane to the
`static` pool -- which carries git and little else and has no
Docker daemon. The real rule is: never use a label the fleet does
not have, and the fleet's own workflows are the evidence of what
it has. `debian-13-docker` is used by ryll, instar, kerbside,
shakenfist and a dozen others, so it exists; it is now declared in
`actionlint.yaml`, with a comment saying that declaring a label
does not create a runner. `l` was deliberately not adopted: it is
the scarcest pool on the fleet and this workspace is one small
crate, so every container lane runs at `s`.

**The existing `lint` job had to move off the `static` pool, which
is a consequence of 2b rather than a choice of 2c.** Step 2b put
`rustfmt`, `clippy` and `cargo deny` into `pre-commit` as hooks
that call the `Makefile`, and the `Makefile` runs every `cargo`
inside `docker run`. `pre-commit run --all-files` therefore now
needs a Docker daemon, which the `static` image does not have, so
the job would have failed the first time a commit touched a `.rs`
file. It still runs exactly the same command; only `runs-on`
changed, to `[self-hosted, vm, debian-13-docker, s]`. Moving to a
`vm` runner also means it now carries the fork guard the rest of
the fleet puts on anything that runs checkout content on a
credentialed runner.

**CodeQL does analyse Rust, and the fleet template needs no
change to do it.** Checked rather than assumed, two ways. GitHub's
"Workflow configuration options for code scanning" page lists Rust
among the languages code scanning supports and gives its
identifier as `rust` in the language-identifier table, with no
preview or beta label -- the only "public preview" wording on that
page is about model packs and threat models. And empirically:
ryll carries the same workflow with the same
autodetection, differing from the template only in using floating
`@v4` action tags where the template pins by commit -- so ryll is
the drifted copy and the one written here is the correct one. Its
most recent successful run, checked in the management session with
`gh run view`, logs `Autodetected languages: rust, python,
javascript, ruby, actions`, `None of the languages in this project
require extra build steps`, and `Running queries for rust`. So
autodetection finds Rust without a `languages` pin, and the
template's `Autobuild` step is a no-op for it. The file is
therefore copied byte for byte, which `diff` against
`shakenfist/development/templates/codeql/codeql-analysis.yml`
confirms, and no comment was added to it -- the template's README
says to copy it verbatim and the fleet audit compares the bytes,
so the reasoning is recorded here instead.

**Definition-of-done item 13 is wrong and was not worked around.**
It asks that "a CodeQL workflow exists and its language matrix
names Rust". There is no language matrix: the fleet template
deliberately pins no `languages`, the README names the only two
repositories that diverge and why, and teneral adding a third
divergence would buy nothing CodeQL does not already do by itself.
The checkable claim the item should make is that a CodeQL workflow
exists and CodeQL analyses Rust in it, which the ryll evidence
above establishes for these exact bytes. Left for 2f to correct at
source rather than edited here.

**The supply-chain lane gates nothing, deliberately.**
`supply-chain.yml` runs `make deny` -- the full check, advisories
included -- weekly and on pull requests, and appears in no
`needs:` list. An advisory published between a pull request opening
and its merge is not that pull request's fault, and a gate on it
would eject a queue entry for something the author did not cause.
The half that *is* the author's fault, licences and bans and
sources, is already a `pre-commit` hook and so is gated inside
`lint` with everything else a developer can reproduce locally.

**`lint` does not consume the path filter, unlike `build` and
`test`.** ryll can filter its lint job because its lint job is
Rust-only and its prose scanners are separate jobs. Teneral's one
`lint` job runs the whole `pre-commit` suite, most of which reads
human-written text -- trailing whitespace, YAML validity, gitleaks,
skillsaw over the agent context -- so a documentation-only change
is exactly the change those hooks exist to catch. Filtering it
would skip the only check such a change has.

**The cargo cache has to live outside the checkout in CI.** The
`Makefile`'s in-checkout default is deleted every run by
`actions/checkout`, whose `clean: true` runs `git clean -ffdx`,
and the `Makefile` comment already says to override `CARGO_CACHE`
for that reason. Every container lane sets it to
`${{ github.workspace }}/../teneral-cargo-cache`: outside the
checkout so it survives the clean, inside the runner's work
directory for this repository so it is per-repository and needs no
hardcoded home path.

**Tiering, stated so a reader can see it was chosen.** `lint` and
`build` are the cheap tier, on `pull_request`, gating
`Can enqueue`; `test` is the expensive tier, on `merge_group`,
gating `Can merge`; `Can see status` runs on every event and
depends on nothing. `test` is in the merge tier for what the suite
becomes rather than what it is -- `build` and `lint` already
compile the workspace and `clippy --all-targets` type-checks the
test targets, so what `test` adds is *running* them, and the growth
in it is the differential comparisons against a captured corpus.
Placing it there now is cheaper than moving it later under time
pressure. `workflow_dispatch` runs both tiers, which keeps
`pr-retest.yml` a full retest.

**2026-11-22, AEDT. The `.gitignore` adoption step for review
tracking is a no-op here, and the opposite entry was needed
instead.** Definition-of-done item 14 asks that "the `.gitignore`
exception for `.vscode/*.weaudit-shas.json` is present". It is not,
and must not be: teneral's `.gitignore` carries no `.vscode` rule
at all, so every review-state file -- the weAudit marks, their
sidecars, `.vscode/imports.weaudit-shas.json` and
`review-scope.toml` -- is already visible to git, and a `!`
negation with nothing to negate would read as a guard that does
something. Adoption step 1 asks for the *reverse* entry in a
repository that does not ignore `.vscode/`, and that was added:
`.vscode/.weauditdaylog`, weAudit's per-session log of which files
were opened, which attests to nothing and would otherwise ride
along in every review commit. Item 14 should be read as "the
`.gitignore` is correct for whichever case the repository is in",
and 2f can correct it at source.

**The pre-commit half of adoption step 1 did apply.** teneral runs
`trailing-whitespace` and `end-of-file-fixer`, which rewrite what
they are given, and the review-mark generator emits no trailing
newline -- so without an exclude every `pre-commit run --all-files`
rewrites the marks and reports a failure that cannot usefully be
fixed. Both hooks now exclude `^\.vscode/.*\.weaudit`, scoped to
those two hooks rather than at the top level, so that gitleaks and
skillsaw still read the review notes: they are human-written prose,
which is exactly where a credential or a smuggled character lands.
This is what the `review-marks-pre-commit` audit checks.

**Adoption step 8's `paths-ignore` was already half done, and the
other half went to `supply-chain.yml`.** `ci.yml` filters the
review state at the job level, which is the form a required check
needs, and `codeql-analysis.yml` is the fleet template and carries
both forms already. The one code-shaped workflow left was
`supply-chain.yml`, which gates nothing and so may safely use the
trigger-level form; it now skips `REVIEWS.md` and the two weAudit
patterns on `pull_request`, and deliberately not
`.vscode/review-scope.toml`, which is policy rather than review
state. `secret-scan.yml` was deliberately left alone: it is a
content scanner, and step 8 says not to blind those.

**The `release-automation-rust` build section needed a `release`
target that did not exist.** The template's example build job runs
`make release` and uploads `target/release-assets/`. Teneral's
`Makefile` had no `release` target -- step 2a wrote `build`, which
is a debug compile -- so the section was filled with a `release`
target of its own: `cargo build --release --frozen --workspace`,
offline and against the read-only cache like every other compile
target. A release must not ship a debug binary, and the workflow
should not have to know the flags that distinguish the two. The
artefact is that one binary, unpackaged: no `.deb`, no `.rpm`, no
tarball and no registry, because teneral has nothing worth
installing yet and inventing a package format here would be a
decision made by whoever wrote the workflow rather than by whoever
has to support it. The example's runner label was also `m`, which
the fleet has but this repository does not use; it is `s` like
every other container lane here.

**The optional Makefile wrappers were declined.** The template's
README says a repository *may* add `make propose-release X.Y.Z`,
as ryll and andris do. Teneral does not. Passing a version to a
`make` target needs a second goal plus a no-op rule to absorb it,
which is fifteen lines of make machinery standing in front of a
script that already takes the version as an ordinary argument, and
no audit asks for the wrappers. `docs/releasing.md` records the
choice beside the process, so the next person to wonder finds the
reason rather than an omission.

**`REVIEWS.md` was generated, not written.**
`./tools/review-tracking.sh regen` produced it, against the
`shakenfist/development` clone the wrapper finds. It says 0 of 40
in-scope files are reviewed, which is correct: no file in this
repository has been reviewed by anybody, and the import that would
carry in shakenfist/development's own review of the six copied
template files runs in CI, which cannot run. `scope-orphans`
reports every tracked file either in scope or explicitly excluded,
so `review-scope-completeness` would pass.

**GitHub-side adoption, blocked on the repository existing.** None
of the following can be done while there is no `shakenfist/teneral`
on GitHub and no remote. They are not ticked, not worked around and
not disabled; whoever creates the repository does them, in this
order, and the file half of both templates is already in the tree
waiting for them.

Review tracking, before `prune-reviews.yml`'s first run (adoption
steps 3 and 7 of shakenfist/development's
`docs/code-review-tracking.md`, which carries the exact commands):

1. A ruleset protecting `develop` from history rewrites, with at
   least the `deletion` and `non_fast_forward` rules.
2. A `review-marks` environment whose `deployment_branch_policy` is
   `{"protected_branches": false, "custom_branch_policies": true}`,
   with exactly one deployment-branch policy: `type: branch`,
   `name: develop`, no glob characters and no tag entry. Required
   whether or not the repository will hold a key, because an
   environment GitHub creates on first reference has no policy at
   all and a key added later would then be readable from any
   branch. Read both back and check them; "admits the default
   branch" is not the test.
3. Only if `develop`'s ruleset requires a pull request, and then in
   this order: install the `shakenfist-review-marks` App on the
   repository; set its private key as the `REVIEW_MARKS_APP_KEY`
   secret of the `review-marks` environment and nowhere else, which
   is a fleet-wide key rotation rather than a new key for this
   repository; and add the App (app id 5538020) to that ruleset's
   bypass actors, carrying every existing actor over. Without the
   ruleset, none of the three is needed and the push uses
   `GITHUB_TOKEN`. With the ruleset and without the key, the first
   run fails closed on the refused push, which is the only signal
   that the key is missing.
4. Add teneral to the "Projects using these templates" table in
   `templates/review-tracking/README.md`, in shakenfist/development,
   with its default branch and which identity it pushes as. That is
   a change to another repository, so it is not in this tree.

Release automation, before the first release (steps 2 to 4 of
`templates/release-automation/RELEASE-SETUP.md`):

5. A `release` environment with required reviewers, deploying only
   from tags matching `v*`. The `release-environment-protection`
   audit checks exactly this.
6. A `Release tags` tag ruleset targeting `v*`, with restricted
   creations and deletions and force pushes blocked, and the person
   who pushes release tags on its bypass list -- `sign-tag` needs no
   bypass of its own. `current_user_can_bypass` must read `always`
   for whoever runs `tools/tag-release.sh`.
7. The merge queue on `develop`, which the release gate depends on:
   it counts only a `merge_group` run or a push to the default
   branch, and this repository's CI reports its results through
   `Can enqueue` and `Can merge`.
