# Staged upstream work: dhcproto

Work prepared for <https://github.com/bluecatengineering/dhcproto>.
The patch has since been sent, as [pull request
#111](https://github.com/bluecatengineering/dhcproto/pull/111).
The drafted issue was not sent. Nothing else in this directory
has been submitted, and nothing here should be submitted without
Michael deciding to do it.

## Disposition

| Item | Disposition | State |
|------|-------------|-------|
| [ISSUE-hlen-chaddr.md](ISSUE-hlen-chaddr.md) | not sent | drafted, then declined |
| [PR-hlen-chaddr.md](PR-hlen-chaddr.md) + [the patch](0001-clamp-chaddr-to-the-field.patch) | **sent** | filed, with minor edits, as [#111](https://github.com/bluecatengineering/dhcproto/pull/111) on 2026-11-22 |

**The pull request went; the issue did not.** Both were drafted
on the assumption they went together, the issue describing the
defect and the pull request fixing it. In the event the issue
was judged unnecessary: this project has not used its issue
tracker since 2023, so an issue would have been a second place
for the report to sit unread rather than a route to a
maintainer. That is the middle path the policy notes below set
out, and it answers the disclosure question the same way -- the
pull request describes the defect accurately without being
worded as an advisory.

The drafted issue text is kept rather than deleted, because it
holds the two reproducers and their verbatim output, which the
pull request summarises rather than repeats.

## What was found

Both `chaddr()` accessors in the v4 module size a slice using
`hlen`, a byte read from the packet without validation, while
`chaddr` is the fixed 16-byte BOOTP field. One unauthenticated
DHCP packet with `hlen > 16` panics a server that calls
`Message::chaddr()`, and the zero-copy equivalent returns up to
255 bytes of the surrounding packet as the client's hardware
address instead.

Found while establishing whether `dhcproto` was a viable base for
this project's DHCP front-end, which it still is: the defect is a
few lines and the crate is otherwise sound. Teneral's own
mitigation -- reject `hlen > 16` at ingress, never size a slice
from a packet byte -- is recorded in
[the phase 1 plan](../../docs/plans/PLAN-teneral-implementation-phase-01-research-spikes.md)
and does not depend on upstream acting.

## The project's own policy, checked 2026-11-21

This is the part that has to be established before anything is
sent, and here it is thin:

- **No `CONTRIBUTING.md`**, at the repository root or under
  `.github/`.
- **No `SECURITY.md`**, so no documented private disclosure path.
- **GitHub private vulnerability reporting is disabled**
  (`private-vulnerability-reporting` returns `enabled: false`), so
  there is no non-public channel short of mailing a maintainer.
- **No code of conduct and no issue templates.**
- **No stated policy on machine-assisted contributions**, either
  way. There is nothing to comply with and nothing granting
  permission, so whether the report carries a disclosure note is a
  judgement call rather than a requirement.
- Issues are enabled and there are 7 open, but the newest was
  filed in 2023 while `master` was pushed to in November 2026:
  the maintainers appear to work through pull requests and let
  issues sit. An issue may therefore get no response, and a pull
  request carrying the fix may be the more effective route.

Because there is no `SECURITY.md` and no private channel, filing
an issue **is** public disclosure of a remote denial of service.
That is a decision, not a formality. Points either way:

- For filing publicly: it is two lines of obvious code in a
  public crate, trivially found by anyone reading the accessor;
  the impact is a panic rather than memory unsafety; and there is
  no supported private alternative.
- Against: it is a one-packet denial of service against anything
  built on this crate, and the maintainers have had no chance to
  ship a fix first.

A middle path, if preferred: open the pull request alone, worded
as the input-validation fix it is and without the words that make
it a scannable advisory, and raise the severity with the
maintainers once it has landed. The patch is written so that this
works -- it describes the defect accurately and does not editorialise
about exploitability.

## Affected versions

0.15.0, the current release on crates.io, and `master` at
`5ba6f4e7781b` (2026-11-10), whose `Cargo.toml` says `0.16.0` --
a version prepared but not yet published. Both accessors carry the
defect at the same lines in both, so a fix would ship in 0.16.0.

The patch is against `master`, and was built and tested on the
toolchain `rust-toolchain.toml` pins (1.90.0).
