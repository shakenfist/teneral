# Development

How to build, lint and test teneral, what CI runs, and which parts of
the arrangement live in GitHub's settings rather than in this tree.

Teneral has no Cargo workspace yet, so this page is in two halves:
what is in place now, and what arrives with the first code. The
[implementation plan](plans/PLAN-teneral-implementation.md) says when.

## Rust is built in Docker, not on the host

Rust toolchains are deliberately not installed on the development
machine, which runs several Rust projects at different toolchain
versions. The build therefore happens inside a container image the
repository defines, following the pattern
[ryll](https://github.com/shakenfist/ryll) and
[instar](https://github.com/shakenfist/instar) already use: a
`Makefile` wraps `docker run` so that `make lint`, `make build` and
`make test` work the same on a laptop and on a CI runner.

None of that exists yet. It arrives with the workspace.

## Linting

```
pre-commit run --all-files
```

That is the whole gate, and it is what `.github/workflows/ci.yml`
runs. Nothing in CI names a linter directly, so a hook added to
`.pre-commit-config.yaml` is enforced on the next pull request without
a CI change, and CI cannot end up checking something different from
what a developer's commit checked.

The hooks today:

| Hook | What it reads |
|------|---------------|
| `actionlint` | The workflow files, including their embedded shell. Its runner-label list is `.github/actionlint.yaml`. |
| `trailing-whitespace`, `end-of-file-fixer`, `check-yaml`, `check-merge-conflict` | Everything. |
| `shellcheck` | Shell under `scripts/` and `tools/`, with `-x` so a sourced sibling is followed. |
| `gitleaks` | The working tree, for credentials. |
| `skillsaw` | The agent context -- `AGENTS.md` and anything else an agent is handed -- for malformed structure, smuggled instructions and pasted secrets. |

`rustfmt`, `clippy` and `cargo deny` join the list with the
workspace, running through the Docker image rather than on the host.

## What CI runs

Every workflow runs on the Shaken Fist self-hosted runner pools. The
labels this repository uses are declared in `.github/actionlint.yaml`,
so a typo or a retired image fails the lint rather than leaving a job
queued forever.

| Workflow | Trigger | What it does |
|----------|---------|--------------|
| `ci.yml` | Pull requests to `develop`, and `workflow_dispatch` | The `pre-commit` suite, then the automated reviewer once it passes. |
| `secret-scan.yml` | Pull requests, pushes to `develop`, and `workflow_dispatch` | `gitleaks` over the branch and the history behind it. Separate from `ci.yml` because it needs the push trigger, and because it must read the prose a path filter would skip. |
| `renovate.yml` | Hourly | Dependency updates, from the policy in `renovate.json`. |
| `export-repo-config.yml` | Daily | Exports this repository's GitHub settings and opens a pull request when they have changed, so the settings below are reviewable in version control. |
| `pr-re-review.yml` | Comment `@shakenfist-bot please re-review` | Another pass of the automated reviewer on a pull request. |
| `pr-retest.yml` | Comment `@shakenfist-bot please retest` | Re-dispatches the test workflow without a new commit. |

`ci.yml` carries no path filter and no two-tier smoke/merge split, and
declares no `Can enqueue` / `Can merge` gates. Everything in the
repository today is prose, and every hook above reads prose, so a
filter would skip the only job there is. The filter, the merge tier and
the gates arrive with the code they exist to protect; the fleet shape
to copy them from is
[andris's `ci.yml`](https://github.com/shakenfist/andris/blob/develop/.github/workflows/ci.yml)
and the reasoning is in shakenfist/development's
[expensive-lane-path-filter](https://github.com/shakenfist/development/blob/main/docs/audits/expensive-lane-path-filter.md)
and
[merge-group-cancellation](https://github.com/shakenfist/development/blob/main/docs/audits/merge-group-cancellation.md)
criteria.

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
| Branch ruleset on `develop` | Required reviews and required status checks | The checks to require are the jobs in `ci.yml`; a merge queue, and the `Can enqueue` / `Can merge` gates it needs, come with the merge tier. |

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

A merge queue is required of a repository that publishes releases, so
it is adopted along with release automation rather than now. When it
is, `max_entries_to_build` and `min_entries_to_merge` are both `1`; the
reasoning is in shakenfist/development's
[merge-queue-config](https://github.com/shakenfist/development/blob/main/docs/audits/merge-queue-config.md)
criterion.

## Still to come

These are the fleet conventions teneral does not yet meet, each
because it has nothing to act on. They are work for the foundations
phase of the
[implementation plan](plans/PLAN-teneral-implementation.md), not
oversights.

- **The Cargo workspace**, the `Makefile` and the Docker build image,
  and with them the `rustfmt`, `clippy` and `cargo deny` hooks and the
  build and test lanes.
- **`deny.toml`**, which is what proves no GPL or AGPL dependency
  enters the tree. That is a licence constraint rather than a
  preference; see the [provenance record](provenance.md).
- **The panic lints**: `unwrap_used` set to `warn` or `deny` in the
  workspace manifest's `[workspace.lints.clippy]`, every crate
  inheriting it, and `allow-unwrap-in-tests = true` in a root
  `clippy.toml` so test code is exempt. A panic here is an outage for
  every network at once, so this is not style; `AGENTS.md` carries it
  as an invariant.
- **Two-tier CI** with a path filter and the `Can enqueue` /
  `Can merge` gates, and the merge queue they back.
- **Release automation**, from shakenfist/development's
  [`release-automation-rust`](https://github.com/shakenfist/development/tree/main/templates/release-automation-rust)
  template.
- **Human review tracking**, from shakenfist/development's
  [`review-tracking`](https://github.com/shakenfist/development/tree/main/templates/review-tracking)
  template: `.vscode/review-scope.toml`, `REVIEWS.md` and the pruning
  workflow. It tracks which source files a human has read, so it is
  deployed once there are source files.
- **CodeQL**, from shakenfist/development's
  [`codeql`](https://github.com/shakenfist/development/tree/main/templates/codeql)
  template, whose language matrix needs code to analyse.

## The fleet conventions themselves

Teneral is not yet in scope for shakenfist/development's weekly
consistency audit. The criteria it will be measured against are
specified in
[`docs/audits/`](https://github.com/shakenfist/development/tree/main/docs/audits)
there, one page per criterion, and the templates the fleet copies from
are in
[`templates/`](https://github.com/shakenfist/development/tree/main/templates).
