Thanks for your work on this. I appreciate it. Some final
checks before I push.

## How to use this runbook

This repository is a network service that hands out addresses
and answers name lookups for every client on a segment, and
which deliberately serves many networks from one process. Its
blast radius has three parts, and the briefs below are written
for them:

**A defect is silent.** Nothing here returns an error to a user.
A duplicated lease, a reservation that stopped applying, or a
name answered from the wrong zone surfaces as a client that
cannot reach anything, minutes to hours later, with no stack
trace and usually no log line at the point of failure.

**A defect can cross networks.** One process holds every
network's bindings, and two networks may legitimately use the
same address. Anything touching lookup, dispatch or the binding
store can leak one network's answers into another. Once the
metadata front-end exists, that leak serves another instance's
credentials.

**The input is unauthenticated and hostile.** Every DHCP and DNS
byte arrives from an unauthenticated client. Rust removes the
memory-safety class; it does not remove a parser that trusts a
length field, a lease table that trusts a client identifier, or a
panic that takes the daemon down for every network at once.

The audit splits into two waves:

**Wave 1 -- mechanical.** Lint and the test suites, then
the grep-level checks on the diff. Always run wave 1 first;
wave 2 is only worth spending on if wave 1 passes.

**Wave 2 -- judgment.** Four independent sub-agents that read
code and apply judgment. They can be spawned in parallel.

The management session reviews all findings, fixes any issues,
and confirms the push.

Every diff command below reads a range held in one variable,
`AUDIT_RANGE`. Run `git fetch origin` first: `origin/develop` is
itself a cached ref that only advances on fetch, so a clone
left alone for a while would otherwise widen the audit
silently, even under the range below.

The default, for work not yet merged, is `origin/develop...HEAD`.
Work that has already landed sets `AUDIT_RANGE` explicitly
instead: a phase that landed as one merge commit sets
`AUDIT_RANGE=<sha>^1..<sha>`, and a plan whose phases landed as
several merges runs every command once per merge. A phase that
landed on `develop` directly, with no merge commit, sets
`AUDIT_RANGE=<first>^..<last>` -- with the caret. `first..last`
is the label the `Merged` column records, but `A..B` excludes
`A`, so the diff that actually covers that label is one commit
wider than the bare range; drop the caret and the first commit
of the phase silently falls out of the diff, which is where a
version bump or a column addition tends to live.

Every command below is written
`git diff "${AUDIT_RANGE:-origin/develop...HEAD}"`, with the
default in the shell expansion rather than left for the reader
to export: an unset bare `$AUDIT_RANGE` diffs the working tree
instead, which is empty on a clean checkout, and every check
then passes over nothing.

One consequence matters for wave 2, because it is silent. A
sub-agent runs in its own shell and inherits nothing, so a brief
handed over with the expansion in it expands to the default --
which against already-landed work is the empty diff this rule
exists to prevent, and an empty diff reads as a clean review. So
the management session substitutes the concrete range into each
wave 2 brief rather than passing the expansion, tells each agent
which range it is reading, and requires every report to state
the insertion and deletion totals it measured. A report whose
totals do not match the range is re-run rather than triaged: it
is the one check on this that does not depend on the agent
noticing its own mistake.

## Wave 1: Mechanical checks

```
pre-commit run --all-files
```

That covers the hooks: actionlint over `.github/workflows/`,
shellcheck over `tools/`, and the formatting and whitespace
hooks. `ci.yml` runs the same command on every pull request, so a
clean local run is what makes the CI run boring.

Then the Rust toolchain, which is the substance of wave 1 here:

```
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo deny check
```

`cargo deny check` is the licence gate as much as the advisory
gate. This project is Apache-2.0 and may not take a GPL or AGPL
dependency even transitively, and `deny.toml` is what proves it;
an inspection of `Cargo.toml` does not, because the problem
arrives through the dependency tree rather than the direct
dependencies. Treat a new advisory exception the same way as a
new `#[allow]`: it needs a comment saying why.

Then the grep-level checks on the diff:

```
# origin/develop is a cached ref: without this a stale clone widens
# the audit silently, even under the range rule above
git fetch origin

# Touched source files long enough for `source-file-size` in 2a to
# be worth raising -- 800 lines to ask the question, 1500 past which
# a file may not grow without a reviewed edit to the repository's
# baseline. Judging them is wave 2's job; this only produces the
# number, so that the judgement is made about a named file rather
# than left to whether anyone happened to notice. wc measures each
# touched file whole, not the branch's hunks, because the block has
# 2a read the files a diff touches: a small addition to a long file
# is how slow growth gets past a review of the diff alone.
# The extension list is this repository's: Rust, plus the Python and
# shell under tools/. --diff-filter=d keeps a deleted path out of
# wc's argv, and the awk drops wc's own total line -- xargs splits a
# long list over several wc runs, each printing one. -z/-0 rather
# than a bare pipe because a path with a space in it would otherwise
# be split, and git C-quotes anything non-ASCII under core.quotePath
git diff --name-only -z --diff-filter=d \
    "${AUDIT_RANGE:-origin/develop...HEAD}" -- '*.rs' '*.py' '*.sh' | \
    xargs -0r wc -l | sort -rn | awk '$1 > 800 && $2 != "total"'

# Panics reachable from network input. A DHCP or DNS parser that
# unwraps is a remote denial of service against EVERY network the
# daemon serves, not just the one that sent the packet -- the
# consolidation that makes this project worth building is also what
# turns one panic into a cluster-wide outage. Hits in tests and
# build scripts are fine; hits under src/ are findings until 2a
# says otherwise.
git diff "${AUDIT_RANGE:-origin/develop...HEAD}" -- '*.rs' ':!*/tests/*' ':!tests/*' | \
    grep -nE '^\+[^+].*(\.unwrap\(\)|\.expect\(|panic!\(|unreachable!\(|todo!\(|unimplemented!\()'

# Slice indexing added in parsing code. `buf[n]` and `buf[a..b]`
# panic on a hostile length field; `get()` and `get(a..b)` return
# None. This is the single most likely shape of a remotely
# triggerable panic in this codebase.
git diff "${AUDIT_RANGE:-origin/develop...HEAD}" -- 'src/*.rs' | \
    grep -nE '^\+[^+].*\[[a-z_][a-z_0-9]*(\s*\.\.|\s*\])'

# New or widened unsafe. Raw sockets, AF_PACKET and setns are where
# this project is tempted; each use needs a SAFETY comment naming
# the invariant it relies on.
git diff "${AUDIT_RANGE:-origin/develop...HEAD}" -- '*.rs' | \
    grep -nE '^\+[^+].*\bunsafe\b'

# New dependencies. Each needs a reason, and `cargo deny` needs to
# have been re-run: the licence risk in an Apache-2.0 project
# arrives transitively, so a new direct dependency is a new subtree.
git diff "${AUDIT_RANGE:-origin/develop...HEAD}" -- 'Cargo.toml' '*/Cargo.toml' | grep -nE '^\+'
git diff "${AUDIT_RANGE:-origin/develop...HEAD}" -- 'Cargo.lock' | \
    grep -nE '^\+name = ' | sort -u

# Protocol behaviour changed without its provenance recorded. This
# repository may not copy GPL code, so every wire-visible behaviour
# traces to an RFC section or to a named observed-behaviour test.
# If the first command prints anything and the second prints
# nothing, that is a finding before the code is even read.
git diff "${AUDIT_RANGE:-origin/develop...HEAD}" --name-only -- \
    'src/dhcp/*' 'src/dns/*' 'src/tftp/*' 'src/wire/*'
git diff "${AUDIT_RANGE:-origin/develop...HEAD}" --name-only -- 'docs/provenance.md'

# Configuration read from a file rather than the API. The decision
# that teneral has no config file is load-bearing: it is what keeps
# the binding store the single source of truth and what removed the
# need to reproduce dnsmasq's option grammar. A file parser
# creeping back in is an architectural regression, not a feature.
git diff "${AUDIT_RANGE:-origin/develop...HEAD}" -- 'src/*.rs' | \
    grep -nE '^\+[^+].*(read_to_string|File::open|/etc/|\.conf|\.yaml|\.toml")'

# Hard-coded addresses, interfaces and ports. A network daemon with
# a baked-in 10.0.0.1, eth0 or :67 works in one deployment and in
# no other. Test fixtures are expected to hit this; src/ is not.
git diff "${AUDIT_RANGE:-origin/develop...HEAD}" -- 'src/*.rs' | \
    grep -nE '^\+[^+].*([0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}|"(eth|ens|br-|veth)[a-z0-9-]*"|:(53|67|68|69|80|547)\b)'

# TODO / FIXME / HACK / XXX, and new suppressions
git diff "${AUDIT_RANGE:-origin/develop...HEAD}" -- '*.rs' '*.py' | \
    grep -nE '^\+.*\b(TODO|FIXME|HACK|XXX)\b'
git diff "${AUDIT_RANGE:-origin/develop...HEAD}" -- '*.rs' | \
    grep -nE '^\+.*#\[allow\('
git diff "${AUDIT_RANGE:-origin/develop...HEAD}" -- 'deny.toml' | grep -nE '^\+'

# Plan references added to code, which the `plan-references-in-code`
# block below makes a finding. Markdown and plans directories, where
# plans cite themselves, are excluded; the plans exclusion needs
# `glob` magic, without which `docs/**/plans/**` matches nothing under
# docs/plans/. A hit is a candidate for the reviewer, not a verdict.
git diff "${AUDIT_RANGE:-origin/develop...HEAD}" -- ':!*.md' ':(exclude,glob)docs/**/plans/**' | \
    grep -niE '^\+[^+].*(\bphases?\s+[0-9]+[a-z]*\b|\bphase-[0-9]+\b|\bdecisions?\s+[0-9]+\b|\bsteps?\s+[0-9]+[a-z]+\b|PLAN-[A-Za-z0-9._-]+\.md)'

# Documentation touched at all (warns if none)
git diff "${AUDIT_RANGE:-origin/develop...HEAD}" --name-only -- 'docs/*' '*.md'
```

Exit condition: wave 1 passes when `pre-commit` is clean and
each grep has either no hits or hits the management session has
looked at and accepted. The greps report; they do not block.

If the diff touches a shared block copied from
shakenfist/development, it is almost certainly wrong: those blocks
are maintained upstream and arrive here by template re-sync, not by
local edit. Revert it and raise the change upstream instead.

If the diff touches `docs/provenance.md`, read the added rows
against the code they claim to cover. A row that cites an RFC the
code does not implement, or an observed-behaviour test that does
not exist in the differential rig, is worse than no row: it
converts an open question into a settled one without settling it.

`REVIEWS.md` needs no change in a pull request that changes code
or documentation, and review tracking is not set up in this
repository yet, so there is nothing to prune. The phase that
adopts the `review-tracking` template is what makes the fleet
rule live here.

## Wave 2: Deeper review

Only run wave 2 after wave 1 passes.

### 2a. Code quality

| Setting | Value |
|---------|-------|
| Model | opus |
| Effort | high |

Higher than the fleet default for this slot, deliberately. The
characteristic defect in this repository is not ugly code, it is
code that is correct for one network and wrong for two, or correct
for a well-formed packet and a panic for a malformed one. Neither
is visible without holding the lookup path in mind, and neither
shows up as a failing test unless someone thought to write the
second network into the fixture.

**Brief for sub-agent:**

The mechanical sweep has already extracted panics, slice
indexing, `unsafe`, new dependencies, suppressions and
TODO/FIXME comments. Take that report as input, and triage each:
blocking or advisory, and why. It also listed the touched source
files over 800 lines: read each of those whole, not only its
hunks, and judge it against `source-file-size` below.

Then the judgment-level review of
`git diff "${AUDIT_RANGE:-origin/develop...HEAD}"`:

- **Per-network or global?** For every piece of state the diff
  adds or changes, decide which it is, and say so. A cache, a
  counter, a lease table, a name index, an option set: if it is
  global and should have been keyed by network, two networks
  using the same address will overwrite each other's answers.
  This is the defect class the project exists to risk, so it is
  the first thing to look for and the one most worth being
  pedantic about. A single-network test passes either way.
- **Does the lookup carry the network all the way down?** Trace
  the diff's path from "a packet arrived on interface X" to "this
  is the answer". The network identity must be an argument, not
  something re-derived halfway down from an address that two
  networks might share. A function that takes an IP and no
  network is a candidate finding even if today's only caller
  happens to be unambiguous.
- **Panics reachable from the network.** Every hit from the
  mechanical sweep under `src/` is guilty until shown otherwise.
  The question is not "can this be None" but "can a client make
  this None". A panic here takes down every network at once.
- **Arithmetic on lengths and offsets.** Option encoding, DNS
  name compression and TFTP block numbering all do arithmetic on
  values a client supplied. Look for subtraction that could
  underflow, casts that could truncate, and totals that could
  exceed a buffer the code then trusts. Rust turns these into
  panics in debug and wrapping in release, so neither is safe.
- **Lease and reservation lifecycle.** Does the change preserve
  the invariant that an address is committed to at most one
  client at a time, across restart? An address handed out from a
  pool before it is durably recorded is handed out twice after a
  crash. Say where the durability point is.
- **Provenance.** Any wire-visible behaviour the diff adds or
  changes must be traceable in `docs/provenance.md` to an RFC
  section or a named observed-behaviour test. A behaviour
  justified in a comment as "what dnsmasq does", with no RFC and
  no test, is both a licence concern and an untested assumption.
- **No config file.** The daemon is configured through its API.
  Flag anything that reads configuration from a file at runtime.
  Reading a *state* file the daemon itself wrote is different and
  fine; say which one it is.
- **Duplicated logic across front-ends.** DHCP, DNS and TFTP
  resemble each other by design -- each turns a request into a
  binding-store lookup and back. Flag duplication only where a
  helper already exists and was not used, or where a front-end
  reimplements the lookup rather than calling it.

<!-- shared-block: comment-proportion v1 -->
Comment proportion (shared block; do not edit -- the canonical
copy lives in shakenfist/development at
`templates/shared-blocks/comment-proportion.md`):

- A comment or docstring earns its length by saying what the code
  cannot: the contract, the units, the failure modes, the reason a
  surprising choice is correct. Restating the code in prose is not
  documentation.
- Treat as candidates any added comment or docstring that is longer
  than the code it documents, and any comment block over roughly
  fifteen lines attached to a body under ten. These are candidates,
  not verdicts -- a subtle algorithm, a public API contract, or a
  hard-won bug explanation can justify the length.
- Where the length is not justified the finding is advisory, and
  the fix is to cut the restatement rather than delete the comment:
  keep the why, drop the line-by-line narration of the what.
- Prose that documents user-visible behaviour rather than the
  implementation usually belongs in `docs/`, with the comment
  reduced to a pointer.
<!-- shared-block-end -->

<!-- shared-block: source-file-size v2 -->
Source file size (shared block; do not edit -- the canonical
copy lives in shakenfist/development at
`templates/shared-blocks/source-file-size.md`):

- Where a repository tracks whole-file human review, a file's cost
  is its length times how often it is touched: every change
  discards the review of the whole file, and the next session
  re-reads all of it. That, rather than taste, is why length is
  worth raising in review at all.
- Treat a source file over roughly 800 lines as a candidate to
  split, and one over roughly 1,500 as wanting a stated reason to
  stay whole. These hold whether or not a repository tracks review
  per file: tracking is what makes the cost repeat and become
  measurable, not what makes a long file expensive to read.
- There is no cap, and no file is ever forced to split: a file
  already over 1,500 lines may stay exactly that long. What it may
  not do is grow, and no file may cross 1,500, without a reviewed
  edit to the repository's baseline of oversize files -- the
  ratchet in shakenfist/development's
  `templates/source-size-ratchet/` checks that on every commit.
  Raising an entry is allowed: it is the stated reason, asked for
  where a reviewer cannot scroll past it. Where a repository has
  not adopted the ratchet, the push auditor applies the same rule
  by hand.
- The push auditor reads the files the diff touches, not only the
  diff. A branch that grows a file already over 800 lines is a
  finding to raise even when its own addition is small, because
  per-diff review is how slow growth gets past: every step looks
  reasonable on its own. Under 1,500 the finding opens a question
  rather than recording a defect. A raised baseline entry is
  reviewed as the decision it is, and its reason belongs in the
  pull request description.
- Generated files, vendored trees and protocol or data tables are
  exempt: they are not read the way source is, and a tool that
  counts them -- the ratchet included -- is measuring the wrong
  thing, so they belong in its exclusion list, not its baseline.
- Split along a seam that already exists -- one module's public
  entry point, one check, one subcommand, one endpoint -- so that
  a later change touches one of the pieces rather than all of
  them. A file split at a line number rather than at a seam is
  worse than the long file it replaced.
- Length is never reduced by deleting the comments and docstrings
  that explain why the code is the way it is. Those are what make
  a long file reviewable, and trading them for a line count makes
  the review worse while making the number better. A diff that
  shrinks a baselined file by deleting rationale comments or
  docstrings is a finding, not a win: the ratchet counts lines and
  cannot tell the difference, so the reviewer has to. Cut
  duplicated scaffolding first; see `comment-proportion` for what
  earns its length.
<!-- shared-block-end -->

<!-- shared-block: plan-references-in-code v1 -->
Plan references in code (shared block; do not edit -- the
canonical copy lives in shakenfist/development at
`templates/shared-blocks/plan-references-in-code.md`):

- Code, comments, docstrings, test names, fixture descriptions and
  configuration describe the software as it is now. Which plan,
  phase, step or decision produced a line is history, and the plan
  and the commit log already keep it. Do not write "added in phase
  5", "per decision 3", "pending step 5f" or "the phase-4 leaks
  pass": a reader of the code has not read the plan, and the
  number tells them nothing.
- Where a comment cites a plan to explain why the code is the way
  it is, the explanation belongs in the comment. Write the reason
  -- the constraint, the measurement, the failure it prevents --
  and drop the citation. A pointer standing in for the reasoning
  costs every reader a detour, and rots when the plan is archived
  or renumbered.
- A plan link is acceptable only for work that is not built yet: a
  deliberate gap or refusal whose lifting is planned, where
  "deferred; see `PLAN-foo.md`" tells the reader the gap is known.
  The link comes out when the work lands. Prefer an issue link
  where one exists, and write a plan in another repository as an
  absolute URL; the `plan-source-references` audit checks that
  these links resolve.
- Plan documents and commit messages may cite phases and decisions
  freely; recording that history is their job.
- "Phase" in its ordinary sense -- a two-phase commit, a compiler's
  link phase -- is not a plan reference.
- A plan reference a diff adds to code is a finding to fix before
  pushing. References on lines the diff does not touch are backlog,
  not findings against the change.
<!-- shared-block-end -->

<!-- shared-block: python-version-discipline v1 -->
Python version and typing (shared block; do not edit -- the
canonical copy lives in shakenfist/development at
`templates/shared-blocks/python-version-discipline.md`):

- No syntax or standard library API newer than the floor in
  `requires-python`. Structural pattern matching, `X | Y` unions in
  annotations evaluated at runtime, `tomllib`, and
  `datetime.UTC` each raise on an interpreter the package still
  claims to support, and none of them fail in CI when CI runs only
  the newest version. This is the finding to look for first: it is
  a real break on a real user's machine, not a style point.
- New and modified code carries type hints, and mypy is expected to
  be clean over it. A project part way through a staged rollout is
  held to the new code, not to the whole tree.
- Prefer the walrus operator and f-strings where they make the code
  read better, subject to the floor above.
- Raising the floor in `requires-python` is a supported-platforms
  decision, not a convenience: it drops users. If it is genuinely
  right, the platforms table, `requires-python` and
  `constraints.python` in `renovate.json` all move together.
<!-- shared-block-end -->

Report findings as a bullet list. For each, state the file, line,
and whether it is blocking or advisory.

### 2b. Test review

| Setting | Value |
|---------|-------|
| Model | sonnet |
| Effort | medium |

**Brief for sub-agent:**

Review `git diff "${AUDIT_RANGE:-origin/develop...HEAD}"` for test
coverage. Tests are Rust: unit tests beside the code, integration
tests under `tests/`, and the differential rig under
`tests/differential/` which runs a real dnsmasq and compares
bytes. All are run by `cargo test --all`.

- **The two-network test.** Does every change to lookup or to the
  binding store have a test with two networks holding the *same*
  address, asserting each client gets its own answer? One network
  proves nothing about the property this project is built on.
  A missing two-network case is the characteristic gap here and
  should be reported as blocking, not advisory.
- **The malformed-input test.** Does every parser change have a
  case with a hostile input -- a length field longer than the
  buffer, a truncated option, a zero-length name, a compression
  pointer that loops? A parser tested only on well-formed packets
  is tested on the inputs that were never the risk.
- **Round trips.** Does new encode/decode code have a round-trip
  test, and ideally a property test over arbitrary inputs rather
  than a handful of literals?
- **Does the differential rig cover it?** Any change to bytes on
  the wire should have a differential case. If the behaviour
  deliberately diverges from dnsmasq, the rig should assert the
  divergence rather than omit the case, so that the difference is
  recorded rather than merely absent.
- **Durability.** Is there a test that restarts the store and
  asserts the lease survived? An in-memory-only test cannot
  distinguish "recorded" from "about to be recorded".
- **Does any test reach outside its fixture** -- a real
  interface, a privileged socket, the network, a path outside
  `tempfile`? Those pass on a developer's box and fail in CI, and
  the ones that need `CAP_NET_RAW` need to be marked so they can
  be skipped rather than silently failing.
- Are there assertions on exact log strings or rendered messages
  where the behaviour under test is the protocol result? Those
  break on wording changes and teach people to update tests
  without reading them.

<!-- shared-block: functional-test-coverage v1 -->
Functional test coverage (shared block; do not edit -- the
canonical copy lives in shakenfist/development at
`templates/shared-blocks/functional-test-coverage.md`):

- The standard is "do we run the code to do the real thing, and
  does it work as intended". Every subcommand exposed on the command
  line, and every endpoint exposed by an API, should have a test
  that exercises it for real rather than against a mock of itself.
- For a change that adds or alters user-visible behaviour, the
  question to answer is which functional test would have failed
  before it and passes after. If there is none, that is the finding,
  and it is a finding about this change rather than a note for
  later.
- Unit tests are held to no coverage percentage, but a branch that
  is reachable from outside the process and has no test is worth
  naming. Error paths and argument validation are where this bites:
  they are the code most often written once and never run again.
- Mocking the system under test proves nothing. Mock the boundary --
  the network, the clock, the hypervisor -- and let the code being
  tested actually run.
- Where a gap is real but out of scope for the change in hand, say
  so plainly and record it, rather than silently widening the
  change or silently leaving it unsaid.
<!-- shared-block-end -->

Report findings as a bullet list grouped by file.

### 2c. Documentation review

| Setting | Value |
|---------|-------|
| Model | sonnet |
| Effort | medium |

**Brief for sub-agent:**

Check that documentation matches the current code state. Read
`git diff "${AUDIT_RANGE:-origin/develop...HEAD}"` and verify:

<!-- shared-block: readme-discipline v1 -->
README discipline (shared block; do not edit -- the canonical
copy lives in shakenfist/development at
`templates/shared-blocks/readme-discipline.md`):

- New user-visible features are documented in `docs/` (and
  `ARCHITECTURE.md` / `AGENTS.md` where appropriate), not by
  adding bullets to `README.md`.
- `README.md` is a pitch: what the project is, who it is for,
  minimal installation instructions, a small number of usage
  examples, and curated absolute links into `docs/`. It only
  changes when the pitch, the install story, or the
  documentation links change.
- README growth is itself a finding: if the diff adds README
  content that belongs in `docs/`, flag it as blocking and
  move it.
<!-- shared-block-end -->

<!-- shared-block: llm-doc-discipline v1 -->
AGENTS.md and ARCHITECTURE.md discipline (shared block; do not
edit -- the canonical copy lives in shakenfist/development at
`templates/shared-blocks/llm-doc-discipline.md`):

- `AGENTS.md` is a working guide: the conventions, invariants and
  gotchas an agent cannot infer by reading the code, plus curated
  links into `docs/`. It is loaded into every session, so every
  line costs context on every task.
- `ARCHITECTURE.md` is a map: the component inventory, how data
  moves between components, and why the shape is the way it is.
  A deep dive on one subsystem belongs in `docs/`, where humans
  benefit from it too.
- One canonical home per fact. If `docs/` covers it, link to it
  instead of restating it -- and the same rule applies between
  `AGENTS.md` and `ARCHITECTURE.md`.
- Neither file is a reference manual, a runbook, or a changelog.
  CLI flags, configuration keys, wire protocols, step-by-step
  procedures and plan history go to `docs/`.
- Growth in either file is itself a finding: if the diff adds
  content that belongs in `docs/`, flag it as blocking and move
  it.
<!-- shared-block-end -->

<!-- shared-block: diagram-discipline v1 -->
Diagram discipline (shared block; do not edit -- the canonical
copy lives in shakenfist/development at
`templates/shared-blocks/diagram-discipline.md`):

- A diagram of *structure or flow* -- components and the arrows
  between them, an ordered exchange of messages, a state machine
  -- is written as a fenced `mermaid` block, not drawn in ASCII.
  GitHub renders those natively and the mkdocs sites render them
  through `pymdownx.superfences`, so the same source is a picture
  in both places.
- Not every box of characters is a diagram. These stay as plain
  code fences, because mermaid cannot express them and would lose
  what they show: directory and file trees; memory maps, address
  space layouts and register or bit-field diagrams, where column
  alignment carries the meaning; wire-format and on-disk byte
  layouts; captured terminal output; and tables. The test is
  whether the picture is nodes and edges. Something that is a
  table with lines drawn on it is a table.
- Pick the diagram type that matches the claim: `flowchart` for
  components and data flow, `sequenceDiagram` for an ordered
  exchange between parties, `stateDiagram-v2` for a state
  machine, `erDiagram` for data relationships. A sequence drawn
  as a flowchart has thrown away the ordering it existed to show.
- A new ASCII box-and-arrow diagram in the diff is a finding.
  Converting one the diff already touches is in scope; converting
  every other diagram in the file is not, because a sweep is its
  own change and its own review.
<!-- shared-block-end -->

- In this project, the structure that reaches `ARCHITECTURE.md`
  is the layering -- the binding store, the front-ends over it,
  the datapaths beneath it, and how a request flows between them.
  A new front-end or a new datapath changes that shape; a new
  DHCP option does not. The conventions that reach `AGENTS.md`
  are the invariants an agent cannot infer: that there is no
  config file, that no GPL code may be copied in, that wire
  behaviour needs a provenance row, and that lookup changes need
  a two-network test.
- `docs/provenance.md` is this repository's unusual document and
  the one most likely to rot. Every wire-visible behaviour has a
  row naming the RFC section or the observed-behaviour test it
  came from. Check that the diff's new behaviour has one, and
  that it says which of the two it is.
- A deep dive on one front-end, the API reference, a runbook, or
  the differential rig's own documentation belongs in `docs/`,
  with a summary and a link left behind in `ARCHITECTURE.md` --
  not inlined into it.
- The API is the product's interface, so a change to it is a
  documentation change by definition. An endpoint, field or event
  added without its reference page updated is a finding.

<!-- shared-block: plan-phase-references v1 -->
Plan phase references (shared block; do not edit -- the canonical
copy lives in shakenfist/development at
`templates/shared-blocks/plan-phase-references.md`):

- Documentation outside plans directories describes the current
  state of the software, not the history of how it was built. Do
  not write "implemented in phase 5" or "since phase 3 of the
  two-tier CI plan": a reader wants to know whether a feature
  exists, not which phase of which plan delivered it.
- If a documented behaviour is implemented, describe it plainly.
  If it is planned but not yet implemented, link to the master
  plan in `docs/plans/` instead of citing a phase number.
- Reserve the word "phase" for plan documents. A procedural
  document describing a live multi-stage process (a release
  runbook, say) should call its stages "steps" or "stages", so
  that a phase reference in `docs/` is always a plan smell.
- The consistency audit greps `README.md` and `docs/` (excluding
  plans directories) for "phase <number>". Append
  `<!-- audit-ok: phase-reference -->` to a line only when the
  reference is genuinely not about an implementation plan.
<!-- shared-block-end -->

- Plan files in `docs/plans/` are up to date -- the Execution
  table reflects reality, deferred items are recorded rather than
  dropped, and the row in `docs/plans/index.md` matches. In this
  repository plans keep their phases as sections inside the
  master plan rather than as separate phase files.

Report findings as a bullet list. "No documentation gaps found"
is a valid answer.

### 2d. Security review

| Setting | Value |
|---------|-------|
| Model | opus |
| Effort | high |

**Brief for sub-agent:**

Security review of
`git diff "${AUDIT_RANGE:-origin/develop...HEAD}"`. Read the
actual code, not just the diff summary.

The threat model: this daemon listens on unauthenticated network
segments, serves many networks from one process, assigns the
addresses those networks run on, and will eventually serve
instance metadata that contains credentials. The attacker is any
client on any one of those segments. The interesting
vulnerabilities are ones that cross from one network to another,
or that let a client influence another client's configuration.

- **Cross-network isolation.** The central property. Can a client
  on network A obtain, influence or corrupt an answer belonging to
  network B? Look at every cache, index and shared buffer the diff
  touches, and at anything keyed by an address or a MAC rather
  than by `(network, identity)` -- both are attacker-chosen and
  neither is unique across networks. A finding here is critical
  by default.
- **Metadata confidentiality.** Where the metadata front-end
  exists, the binding that maps a request to a content blob is
  an authorisation decision. Is it made from the interface the
  request arrived on and the binding store, or from something the
  client sent? A `Host:` header, an X-Forwarded-For, or a
  client-supplied instance id used to select content is a
  credential disclosure.
- **Identity spoofing.** A DHCP client supplies its own MAC,
  client identifier and hostname. Can a client claim another's
  reservation, or cause a DNS record to be published for a name
  it does not own? Hostname registration from DHCP is the obvious
  path: a client that can register `router` or `*` for its own
  address redirects its neighbours.
- **Remote denial of service.** A panic, an unbounded allocation
  driven by a length field, an unbounded loop on a DNS
  compression pointer, or a lease pool a single client can
  exhaust. Consolidation means one client's packet can take out
  every network, so availability findings are rated a step higher
  here than they would be in a per-network daemon.
- **Privilege.** Raw sockets, `AF_PACKET`, binding port 67 and
  `setns` all need privilege. Does the diff widen what the daemon
  holds, hold it longer than the socket setup, or drop
  capabilities later than it could? Does any new `unsafe` have a
  SAFETY comment naming the invariant it relies on?
- **The API is the control plane.** Is it authenticated, and is
  it bound somewhere a tenant cannot reach? An unauthenticated
  API that can create a binding is an unauthenticated API that
  can assign an address and publish a name on any network.
- **Upstream DNS.** Where the diff touches forwarding: are
  responses matched on query id *and* question *and* source, is
  the source port random, and is a response from an unexpected
  source dropped? Cache poisoning is the classic finding in this
  code.
- **Secrets.** Does the diff add anything that looks like a
  credential, or log one? Metadata content and API tokens must
  not reach the log at any level.

<!-- shared-block: path-traversal-review v1 -->
Path construction from outside data (shared block; do not edit --
the canonical copy lives in shakenfist/development at
`templates/shared-blocks/path-traversal-review.md`):

- Treat as a candidate any filesystem path built from a value the
  process did not choose: a request parameter, an image name, tag or
  digest, a layer path, an archive member name, a filename out of a
  configuration file or a database row.
- The question is not whether the value looks dangerous but whether
  the resulting path is *proved* to stay inside its intended base
  directory. Resolve the joined path with `os.path.realpath()` and
  verify it still starts with the base; a check on the untrusted
  component alone is defeated by symlinks and by encodings the
  check did not anticipate.
- Prefer a helper that cannot be forgotten at a call site --
  `safe_path_join()` in occystrap, or the framework's own
  (`send_from_directory` in Flask) -- over an inline guard repeated
  at each join.
- Archive extraction is the case most often missed: a member name
  inside a tarball or zip is attacker-controlled in exactly the same
  way as a request parameter.
- Where a bare join is correct because every component is
  process-chosen, say so in a comment rather than leaving the
  reader to re-derive it.
<!-- shared-block-end -->

Report findings with severity (critical / high / medium / low /
informational). For each, state the file, line, the vulnerability
class, and a recommended fix.

## Management session checklist

After all agents complete:

- [ ] Wave 1 passed (`pre-commit` clean, `cargo fmt --check`,
      `cargo clippy -- -D warnings`, `cargo test --all` and
      `cargo deny check` clean, greps reviewed).
- [ ] Wave 2 findings reviewed.
- [ ] Any blocking findings from 2a/2b/2c fixed and re-verified.
- [ ] Security findings assessed -- critical and high must be
      fixed before push. A cross-network isolation finding is
      critical by default.
- [ ] Every change to lookup or to the binding store has a test
      with two networks holding the same address.
- [ ] Every wire-visible behaviour the diff adds has a row in
      `docs/provenance.md` naming an RFC section or an
      observed-behaviour test.
- [ ] No GPL or AGPL dependency entered the tree, and
      `cargo deny check` is what proves it.
- [ ] No configuration file parser was added.
- [ ] Any phase that landed in another repository
      (`homelab-deployments-lfs`, `shakenfist`) recorded its
      `<repo> <sha> (#pr)` in the plan, and was audited against
      that repository's default branch.
- [ ] Commit history is clean -- no fixups that should be
      squashed, no accidental files, no WIP messages.
- [ ] Branch is up to date with `develop`.
- [ ] Ready to push.
