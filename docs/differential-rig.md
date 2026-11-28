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

## The rig must not run dnsmasq

Not in CI, not in a container, not on a developer's machine. A
measurement is only worth citing if it was taken in a known
configuration on a known kernel with known versions, written up, and
kept -- which is what a lab deploy gives and a test container does
not. A dnsmasq started by `cargo test`
would produce numbers nobody could reproduce or defend, and would
make the suite depend on a GPL binary being installed. So the corpus
is captured bytes, taken in the lab, and the rig compares against
those bytes offline, with no network and no privilege.

No lab is kept alive between phases, because a lab kept alive is a lab
that drifts. It is deployed from one command by
`playbooks/linux-bridge-lab-debian-13.yml` in the private
`homelab-deployments-lfs` repository: two overlapping networks, four
clients and a real dnsmasq. **A phase that needs a measurement
redeploys that playbook rather than inventing a rig of its own**,
captures what it needs, and adds the measurements it takes to the
provenance record as rows. There is no second rig to write.

A redeployment is not the same lab, and the write-up says so: the
rows citing experiments 2 to 5 were taken on one deployment and those
citing experiment 6 on another, with its own dnsmasq package version
and its own kernel. Where the second reproduced a configuration the
first had used -- the same range and the same lease time on `gw-n1` --
the write-up says that too, because it is what lets a case captured on
one deployment be compared with a measurement taken on the other.

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
   bytes are evidence for and the configuration the capture was taken
   against. Add the row to the provenance record in the same commit if
   it is not already there; the harness fails a case that names a row
   which does not exist.
6. Write up the run in the lab document, so the `write_up` field
   points at a command and its verbatim output.

Nothing in that sequence is a reading of dnsmasq's source, and
nothing in it should be, including its documentation text.

### A crafted request, and why it does not break the rule

The rule above is about the **reply**: that is the measurement, and it
has to be bytes a running program was seen to emit. A **request** may
be crafted, and for some of what the corpus has to record it must be,
because no real client sends an `hlen` of zero or a `chaddr` of all
zeroes on purpose and no client command will do it on request.

What keeps a crafted request honest is that it is still a frame that
really crossed the wire: a script builds it octet by octet, sends it
on the bridge from inside a client endpoint, and the capture records
it alongside whatever answered. So the case's `client` field names the
script rather than a client program, the write-up holds the script's
own output -- the probe's name and the payload it sent, in hex -- and
the exchange is as real as any other. What is forbidden is inventing a
reply, or inventing a request and *labelling* it as a client's.

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

A case whose capture recorded **no reply** has no `response.hex`, and
that absence is a measurement rather than an omission: "the reference
implementation did not answer this" is a behaviour, and a corpus that
could not record one would have to leave it out. `response` and
`capture.response_bytes` are then both absent, together; one without
the other fails.

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
response = "response.hex"                # absent where nothing replied

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
response_bytes = 342                    # absent where nothing replied

# The configuration the capture was taken against, as data.
[network]
name = "n1"
interface = "gw-n1"                     # named under the namespace `root`
server_address = "10.0.0.1"             # what the interface was observed to hold
prefix_length = 24
lease_seconds = 43200

[[network.ranges]]
name = "pool"
first = "10.0.0.100"
last = "10.0.0.150"

# Any lease the server already held when the exchange was captured.
[[network.leases]]
address = "10.0.0.128"
mac = "fa:16:3e:00:01:0a"

[comparison]
subject = "dhcpv4"   # which of teneral's front ends answers this
status = "pending"   # "pending" or "compared"; see below
expect = "answer"    # or "silence", where teneral sends nothing
fields = ["xid", "chaddr", "option-codes", "option-51"]
agree = ["yiaddr-matches-ip-destination"]
diverges = ["siaddr", "payload-length"]
asserts = """
What the comparison asserts: what must match, what deliberately need
not, and why.
"""
```

Unknown keys are an error, so a mistyped `provenence` fails instead
of leaving a case that cites nothing while looking as though it cites
something. `request_bytes` and `response_bytes` are the integrity
check on the fixtures: an accidental edit to a frame fails here rather
than quietly changing what the corpus claims was observed.

#### Why a case carries a configuration

A DHCP reply is not a function of the request alone. The lease time
comes from the network, the mask and the broadcast address from the
prefix the arrival interface holds, and which address is offered
depends on what the store has already given away -- so a subject that
took only a datagram could answer only against a configuration nobody
had written down, and the rig would be comparing teneral's reply with
an answer to a different question.

`[network]` is therefore the experiment's own configuration, as data
rather than as prose in `capture.server`. The harness builds a store
from it in memory, through the store's own public writes and as an
ordinary operator uid, and asks the subject to answer the captured
request against it. `[[network.leases]]` is part of that configuration
in the same sense as the ranges: a renewal, a release and a refusal
are all answers to a client the server has a record of, and each lease
listed was granted by an earlier exchange of the same capture.

#### What a case may assert on

`fields` names what must match the capture, `agree` names what must
hold within each reply on its own, and `diverges` names what must
*differ* because teneral differs on purpose. A case that names nothing
in any of the three fails like a case that names no provenance row:
both read as settled and establish nothing. The exception is a case
whose `expect` is `silence`, where there is no reply to have fields.

| Name | What it compares |
|------|------------------|
| `op`, `htype`, `hlen`, `hops`, `xid`, `secs`, `flags`, `ciaddr`, `yiaddr`, `siaddr`, `giaddr`, `chaddr`, `sname`, `file` | that field of the BOOTP message |
| `option-codes` | the option codes the reply carries, in order |
| `option-<code>` | the value of one option, which both replies must carry |
| `ethernet-destination` | the hardware address the frame went to |
| `ip-source`, `ip-destination` | the addresses the packet went between |
| `payload-length` | the octets of BOOTP message |
| `yiaddr-matches-ip-destination` | that the address offered is the address the reply went to |
| `chaddr-matches-ethernet-destination` | that the hardware address named is the one the frame went to |

The two agreements are how a case holds a reply whose address teneral
chose differently: which address comes out of a pool is teneral's own
choice and dnsmasq's is by client identity, so a case asserts that the
two fields agree with each other rather than that either equals what
dnsmasq picked.

`ethernet-destination` cannot be asserted on a reply teneral sends by
ordinary routing, because the kernel resolves that address and teneral
never chooses one; the harness says so rather than comparing a zero.
Nothing compares the IPv4 identification, the differentiated-services
field or either checksum: for two of teneral's three delivery paths
the kernel writes them, so they are not teneral's bytes.

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
5. Every case says what it asserts, in prose, and names at least one
   field assertion. An empty `asserts` leaves a captured exchange in
   the tree with nothing recorded about what it is meant to establish,
   and a case naming no field reports a comparison that checks
   nothing.
6. A case's `status` agrees with the subjects registered in
   `subject.rs`: `pending` with no subject, `compared` with one. Both
   mismatches fail, so a front end cannot land with its cases left
   pending, and a case cannot claim a comparison that nothing
   performs.
7. Where the status is `compared`, the subject answers the captured
   request against the case's `[network]`, and every field the case
   names is checked.

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
subject in `teneral/tests/differential/subject.rs` -- a name, and a
function from the case's configuration and the captured request frame
to what teneral did -- and flips its cases from `status = "pending"`
to `"compared"` in the same commit.

**The comparison is field-wise and reports every difference.** An
earlier version of this document promised the opposite -- that the
harness "fails on a difference, naming the first byte that differs" --
and that was wrong rather than aspirational. Three things in a reply
cannot be compared byte for byte without asserting something false:
which address comes out of a pool, which is teneral's own choice; the
two checksums, which whoever puts the frame on the wire recomputes;
and the IPv4 identification and differentiated-services fields, which
on two of teneral's three delivery paths the kernel writes. So a case
names the fields it asserts, and a failing case lists all of them --
one byte offset tells a reader that two frames differ, where a list of
fields tells them what teneral got wrong, which is usually more than
one thing and never usefully described by whichever comes first in the
frame.

**What teneral did includes where it sent it.** A subject returns the
reply's bytes together with the delivery the send path chose --
broadcast, unicast by ordinary routing, or a frame below the IP layer
-- because half of what this corpus has to defend is the addressing: a
reply to a client with no address goes to an address nobody holds yet,
in a frame addressed to the hardware address the client does hold. The
delivery is a value rather than a socket operation precisely so that
it can be compared here, where no socket below the IP layer may be
opened and there is one interface to send on.

**A silence is an answer and is compared as one.** `expect =
"silence"` says teneral sends nothing, and the case fails if it sends
something. Two kinds of case use it: one where the capture holds no
reply either, and one where the reference implementation answered and
teneral deliberately does not. The second is the more valuable, and
the harness says which it is in the failure message.

A deliberate divergence from dnsmasq is asserted rather than deleted.
A case that is removed because teneral does something else on purpose
is indistinguishable, a year later, from a case nobody wrote -- so the
field goes in `diverges`, where it fails if it ever starts matching
and somebody then decides whether the case or the code moved.
