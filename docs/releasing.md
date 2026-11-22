# Releasing teneral

Teneral uses the fleet's Rust release automation: a version bump
goes through the merge queue as a pull request, the merged commit is
tagged, and the tag starts a workflow that checks CI on that commit,
builds, waits for a reviewer to approve the `release` environment,
signs the tag with Sigstore and creates the GitHub release. The
mechanism, its invariants and how to recover from each way the gate
can fail are documented once for the whole fleet, in
[`templates/release-automation-rust/README.md`](https://github.com/shakenfist/development/blob/main/templates/release-automation-rust/README.md).
This page records only what is particular to teneral.

**Nothing here can run yet.** The release workflow, the two
proposing and tagging scripts and the version check are all in the
tree, but there is no GitHub repository behind them: no remote to
push a branch to, no merge queue to land it through, no `release`
environment to approve and no tag ruleset. Whoever creates the
repository configures those first, following steps 2 to 4 of the
fleet's
[`RELEASE-SETUP.md`](https://github.com/shakenfist/development/blob/main/templates/release-automation/RELEASE-SETUP.md).
Until then this page describes a process rather than one that has
been run.

## The release process

1. From a clean checkout of `develop`, in step with `origin`, run
   `tools/propose-release.sh X.Y.Z`. It bumps the version, runs
   `make lock`, pre-commit, the version check and `make test` over
   the bumped tree, then asks before it commits, pushes and opens
   the pull request. It never tags.
2. Land that pull request through the merge queue. The queue's run
   on the merged commit is the CI result the release gate requires,
   so a merge that bypasses the queue cannot be released.
3. Run `tools/tag-release.sh X.Y.Z`. It finds the commit that merged
   `Release X.Y.Z.`, checks it is on `develop`'s first-parent
   history and carries the version, then tags and pushes.
4. Approve the `release` environment when the workflow stops at
   `sign-tag`.

There are no Makefile wrappers for the first and third steps,
unlike ryll and andris. Passing a version to a `make` target needs
a second goal and a no-op rule to absorb it, which is more make
machinery than a passthrough to a script deserves; the scripts take
the version as an ordinary argument.

## What a release contains

One artefact: the optimised `teneral` binary for Linux x86_64,
built by `make release` inside the build container and attached to
the GitHub release. No `.deb`, no `.rpm` and no tarball yet, and no
registry publication -- teneral publishes to no crates.io or PyPI
package, so the GitHub release is the whole of it. Packaging is a
decision for the phase that gives the daemon something worth
installing, and the build section of
`.github/workflows/release.yml` is where it lands when it does.

## Verifying a release tag

Once `sign-tag` has run, anyone with
[gitsign](https://github.com/sigstore/gitsign) can check that the
tag was signed by this repository's release workflow:

```bash
git fetch --tags --force origin
gitsign verify-tag vX.Y.Z \
  --certificate-identity-regexp '^https://github\.com/shakenfist/teneral/\.github/workflows/release\.yml@refs/tags/v[0-9]' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

The identity pattern names the workflow and the kind of ref it ran
on, which is all `tools/sign-release-tag.sh` will sign. A pattern
that stopped at the repository would accept a signature from any of
its workflows, on any branch.
