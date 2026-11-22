# The differential rig

Teneral's bytes beside a real dnsmasq's, case by case, from a corpus
of exchanges captured in a lab.

The rig exists because of a licence. dnsmasq is GPL-2/3 and teneral
is Apache-2.0, so dnsmasq's source may be read to work out *what* to
build and never copied from. For the behaviour the RFCs specify that
costs nothing: the RFC is the better source anyway. For the behaviour
no RFC specifies -- tag-matching precedence, the `dhcp-range` mode
keywords, the hosts-file quirks, the lease-file format -- the only
lawful way to learn what dnsmasq does is to run it and record what it
did. The recording is the citation, and this rig is where the
recordings live. See [the provenance record](provenance.md), which
every case names a row of.

## It cannot compare anything yet, and that is the design

Two reasons, and neither is a gap to be plugged by making the rig
cleverer.

**There is nothing of teneral's to compare.** The repository holds no
DHCP or DNS code, so there is no reply of ours to put beside a
captured one. Every case is therefore `pending`: the harness says so
on every run, by name, rather than reporting a pass it has not
earned.

**The rig must not run dnsmasq.** Not in CI, not in a container, not
on a developer's machine. A measurement is only worth citing if it
was taken in a known configuration on a known kernel with known
versions, written up, and kept -- which is what a lab deploy gives
and a test container does not. A dnsmasq started by `cargo test`
would produce numbers nobody could reproduce or defend, and would
make the suite depend on a GPL binary being installed. So the corpus
is captured bytes, taken in the lab, and the rig compares against
those bytes offline, with no network and no privilege.

The lab the current rows were measured in was deliberately not kept,
because a lab kept alive is a lab that drifts. It is deployed from one
command by `playbooks/linux-bridge-lab-debian-13.yml` in the private
`homelab-deployments-lfs` repository: two overlapping networks, four
clients and a real dnsmasq. **The phase that builds the DHCPv4 front
end redeploys that playbook rather than inventing a rig of its own**,
captures what it needs, and adds the measurements it takes to the
provenance record as rows. There is no second rig to write.

## How a captured pair gets into the corpus lawfully

A fixture is bytes observed coming out of a running program. It is
never a transcription from anybody's source, never a frame somebody
assembled by hand from an RFC and labelled as observed, and never an
expected reply written to make a test pass. That rule is the whole
value of the corpus: a case asserts that something *was seen to
happen*.

1. Deploy the lab from the playbook, and record the versions -- the
   dnsmasq build, the distribution, the kernel, the client program --
   and the configuration dnsmasq was given.
2. Capture on the bridge the exchange crosses, with `tcpdump -i
   <bridge> -w <file>.pcap port 67 or port 68` (or 53, for DNS), while
   the client does the thing being measured.
3. Keep the pcap outside this repository and record its `sha256`. A
   pcap is not an artifact a reviewer can read, and the handful of
   frames that matter are better carried as text.
4. Extract the frames that make up one exchange, one file each, as
   hex text in the format below, with the annotations that make them
   readable.
5. Write `case.toml` beside them, naming the provenance rows the
   bytes are evidence for. Add the row to the provenance record in the
   same commit if it is not already there; the harness fails a case
   that names a row which does not exist.
6. Write up the run in the lab document, so the `write_up` field
   points at a command and its verbatim output.

Nothing in that sequence is a reading of dnsmasq's source, and
nothing in it should be, including its documentation text.

## The corpus

One directory per exchange, under
`teneral/tests/differential/corpus/`, named for what it is:

```
corpus/
  dhcpv4-discover-offer-dhclient/
    case.toml          the metadata, below
    request.hex        the captured request, one frame
    response.hex       the captured reply, one frame
```

A directory with no `case.toml` is a failure rather than something
the harness skips: bytes with no metadata name no provenance row, so
there is nothing to say what they are evidence of.

### `case.toml`

```toml
summary = "one line: what this exchange is"

# The provenance rows these bytes are evidence for, by row id. At
# least one, and each must appear in docs/provenance.md.
provenance = ["dhcp-unasked-option-set"]

request = "request.hex"
response = "response.hex"

[capture]
observed = "2026-11-21"                 # the date it was seen
server = "dnsmasq 2.91, Debian 13, ..." # and its configuration
client = "ISC dhclient 4.4.3-P1, ..."
link_type = "EN10MB"                    # what the frames are frames of
pcap = "lbrlab-...-dhcp.pcap"           # kept outside this repository
pcap_sha256 = "81255c28..."
frames = "1 and 2 of 8"                 # which frames of it these are
write_up = "linux-bridge-lab.md, experiment 2"
request_bytes = 342                     # what each fixture decodes to
response_bytes = 342

[comparison]
subject = "dhcpv4"   # which of teneral's front ends answers this
status = "pending"   # "pending" or "compared"; see below
asserts = """
What the comparison asserts, in prose until there is code to assert
it: what must match, and what deliberately need not.
"""
```

Unknown keys are an error, so a mistyped `provenence` fails instead
of leaving a case that cites nothing while looking as though it cites
something. `request_bytes` and `response_bytes` are the integrity
check on the fixtures: an accidental edit to a frame fails here rather
than quietly changing what the corpus claims was observed.

### The fixture format

Two-digit hex bytes, `#` comments to end of line, whitespace
anywhere. The harness strips the comments and decodes the rest, so the
annotations are a reading aid and cannot change a verdict; a token
that is not exactly two hex digits is an error rather than a guess.

```
# Ethernet: dst ff:ff:ff:ff:ff:ff, src fa:16:3e:00:01:0a, type 0x0800
ff ff ff ff ff ff fa 16 3e 00 01 0a 08 00

# Options, one per line, in the order they were sent.
35 01 02                                  # option 53 message-type: OFFER (2)
```

Hex text rather than a binary blob so that a change to a fixture is a
diff a human can read. A fixture is a whole frame, link layer
included, because the measurements the rig has to defend include where
a reply was addressed and not only what it said.

## What the harness checks

`teneral/tests/differential/main.rs`, run by `make test`:

1. The corpus is not empty. An empty corpus is the one way this rig
   can pass while checking nothing.
2. Every directory loads: `case.toml` is present, parses, and has no
   unknown keys.
3. **Every case names at least one provenance row, and every row it
   names exists.** This is the check the rig is for, and the only one
   that does real work today.
4. Every fixture decodes, to the number of bytes the case recorded.
5. Every case says what it asserts. While a case is `pending` that
   prose is the comparison's only specification, so an empty
   `asserts` leaves a captured exchange in the tree with nothing
   recorded about what it is meant to establish.
6. A case's `status` agrees with the subjects registered in
   `subject.rs`: `pending` with no subject, `compared` with one. Both
   mismatches fail, so a front end cannot land with its cases left
   pending, and a case cannot claim a comparison that nothing
   performs.

What it does not check is anything about DHCP or DNS. The harness
knows about bytes, files and citations; the protocol knowledge
arrives with the front ends.

To see the check that matters fail, break a citation and run the
suite:

```
sed -i 's/dhcp-unasked-option-set/dhcp-unasked-option-sets/' \
    teneral/tests/differential/corpus/*/case.toml
make test
```

It names the case, the row it cited, the closest row that does exist,
and what to do about it.

## Turning a pending case into a comparison

When a front end can answer a captured request, it registers a
subject in `teneral/tests/differential/subject.rs` -- a name and a
function from the captured request frame to teneral's reply frame --
and flips its cases from `status = "pending"` to `"compared"` in the
same commit. The harness then runs the comparison and fails on a
difference, naming the first byte that differs.

A deliberate divergence from dnsmasq is asserted rather than deleted.
A case that is removed because teneral does something else on purpose
is indistinguishable, a year later, from a case nobody wrote.
