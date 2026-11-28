# The differential rig

Teneral's bytes beside a real dnsmasq's, case by case, from a corpus
of exchanges captured in a lab. dnsmasq is GPL-2/3 and teneral is
Apache-2.0, so for behaviour no RFC specifies, watching dnsmasq run
and recording what it did is the only lawful source -- and a
recording is only worth citing if it is kept. The corpus is where
they are kept, and every case names the row of
`docs/provenance.md` it is evidence for.

**The rig runs no dnsmasq, ever.** Not in CI, not in a container, not
on a developer's machine. A measurement is worth citing only if it was
taken in a known configuration on a known kernel and written up, which
is what a lab deploy gives and a test container does not. The lab is
`playbooks/linux-bridge-lab-debian-13.yml` in the private
`homelab-deployments-lfs` repository, and what it measures arrives here
as captured bytes and as rows of the provenance record. **There is no
second rig to write.**

What the harness always checks is the citation: a case that names no
provenance row, or names one that is not in the record, fails. Where a
subject is registered it also compares, field by field, against the
configuration the case records -- and reports every difference rather
than the first.

`../../../docs/differential-rig.md` is the documentation: how a
captured pair gets into the corpus lawfully, the two file formats,
every check the harness makes, and how a front end turns a pending
case into a comparison. Read it before adding a case.

## What is here

| Path | What it is |
|------|------------|
| `main.rs` | The harness. `make test` runs it. |
| `corpus.rs` | A case's metadata, and loading the corpus. |
| `fixture.rs` | Decoding a captured frame from hex text. |
| `frame.rs` | Reading a decoded frame apart into fields, for the comparison. |
| `compare.rs` | The field-wise comparison, and the message each difference gets. |
| `provenance.rs` | The row ids `docs/provenance.md` declares. |
| `subject.rs` | What a captured request can be compared against. |
| `dhcpv4.rs` | The DHCPv4 front end as a subject: the real parser, the real decision layer and a real store. |
| `corpus/` | One directory per captured exchange. |

## Two consequences of `harness = false`

The target is declared in `teneral/Cargo.toml` with `harness =
false`, so it is this `main` that runs rather than libtest. That is
what makes `make test` print the corpus case by case: libtest
captures a test's output and shows it only when the test fails, and a
rig whose passing runs say nothing is a rig that can stop checking
things without anyone noticing.

It costs two things, both small and both worth knowing. `cargo test
<filter>` does not filter inside this target -- the whole rig runs or
none of it does -- and nothing here is a `#[test]`, so
`allow-unwrap-in-tests` does not reach it. The harness obeys
`unwrap_used` like ordinary code, which is why it threads `Result`
rather than unwrapping, and why a failure arrives as a message naming
the case rather than as a panic.
