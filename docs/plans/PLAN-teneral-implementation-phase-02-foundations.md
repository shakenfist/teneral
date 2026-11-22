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
   at push time; under `deny`, the first one fails the build that
   introduced it. `AGENTS.md` carries "a panic is a cluster-wide
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
| 2d | medium | opus | none | The two fleet templates. Copy `templates/release-automation-rust` and `templates/review-tracking` from `shakenfist/development` byte for byte where the template is byte-for-byte, and make the follow-on edits each template's own README says to make -- read both READMEs first. `review-tracking` brings `.vscode/review-scope.toml`, `REVIEWS.md` and a pruning workflow, and `docs/development.md` says it is deployed once there are source files, which 2a has now created. Do not hand-adapt a template file that is meant to be copied verbatim: the fleet runs a consistency audit that compares them. |
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

1. `make build` produces a binary, with nothing Rust installed on
   the host: `which cargo` on the host fails and the build still
   succeeds.
2. `make test` and `make lint` both exit 0, and `make lint` runs
   both `cargo fmt --all --check` and `cargo clippy --workspace
   --all-targets -- -D warnings`.
3. `unwrap_used = "deny"` appears in `[workspace.lints.clippy]`
   and the crate's `Cargo.toml` says `[lints] workspace = true`.
4. An `unwrap()` added to a non-test function fails `make lint`;
   the same call added to a `#[test]` function does not. Both
   demonstrated and recorded, both reverted.
5. `clippy.toml` exists at the root and contains
   `allow-unwrap-in-tests = true`.
6. `make deny` exits 0, and `grep -n 'deny *=' deny.toml` finds no
   `licenses.deny` key.
7. A GPL-licensed crate added as a dependency makes `make deny`
   fail, naming the licence. Demonstrated, recorded, reverted.
8. `Cargo.lock` is committed; `fetch` passes `--locked`, every
   compile target passes `--frozen`, and `lock` is the only target
   that can write the lockfile.
9. There is no `rust-toolchain.toml`, and the toolchain version
   appears exactly once outside `Cargo.lock`.
10. `pre-commit run --all-files` passes, and `rustfmt`, `clippy`
    and `cargo deny` are hooks in it.
11. `actionlint` passes on every file in `.github/workflows/`.
12. `ci.yml` declares a `merge_group` trigger, and jobs named for
    the `Can enqueue` and `Can merge` gates both exist.
13. A CodeQL workflow exists and its language matrix names Rust.
14. `REVIEWS.md` and `.vscode/review-scope.toml` exist, and the
    pruning workflow from the template is present.
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
