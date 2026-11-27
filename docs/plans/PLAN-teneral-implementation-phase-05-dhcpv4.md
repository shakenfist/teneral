# Teneral implementation phase 5: the DHCPv4 front-end

## Planning effort

High, on two of this repository's own criteria at once: this phase
parses and emits bytes from the network, and it decides which
network a request belongs to. It also makes the first write to the
store that no human asked for -- a lease the datapath recorded --
and it is the first time anything in this tree will serve two
overlapping networks from one process, which is the property the
whole project exists for and has so far only been measured with a
Python spike.

Review effort: high. A defect in a packet parser is a remote
denial of service against every network this process serves, and a
defect in the interface-to-network step is a silently wrong answer
inside somebody's guest hours later. Both are the failures the
repository's conventions are shaped around.

## Scope

**In.**

- A DHCPv4 front-end, in a new crate, in three layers: the wire
  (parse and build), the decision (store reads, lease writes), and
  the datapath (sockets, and the frame that reaches a client with
  no address).
- The messages a server answers: DISCOVER, REQUEST in both of its
  states, DECLINE, RELEASE and INFORM, emitting OFFER, ACK and NAK.
- The option set an answer carries, which is measured rather than
  chosen: `dhcp-unasked-option-set` in `docs/provenance.md`.
- The one piece of configuration a DHCPv4 answer cannot be derived
  from anything else: the lease time. It becomes a column on the
  network, reachable over the control API, because there is no
  configuration file and a front-end holds no state of its own.
- The ingress guard the staged upstream patch exists for:
  `hlen > 16` is refused before `chaddr()` is called, together
  with a corpus of malformed packets that must not panic.
- The sweeper, its interval, and the record of what choosing that
  interval also chose.
- Wiring into `teneral serve`, the capability set it needs, and
  what the daemon does when it is asked to serve DHCP and cannot.
- The lab: redeploy `playbooks/linux-bridge-lab-debian-13.yml`,
  run teneral in it against two overlapping networks, capture the
  exchanges, and add the provenance rows the captures support.
- The differential rig turned from a frame into a comparison: a
  case gains the configuration its assertion depends on, the
  subject gains the parameter that carries it, and the one
  existing case stops being `pending`.

**Out, with the reason.**

- **Namespaces and `setns`.** Phase 8, and the lab's root
  configuration is exactly the shape this phase wants: two
  overlapping networks, two interfaces, one process, no
  namespaces. Nothing here may make the namespace case harder, and
  decision 8 is the one place that is a real risk.
- **Arbitrary and vendor options, and per-binding overrides.** The
  measured set is what a client that asks for nothing receives, and
  it is derivable. An option grammar is the thing this project was
  able to delete -- `AGENTS.md` says so -- and inventing one before
  a consumer has asked for a specific option would be inventing it
  from the wrong end. Decision 4 says what arrives instead and what
  it would take to extend it.
- **BOOTP relay.** A request with a non-zero `giaddr` is not
  answered, and decision 6 says why that is a refusal rather than
  an omission. Neither consumer relays: home networks are flat and
  Shaken Fist puts a gateway on the bridge.
- **DNS registration of a lease.** Phase 6 owns the zone, and a
  front-end that registered names into a resource the store does
  not hold would be building half of that phase in the wrong
  place.
- **Packaging, a unit file and the capability bounding set.** Phase
  7. This phase documents the capabilities it needs and how to run
  it by hand; it does not choose how it is installed.
- **PXE, TFTP and the boot options.** Phase 10, and the lab cannot
  test them anyway: it boots no guest.
- **DHCPv6, RA and SLAAC.** Out of the whole plan, per open
  question 6.
- **Any mitigation of the recorded-holder behaviour.** Decision 11
  declines it, and says what it does instead.

## What the survey found

The master plan's phase 5 paragraph is four lines long and three
of them hold. The fourth -- "option encoding" -- is where the
staleness is, and it is load-bearing enough to reshape the phase.
Everything below was checked against the tree on 2026-11-26.
Corrections are made at their source in the same commit as this
plan, so the next reader does not trip over them; the finding says
which.

**1. The store holds no DHCP options, and no subnet, gateway,
nameserver or lease time either.** `Network` is a name, a cursor
and a writer (`teneral-store/src/model.rs:30`). `AddressRange` is
a span (`:69`). `Binding` is an address and an optional hostname
(`:111`). No migration has an options table or an options column:
`grep -i option teneral-store/src/migrations/*.sql` matches one
comment about an optional hostname and nothing else. The `.proto`
has no option message. The lease duration is a parameter of
`acquire_lease`, `allocate_lease` and `renew_lease`, supplied by
the caller on every call. So of the nine options dnsmasq was
measured to send, the store can supply exactly one value: the
address.

This is not a gap phase 3 recorded. The master plan's phase 3
summary says it deferred content blobs to phase 10 and zones and
records to phase 6, and says nothing about options; `ARCHITECTURE.md:38`
states that the store's facts include "protocol options", and
`:49` lists what is deferred without listing them. The claim is
false and phase 4's step 4g did not catch it because it was
sweeping a different claim. **Corrected at its source** in this
plan's commit.

What makes it load-bearing rather than tidy-up is the other half:
`docs/binding-store.md:8-10` says every front-end is a translation
between a wire protocol and a question asked of this store, and
that **no front-end holds per-network state of its own**;
`ARCHITECTURE.md` says it again for the front-end layer. A DHCPv4
front-end that kept a lease time, a mask and a router per network
would break a stated invariant in its first commit. Decisions 3
and 4 resolve it, and they resolve it by making almost all of it
derivable rather than by building a configuration surface.

**2. The differential rig cannot express the one case it holds,
and its two documents disagree about what a comparison is.** The
subject is `answer: fn(&[u8]) -> Result<Vec<u8>, String>`
(`teneral/tests/differential/subject.rs:21`), a pure function from
frame to frame, with nowhere to say what the network was
configured as. `Case`, `Capture` and `Comparison`
(`teneral/tests/differential/corpus.rs:27-88`) carry no
machine-readable configuration: the configuration of the existing
case lives in two prose strings, `capture.server` and
`comparison.asserts`. And the case's own assertion is explicitly
not byte equality -- it says which three fields are *not* compared
(yiaddr's exact value, the IP destination, and the checksums) and
why -- while `docs/differential-rig.md:192-203` says the harness
"runs the comparison and fails on a difference, naming the first
byte that differs". Both cannot be true. Decision 12 settles it,
and the work is a step rather than a footnote.

**3. `make test` cannot open an `AF_PACKET` socket, and this is
measured rather than assumed.** The Makefile runs the build image as
the invoking uid (`Makefile:57`, `-u $(UID):$(GID)`) with no
`--cap-add`, under `--network none`. `CapEff` is empty there, so
`socket(AF_PACKET, SOCK_RAW)` returns `EPERM`, and the container has
loopback and nothing else -- so the half of the datapath that reaches
a client with no address cannot be exercised by the repository's own
test target at all. This is a design input, not an obstacle: it is
why decision 2 puts everything that decides anything above the
socket, and why the socket layer is as thin as it can be made.

This finding said more than that when the plan was written, and the
rest of it was wrong. It claimed that port 67 could not be bound
either, from `net.ipv4.ip_unprivileged_port_start` being 1024 and a
`bind` of UDP 67 as uid 1000 returning `EACCES`. That measurement
was taken on the host rather than in the container: docker gives
each run a network namespace of its own and sets the sysctl to zero
in it, so an unprivileged uid binds port 67 there without complaint.
`SO_BINDTODEVICE` likewise needs no capability on this kernel, where
it once needed `CAP_NET_RAW`. Step 5e measured both and said so, and
the plan is corrected here rather than in a footnote. Nothing in the
design turns on it: `--dhcp-port` exists because a test must not take
a well-known port, which is true however the kernel feels about it,
and `AF_PACKET` remains the one hard barrier.

**4. The view bound does not cover a datapath, and the master plan
names the wrong phase for noticing it.** `Views` is `pub(crate)`
(`teneral-api/src/views.rs:59`) and `lib.rs` does not export it,
so nothing outside `teneral-api` can take a permit. The module's
own comment says "a datapath sharing this process will one day
open its own", which is right. The master plan's *Future work*
entry says the two counts disagreeing is the arrangement "which
phase 8 will make matter by sharing the process with a datapath"
(`:1248`) -- but phase 5 is where a datapath first shares the
process. **Corrected at its source.** Decision 9 says what the
datapath does instead.

**5. DECLINE is missing from the sketch's message list,** which
reads "DISCOVER, REQUEST, ACK, NAK, RELEASE, INFORM". RFC 2131
defines DHCPDECLINE, and a client sends it to say the address it
was offered is already in use by something else -- which in a
network teneral is serving means the store believes something
false. There is no way to record that: no lease state for it, no
exclusion, and nothing a later allocation would consult. Decision
6 says what this phase does with one, and it is deliberately the
cheap answer rather than the complete one.

**6. Nothing sweeps, and the first caller appears in this phase.**
`sweep_expired_leases` is on the trait and no code outside the
store's own tests calls it: `grep -rn sweep teneral/src
teneral-api/src` is empty. The daemon has no timer. The master
plan already warns that the interval is also an address-stickiness
knob (`:1193`) and asks this phase to choose it knowing that.
Decision 10 chooses, and says which of the two it optimised for.

**7. Nothing reserves a namespace name for interfaces that are in
no namespace, and one character is already forbidden.** An
attachment's identity is `(namespace, ifname)`, the namespace is a
required string everywhere it appears
(`teneral/src/cli.rs:126,203`), and nothing defaults it. A datapath
with no namespaces has to pass something, and whatever it passes
becomes the name an operator must type to attach an interface -- and
the name phase 8 has to agree with.

The first version of this finding said nothing validates it, which
was false, and decision 8 was written on that falsehood and had to
be rewritten when step 5d hit it. `attach_interface` runs
`validate_name_part` on both halves (`teneral-store/src/ops.rs:1246`),
which refuses `/` -- because an attachment's identity reaches an
event as one string with the two halves joined by that character, and
the store refuses it in the parts rather than escaping it, so that
`a/b` plus `c` and `a` plus `b/c` cannot become one event. The
comment there makes exactly the argument this plan made for choosing
`/` and reaches the opposite conclusion from it.

**8. The `dhcproto` claim still holds, and was checked rather than
carried forward.** crates.io reports 0.15.0 of 2026-05-19 as the
newest version, so the merged fix (`6194561`, 2026-11-24) is in no
release this project can resolve, and every resolvable version
panics on `hlen > 16`. The guard stays, as the master plan's
*Future work* says it must. The staged patch and its two
reproducers are in `upstream/dhcproto/`; nothing in this phase
sends anything upstream.

**9. `Actor::Daemon` already anticipates this phase** -- its
documentation names "a sweep, a lease the datapath recorded" as
its two cases (`teneral-store/src/actor.rs:14`) -- so the
attribution question phase 4 opened is already answered and this
phase only has to choose the two literals. Nothing wrong.

**10. The address selection order is already in the store, tested
and cited.** `allocate_lease` implements RFC 2131 section 4.3.1's
four steps, with the two inventions documented as this project's
(`lease-address-selection-order`), and the tests include
`the_pool_does_not_offer_an_address_a_binding_reserves` and
`an_address_a_binding_has_since_reserved_does_not_come_back_to_its_old_client`.
The front-end therefore does not implement selection, reservations
or pools: it asks. That removes most of what the sketch's "static
reservations and dynamic pools" sounds like it is asking for, and
it is worth saying so, because a front-end author who did not
notice would write the second implementation this project is
shaped to avoid.

**11. The lab exists and the local checkout cannot see it.**
`playbooks/linux-bridge-lab-debian-13.yml`,
`roles/linux_bridge_lab` and `docs/linux-bridge-lab.md` are all on
`origin/main` of `homelab-deployments-lfs`, merged as `658caab`.
The local `main` there is 13 commits behind and does not have
them, so the step that deploys the lab pulls first. The lab is
left in its *root* gateway configuration, which is the
configuration this phase needs; `lbr-lab-gateway root|netns|show`
switches it, and phase 8 will want the other one.

**12. The provenance rule applies here with no exemption.** Phase
4 was able to say the control API's wire is teneral's own
invention and so has no rows, and `docs/provenance.md`'s header
was scoped to say so. DHCPv4 is the opposite case: every
wire-visible behaviour cites RFC 2131 or a named observed-behaviour
test, and the existing rows already carry the shape of the answer
(`dhcp-unasked-option-set`, `dhcp-reply-unicast-to-offered`,
`dhcp-broadcast-flag-not-permission`, `sock-*`). The phase adds
rows rather than reinterpreting them, and a behaviour that cites
neither is not shipped.

## Decisions

### 1. A new crate, `teneral-dhcp`, because a binary cannot be tested

The front-end is a library crate beside `teneral-store` and
`teneral-api`, not a module of the `teneral` binary. The reason is
mechanical rather than aesthetic: `teneral` has no lib target
(`teneral/src/` is `main.rs`, `cli.rs`, `client.rs`, `serve.rs`),
so an integration test in that crate cannot `use` anything in it
-- `tests/daemon.rs` tests the daemon by running it. The
differential rig lives in `teneral/tests/differential` and has to
call the thing that builds a reply. A front-end inside the binary
would therefore be a front-end the rig cannot reach, and the rig is
the project's stated way of proving wire behaviour.

Two consequences worth stating. The crate depends on
`teneral-store` for the trait and **not** on `teneral-api`: a
front-end is a consumer of the store, not of the control plane,
and a dependency the other way would make the layer diagram a lie.
And `make lint`'s final `cargo check --frozen -p teneral-store`
stays exactly as it is -- a fourth crate taking a tokio runtime
feature is precisely the case that check exists for, and phase 4
found it the hard way.

### 2. Three layers, with the seams chosen so the untestable part is tiny

Inside the crate:

- **`wire`** -- bytes to a parsed request, and a set of answer
  parameters to bytes. No store, no socket, no clock. Every
  malformed-packet test and every option-encoding test lives here,
  and so does the `hlen` guard.
- **`decide`** -- a parsed request plus the interface it arrived
  on, against a `BindingStore`, producing an answer or a silence.
  This is where a network is resolved from an interface, where an
  identity is assembled, where the store is asked and written, and
  where every two-network test in the phase lives.
- **`datapath`** -- the sockets. Receiving with `IP_PKTINFO`,
  naming the egress per message, and the `AF_PACKET` frame for a
  client with no address.

The seam between `decide` and `datapath` is a trait with one
method -- "send these bytes to this client on this interface" --
so `decide` is tested against a recorder and the real thing is a
single implementation of it.

This is the shape finding 3 forces: `make test` cannot open an
`AF_PACKET` socket and has one interface, so anything decided
inside the datapath is decided where the repository's tests cannot
see it. Pushing every judgement above that line is what keeps the
untested surface down to "the bytes went out of the right fd",
which is the one thing the lab measures directly.

### 3. The lease time is a column on the network, and nothing else is added

`network` gains `lease_seconds`, written by a new
`update_network`, served over the control API, with a schema
default of 43200 -- twelve hours, which is the lab's
`dhcp-range=...,12h` and therefore the number the one existing
differential case was captured against.

It goes on the **network** rather than on the address range, and
this is the decision in this plan most likely to be argued with.
The case for the range is real: that is where dnsmasq's lease time
lives, the measured 12h came from a `dhcp-range`, and a network
with two ranges might reasonably want two lease times. The case
against it is that teneral's range is not dnsmasq's. dnsmasq has
no object for "network" at all -- its range *is* its scope, which
is why the lease time has nowhere else to go, and
`dhcp-ranges-merge-in-subnet` records what that costs. Teneral has
a real network, every resource is keyed by it, and a binding's
address need not lie inside any range -- so a lease time on the
range leaves "what lease time does a reserved address outside
every range get" as a question with no answer, and the obvious
patch for that is a network-level default, which is the thing
being avoided. One value, in the scope everything else in this
store is keyed by, with a per-range override deferred until a
consumer asks for two lease times in one network.

The number lives in the migration and nowhere else. There is no
`--lease-time` flag: a daemon-wide flag for a per-network value
would be configuration outside the API, which is the invariant
`AGENTS.md` puts first, and it would be a second home for a number
the store already holds.

### 4. Every other option is derived from the arrival interface, as measured

`dhcp-unasked-option-set` records exactly what dnsmasq sends a
client that asked for nothing: 53, 54, 51, 58, 59, 1, 28, 3 and 6,
with T1 at half the lease and T2 at seven-eighths, and **router
and dns-server defaulting to the server's own address**. Teneral
emits the same nine, and takes each from the only honest source:

| Option | Source |
|--------|--------|
| 53 message type | the answer being built |
| 54 server identifier | the address of the interface the request arrived on |
| 51 lease time | the network's `lease_seconds` (decision 3) |
| 58, 59 T1, T2 | half and seven-eighths of it, measured fractions |
| 1 subnet mask | the prefix configured on that interface |
| 28 broadcast | that address and mask |
| 3 router | the interface's own address, as measured |
| 6 nameserver | the interface's own address, as measured |

Reading an interface's address and prefix from the kernel is
observation, not per-network state: nothing is remembered between
packets, so the invariant in `docs/binding-store.md:8-10` holds.
An interface with no address, or with several, is a refusal rather
than a guess -- a server that picked one of two addresses to call
itself would be answering differently depending on the order
`getifaddrs` happened to return, and the one thing worse than not
answering is answering inconsistently. The refusal is logged
naming the interface, because it is a configuration fault with an
operator-shaped fix.

What this defers, explicitly: an operator who wants a router or a
nameserver that is not this machine, a domain name, an MTU, or any
option outside the nine. Each of those is a row in a table that
does not exist yet, and the shape of that table should be decided
by the first consumer that names an option it needs rather than by
guessing which of dnsmasq's 286 directives matter. The *Future
work* entry this phase writes says so and names the obvious shape
(an option code, a network or a binding, and bytes), so that the
deferral is a decision and not an oversight.

### 5. `hlen` is refused before anything reads `chaddr`, and the corpus proves it

`Message::chaddr()` in dhcproto 0.15.0 slices the 16-byte BOOTP
field by `hlen`, an unvalidated byte from the packet: above 16 the
owned path panics and the borrowed path returns up to 255 bytes of
neighbouring packet as the client's hardware address. Finding 8
confirms no release carries the fix. So the first thing `wire`
does with a buffer is refuse `hlen > 16`, before any accessor is
called, and the refusal is a named error rather than a clamp --
clamping would invent a hardware address for a packet that does
not have a valid one.

This is the project's `unwrap_used = "deny"` rule reaching the
place it was written for, and a guard is not enough on its own. The
step also lands a corpus of malformed inputs -- truncated at every
field boundary, `hlen` at 0, 16, 17 and 255, an option that runs
off the end, a zero-length and a 65507-byte datagram, options with
a length that overruns, a magic cookie that is wrong, and a
`chaddr` of all zeroes -- each with a test that names it. Most are
refused; `hlen` of exactly 16 is the boundary and is *accepted*,
because 16 is the field's size and a guard that refused it would be
an off-by-one shipped as a safety check. A panic in this layer is a
remote denial of service against every network this process serves,
which is why `AGENTS.md` makes that lint `deny` rather than `warn`.

**A framing error refuses the datagram and a bad value does not.**
This distinction was not in the plan and arrived from the step,
which found that the decoder stops at the first option it cannot
read and returns *success* -- so a packet with one bad option would
have parsed cleanly and short, silently missing whatever followed,
which could be the requested address or the client's identifier. The
answer is to walk the option TLVs before decoding, and then the two
failures are distinguishable and must be treated differently. A TLV
that runs past the end, or a length byte that is not there, means the
option stream's boundaries cannot be trusted, so the datagram goes. A
value a typed decoder declines does not: the option is dropped, the
request records which code was dropped, and the client is still
answered. The reason is that the strict reading falls on ordinary
clients -- a Windows machine with a non-ASCII name sends option 12 in
a local codepage, and losing that name should cost the name and not
the lease. Nothing is lost on the options that matter, because each
of those is asked for by name and its absence is already a refusal.

**Five narrowings the parser makes that this plan did not ask for,
kept deliberately.** A message whose `op` is not a request is
refused, because a reply arriving on port 67 is another server's
traffic rather than a question, and a message type outside the five a
client sends is refused for the same reason. A plain BOOTP request,
which carries no message type at all, is refused, so this front-end
serves no BOOTP-only client -- the reference implementation does, and
that is a feature not built rather than an accident. And a message
with `hlen` of zero is refused, as is one whose `chaddr` is all
zeroes. Both are addressable in
principle -- a client with no hardware address is identified by its
client identifier, and RFC 4390 specifies exactly that shape for
InfiniBand -- but this project's reply path hand-builds an Ethernet
frame, and neither case leaves an address to send it to. Every one of the
five is a refusal with a named error that the daemon logs rather than
a silent drop, 5g measures what the reference implementation does
with each, and 5h documents them as limits rather than leaving them
to be found.

**The cost of a packet is a constant.** Dropping an option and
decoding again is a pass over the whole datagram, and the number of
passes is bounded by the number of codes the decoder has a type for
-- so a datagram packed with two-byte records whose values are all
refused would buy a sender some sixty full decodes for one packet.
The datapath answers one packet at a time per socket by decision 9,
so that amplification would land on the one thread serving every
network in the process. Four drops is the bound, and beyond it the
datagram is refused: a client offering one or two options this
decoder declines is the ordinary case the paragraph above is about,
and one offering five is not a client whose lease anybody is
protecting.

When dhcproto next publishes, the question becomes whether the
floor in `Cargo.toml` can require it and the guard can go. That is
a dependency decision for whoever sees the release, and the
*Future work* entry already says to watch for it rather than for a
review.

### 6. Which messages are answered, and what the other cases get

| Message | Answer |
|---------|--------|
| DISCOVER | OFFER, from `allocate_lease`, honouring a requested address only at its place in the order |
| REQUEST, selecting (server identifier present) | ACK if the server identifier is ours and the address is allocatable to this client, else NAK; silence if it names another server |
| REQUEST, renewing or rebinding (no server identifier) | ACK from `renew_lease`, NAK if the store refuses because another client holds it or the lease has lapsed |
| RELEASE | `revoke_lease`, and no reply -- the protocol specifies none |
| DECLINE | recorded in the log, the lease revoked, and no reply |
| INFORM | ACK carrying the derived options and no lease, no lease time and no yiaddr |

Two of these are deliberately the cheap answer.

**DECLINE** says the address teneral just handed out is already in
use by something the store does not know about. The complete
response is to stop offering that address, which needs a state the
store does not have -- an exclusion, consulted by allocation,
expiring on its own. This phase revokes the lease and logs the
event, so the address goes back to the pool and the next client
may be offered it again. That is honest and it is a loop if the
conflict is permanent. Doing better is a store change with a
retention policy of its own, and it is written up as future work
with its cost; what is *not* acceptable is silence, because a
DECLINE is the clearest evidence a server ever gets that its
belief is wrong, and dropping it on the floor is how an operator
never learns.

**A non-zero `giaddr`** -- a relayed request -- is not answered,
and the drop is logged with the giaddr once per relay rather than
once per packet. Answering a relay correctly means replying to the
relay's address, taking the subnet from `giaddr` rather than from
the arrival interface, and therefore means a subnet model this
phase does not build (decision 4 derives everything from the
arrival interface). Answering it incorrectly means a lease in the
wrong subnet. Neither consumer relays. The log line is what makes
this distinguishable from a silence nobody chose.

### 7. The reply path is the measured one, and the flag is not permission

Three measured rows decide this and there is no judgement left in
it. A reply to a client with no address is unicast to the address
being offered (`dhcp-reply-unicast-to-offered`), which ordinary IP
routing cannot deliver (`sock-unicast-fails-at-ip`) and which
works below the IP layer, in a frame addressed to the MAC the
client does hold (`sock-unicast-works-below-ip`). A broadcast
reply from an interface with no default route needs the egress
named per message (`sock-broadcast-needs-egress`). And a client
that cleared the BROADCAST flag still accepted a broadcast reply
in all seven observed transactions
(`dhcp-broadcast-flag-not-permission`), so the flag is read as a
request for broadcast and never as permission to unicast.

So: receive on a wildcard-bound UDP socket (`sock-bound-misses-broadcast`
-- a socket bound to the specific address never sees
`255.255.255.255`), take the arrival interface and the datagram's
destination from `IP_PKTINFO` (`sock-pktinfo-reports-arrival`), and
reply through `AF_PACKET` when the client has no address, through
UDP with a named egress when it does. The interface name comes from
an `ioctl` on a socket and never from `/sys/class/net`
(`netns-sysfs-ignores-setns`) -- which costs nothing here, where
there is one namespace, and is the difference between working and
silently wrong in phase 8.

### 8. The namespace an interface in no namespace is attached under is `root`

An attachment is identified by `(namespace, ifname)`; the namespace
is required and opaque. This phase has to pass something, and the
string it picks is the one an operator types for the rest of the
project's life and the one phase 8 has to agree with.

It was `/` and could not be. The argument was that no name `ip netns`
can create contains a slash, those names being filenames, so `/`
could never collide with a real namespace and would need no
validation anywhere. The store had got there first and drawn the
opposite conclusion: finding 7 above has the detail, and the short
version is that `/` is the character an attachment's two halves are
joined by in its event identity, so the store refuses it in either
half. The choice was unimplementable rather than merely awkward, and
step 5d found it by trying.

So it is `root`. Not an empty string, which is indistinguishable
from a field somebody forgot to fill in. Not `.`, which reads as
"here" and is unforgeable for the same filename reason, but which a
consumer that ever puts an attachment's identity in a URL path would
have silently collapsed, this project having a web interface in its
future.

`root` can collide, because `ip netns add root` succeeds, and that is
acceptable for a reason worth stating rather than waved at: the
collision is loud. An attachment's primary key is
`(namespace, ifname)`, so an interface in the initial namespace and
an interface of the same name inside a namespace called `root` claim
one identity, and the second claim is refused naming the holder.
Nothing leaks between networks and nothing is answered wrongly; an
operator is told they cannot attach that interface. A deployment that
has a namespace called `root` *and* serves an identically named
interface in both is pathological, and the worst it gets is a
refusal it can work around by renaming its namespace. Weighed against
a store change to the identity format every event already carries,
that is the cheaper end of the trade.

The cost is paid in the help text, in `docs/`, and in the line the
daemon logs at startup naming the namespace it is serving.

### 9. The datapath handles one packet at a time per socket, and so bounds its own views

The view bound belongs to `teneral-api` and its permits are
`pub(crate)` (finding 4), so the datapath cannot take one and
should not: the bound is a statement about how many *consumers*
may read at once, and a datapath is not a consumer.

Instead the datapath bounds itself by construction. Each socket is
read and answered in turn, so the number of views it can hold at
once is the number of interfaces it serves, and that is a number
an operator configured rather than a number a client can drive.
`live_views()` therefore reports the datapath's views alongside the
API's, the diagnostics show both numbers beside the API's bound,
and the day they disagree is visible -- which is what phase 4 built
them for.

Serial handling is also what the reference implementation does;
dnsmasq is single-threaded. The load this gives up is not load
DHCP produces: a client sends two packets per lease time, and the
lab's four clients at a twelve-hour lease generate eight packets a
day. If a consumer ever appears for which that is wrong, the fix
is a bounded worker set per socket, and the bound it needs is the
one being chosen here rather than a new one.

### 10. The sweeper runs daily, and it is the stickiness that was optimised for

`--sweep-interval`, in seconds, defaulting to 86400, with no sweep
at startup. The master plan asked this phase to choose knowing
that the interval is also how long a client keeps getting its old
address back after its lease lapses, and to say which of the two it
optimised for. **It optimised for stickiness**, and a day is
chosen so that the sentence an operator reads is "a client that was
away for a day gets its address back".

The reasoning is that the two things the interval sets are not
equally valuable. The expiry *event* is a convenience: the store's
own documentation says nothing waits for the sweep, every read
evaluates expiry against the clock, and the lease on the wire
carries both `expires_at` and `expired`, so a consumer that wants
to know an address is free can know without being told. The lapsed
*row* is not a convenience: it is the only thing standing between a
returning client and a different address, because teneral picks the
lowest free address from the pool where dnsmasq picks by client
identity (`dhcp-address-by-identity`), so teneral has no other
source of stability. Sweeping promptly would throw away the only
mechanism it has in order to make a message timelier.

No sweep at startup, deliberately: a daemon that swept as it came
up would make its own restarts the stickiness bound, which is the
one event most likely to coincide with the outage a client is
returning from.

What this does not fix is that the two remain one knob. Decoupling
them means the sweeper emitting the expiry when the lease lapses
and deleting the row later -- two writes, a grace period, and a
row state the schema does not have. That is written up as future
work with the shape named, because a reader meeting an
address-stability complaint should find the design rather than the
symptom.

### 11. The recorded-holder behaviour is declined, and moved where it can be found

The master plan asked this phase to decide whether to mitigate a
client that stops presenting its client identifier not being
recognised as its lease's holder. **Declined**, and the reasoning
is that the obvious mitigation is already there and is not the
problem. `leases_held_by` matches against every attribute in the
identity it is given, ordered by precedence -- it is not restricted
to the recorded one. What cannot be fixed by consulting more
attributes is the other side: the lease records *one* holder, and a
client that no longer presents it is, as far as anything can tell,
a different client.

Doing better means recording every attribute a client presented
when it acquired an address and matching on any of them, which is
a side table, a retention question, and a deliberate decision to
treat two identities as one machine -- the exact guess RFC 2131's
own warning about hardware addresses is about. The store cannot
know, and a store that guessed would hand one client another
client's address in the case where the guess is wrong.

So it stays as it is, and the fix is where it is written down:
`docs/` gains the behaviour, its cause and its consequence (a
second address, with the first held until it lapses), because
today it is explained only in the comment of
`a_lease_is_recorded_against_the_highest_precedence_attribute_presented`,
and a front-end author will not find it there.

### 12. The rig gains a configuration, and a comparison is field-wise

Finding 2 is two problems and both are fixed here.

A case gains a `[network]` table: the ranges, the lease time, the
server's own address and prefix -- the configuration the capture
was taken against, as data rather than as prose in
`capture.server`. The harness builds a store from it, in memory,
and asks the subject to answer the captured request against it. So
the subject's signature becomes "this request, against this
configuration" rather than a pure function of bytes, because a
DHCP reply is not a function of the request alone and a rig that
pretended otherwise could only ever compare a packet nobody
configured.

And the comparison is field-wise, not byte-wise. The existing
case already says so and names the three things that cannot be
compared byte for byte: which address comes out of the pool is
teneral's own choice and dnsmasq's is by identity, so `yiaddr` and
the IP destination are asserted to *agree with each other*; and
the checksums are recomputed rather than compared.
`docs/differential-rig.md:192-203` promises the opposite -- "fails
on a difference, naming the first byte that differs" -- and is
wrong rather than aspirational. The harness compares the fields a
case names, reports every difference rather than the first, and a
case that names no field assertions fails like a case that names
no provenance row.

The old `status = "pending"` path stays, and so does the rule that
a status and a registration which disagree is a failure in either
direction. A front-end that lands without flipping its cases, and
a case flipped before anything can answer it, are both caught.

### 13. The numbers, and where each one lives

| Number | Value | Where it lives | Why this value |
|--------|-------|----------------|----------------|
| Default lease time | 43200s | migration 0008's column default | The lab's `12h`, so the captured case and the store's default agree |
| T1, T2 | 1/2, 7/8 of the lease | `wire` | Measured: `dhcp-unasked-option-set` |
| Sweep interval | 86400s, `--sweep-interval` | `teneral/src/cli.rs` | Decision 10 |
| Maximum datagram accepted | 1500 bytes | `wire` | A DHCP message is 576 bytes by specification and the field that could claim more is the one being guarded; anything larger is refused unparsed |
| Listen port | 67, `--dhcp-port` | `teneral/src/cli.rs` | Finding 3: the port has to move for any test that binds one, and a flag is cheaper than a conditional compile |

`--dhcp-port` exists for testing and says so in its help. It is
not a second configuration surface: the protocol's port is 67 and
a client cannot be told to use another.

### 14. The daemon serves DHCP only when asked, and says what it is serving

`teneral serve --dhcp-interface <ifname>`, repeatable, and absent
means the daemon is exactly what phase 4 shipped. Two reasons for
an explicit list rather than serving every interface the store has
an attachment for: a process that bound a socket because somebody
wrote a row has turned a configuration write into a privileged
action, and the capability set this needs
(`CAP_NET_BIND_SERVICE`, `CAP_NET_RAW`) is one an operator should
be granting deliberately for named interfaces.

The daemon logs, at startup, each interface it is serving with the
network that claimed it, the namespace it is attached under
(decision 8), the address and prefix it derived, and the lease time
it will hand out. That line is the whole of decision 4's derivation
made visible, and it is the first thing to read when a client got an
answer nobody expected. An interface named on the command line that
no network has claimed is a refusal to start, not a warning: a
daemon that bound port 67 and then NAKed everything would be worse
than one that did not come up.

### 15. How a packet's attributes become selector values

This decision was missing and the store says so: `Selector::new`
normalises a hardware address and nothing else, and its comment
states that "the front-end that renders an option into a selector
value owns the choice of form. That choice has to be the same on the
configuring side and the matching side, and this store is not in a
position to check that it is." So the form is this phase's to choose,
and choosing it badly is the project's signature failure in its
quietest form: an operator configures a reservation, every test
passes, and no binding ever matches a real client.

- **The hardware address** is `chaddr` truncated to the `hlen` octets
  the client declared, as lower-case colon-separated hex. The store
  normalises this one, and accepts three written forms, so an
  operator may type any of them and still match.
- **The client identifier**, option 61, is every byte of the option
  including the leading type byte, as lower-case colon-separated hex.
  The precedent is measured rather than invented: the reference
  implementation's lease file renders a client identifier as
  colon-separated hex (`lease-file-format`), so an operator moving
  from it can paste what they already have.
- **The user class and the vendor class**, options 77 and 60, are the
  option's bytes as text when they are valid UTF-8 and the store will
  take them. When they are not -- a control character, or bytes that
  are not UTF-8 at all -- the attribute is simply not presented, and
  the fact is recorded the way a dropped option is. A client whose
  vendor class is unprintable loses that attribute and keeps its
  lease, which is the same ruling as decision 5's.
- **DUID and IAID** are never presented by a v4 client. The kind
  exists for the v6 work that is out of this plan and stays empty
  here rather than being filled with something that looks like one.

What makes this safe rather than merely written down is a test that
goes the whole way round: render a selector value from a packet's
bytes, configure a binding with the value an operator would type,
and match that packet against that binding. A unit test on the
renderer alone cannot see a divergence, because it would be
asserting the renderer against itself. `docs/` carries the forms,
because the operator typing them is the other half of the agreement.

## Step plan

Sequential, every step. All eight write to the same worktree and
run `cargo` through the `Makefile` into one build image, so two at
once would trip over each other's `target/` and each other's
half-finished tree. The order is also a dependency order: nothing
can build a reply before the lease time exists, nothing can decide
before it can parse, and the rig cannot compare before there is
something to compare.

| Step | Effort | Model | Isolation | Brief for sub-agent |
|------|--------|-------|-----------|---------------------|
| 5a | high | opus | none | The one piece of configuration a DHCPv4 answer needs. Read decision 3 and finding 1 of this phase file first. (1) `teneral-store/src/migrations/0009_lease_time.sql`: `ALTER TABLE network ADD COLUMN lease_seconds INTEGER NOT NULL DEFAULT 43200`, with a `CHECK` that it is positive -- follow 0007's shape for an added NOT NULL column on a STRICT table, and register it in `teneral-store/src/schema.rs`, raising `SCHEMA_VERSION` to 9 -- 0008 is the attribution backfill this step also lands, which is explained in the execution notes. (2) `Network` in `teneral-store/src/model.rs:30` gains the field; every construction site in the crate follows. (3) A new `update_network(actor, network, expected: &Cursor, lease_seconds)` on `BindingStore` -- the position before the value, which is the order its sibling takes, compare-and-set on the cursor like `update_address_range` (`store.rs:342`) -- read that one and match it, including the refusal when the position is stale. It appends an event like every other write and returns the position. Run `scripts/check-store-trait.sh` and confirm it reports the new method as `network`-scoped and cursor-returning. (4) The API: a `UpdateNetworkRequest` in `teneral-api/proto/teneral.v1.proto` beside the other network operations, the handler in `operations.rs`, the capability in `authz.rs`'s exhaustive `required_capability` match (`write`), the field on the `Network` resource message **and** in `resource.rs`'s converter and in the event-state mapping, and the row in `docs/control-api.md`'s networks table. `scripts/check-api-docs.sh` must still report every operation documented, both ways. (5) Tests: two networks holding the same address, one with a changed lease time and one without, asserting each reads its own; a stale-cursor refusal; and the default appearing on a network created without one. The debugging client is not touched here: it has no write subcommand of any kind to extend, which is what step 5b is for. Do not add a `--lease-time` flag anywhere -- decision 3 says why. Commit subject: "Give a network the lease time it hands out." |
| 5b | medium | opus | none | Let the debugging client write what it reads. The premise this step exists for was found by checking another step's brief: `teneral/src/cli.rs` exposes `get`, `explain`, `diagnostics`, `mint-token`, `mint-epoch` and `watch`, and **no write of any kind**, so nothing in this repository can create a network, attach an interface or configure a range -- and step 5g cannot configure the lab it deploys. Read `teneral/src/client.rs` in full first: `one()` sends a `request::Kind` and prints the response, `get()` builds the read requests, and `selector`, `cursor_from`, `scope_from` and `capability_from` are the argument parsers to reuse rather than duplicate. The gap is wider than a missing write: the client reaches ten of the schema's thirty-four operations, and the twenty-four it cannot reach are every write, every listing, every lookup, the digest and two of the token operations. Close all of it, because the alternative is finding the next hole in 5g with the lab up and no way to see what was configured. Take `--expected EPOCH:SEQUENCE` where an operation needs a position, with the help text saying it comes from a `get`, and keep the six subcommands that already exist in the shape they have rather than regularising them into something a reader of the existing help would not recognise. Nothing new goes in `teneral-api`: every one of these requests exists in the schema and the server serves it already, so a change in that crate means something has been misread. Add a test that names every operation in the schema and asserts the client builds a request for each, so an operation added later without a subcommand fails rather than being quietly unreachable. That test is the gate, and it is why this is worth doing once rather than twice. Commit subject: "Let the debugging client configure a daemon." |
| 5c | high | opus | none | The wire layer, and nothing above it. Read decisions 2, 4, 5 and 13, and `docs/provenance.md`'s `dhcp-unasked-option-set` row. Create the `teneral-dhcp` crate: `publish = false` (phase 4 found that cargo-deny's wildcard-path allowance does not reach publishable crates), `rust-version.workspace = true`, the workspace lints inherited, `dhcproto = "0.15.0"` and nothing else beyond `teneral-store` -- and **not** `teneral-api` (decision 1). Add it to the workspace members and to `[workspace.dependencies]`. Then `wire`: a parse that refuses `hlen > 16` **before** any `chaddr` accessor is called and a datagram over 1500 bytes before it is parsed, returning a named error per cause; an `Answer` type holding the parameters decision 4's table lists; and an encoder producing the nine options in the measured order with T1 and T2 at a half and seven-eighths. The malformed corpus is the point of this step, not a side-effect: the inputs decision 5 names, each asserting a named refusal and no panic, as table-driven tests with the input described in the test name. Mutate your own guard to prove the tests see it -- copy the tree, remove the `hlen` check, confirm a named test fails, restore from the copy, each run under `timeout` -- and report which test killed it and the script's path. `make lint`, `make test` and `make deny` must all be clean, including the per-crate store check at the end of lint; run `make fetch` before `make deny` so the new dependency tree resolves. Commit subject: "Parse and build DHCPv4 messages safely." |
| 5d | high | opus | none | The decision layer: a parsed request plus its arrival interface, against a `BindingStore`. Read decisions 2, 6 and 11, finding 10, `docs/binding-store.md`'s lookup and lease-lifecycle sections, and the `lease-address-selection-order` and `selector-client-id-over-chaddr` provenance rows. The store already implements address selection, reservations, pools and the precedence order -- **do not reimplement any of it**; assemble a `ClientIdentity` from what the packet presents and ask. The path is: `interface_owner(namespace, ifname)` to get the network, refuse with a log line if nothing claimed it, then `lookup_binding_on_interface` for a reservation, then `allocate_lease` or `acquire_lease` or `renew_lease` or `revoke_lease` as decision 6's table says. The actor is `Actor::Daemon("dhcpv4")`. Decision 15 is the one to read twice: the form a packet's attributes take as selector values is this step's to implement and the store cannot check it, so the test that matters goes from packet bytes through a configured binding and back rather than asserting the renderer against itself. The send side is a trait with one method, so this layer is tested against a recorder. Tests: every row of decision 6's table, and **every one of them with two networks holding the same address** -- a DISCOVER arriving on each network's interface from the same MAC gets each network's own answer, a REQUEST for an address held in the other network is NAKed rather than ACKed, and a RELEASE in one network leaves the other's lease alone. That is the project's central correctness property and this is the layer where it is decided. Also: a request whose `giaddr` is non-zero is dropped with one log line, and an INFORM gets options and no lease. Report the test count. Commit subject: "Answer a DHCPv4 client from the binding store." |
| 5e | high | opus | none | The datapath and the daemon. Read decisions 7, 8, 9 and 14, and the `sock-*` and `netns-*` provenance rows -- every behaviour here is already measured and none of it is to be rediscovered. The receive path: one wildcard-bound UDP socket per served interface (a socket bound to the interface's own address never sees `255.255.255.255`), `IP_PKTINFO` for the arrival ifindex and the datagram's destination, and the interface name from an `ioctl` on a socket rather than from `/sys/class/net`. The reply path: `AF_PACKET` with a hand-built Ethernet/IPv4/UDP frame when the client has no address, and UDP with the egress named per message when it does; the BROADCAST flag is a request for broadcast and never permission to unicast. Phase 1d's spike (iii) in `docs/plans/PLAN-teneral-implementation-phase-01-research-spikes.md:974` built exactly this in Rust and a real client accepted it -- read it and follow what it found, including the privilege. Then the daemon: `--dhcp-interface` repeatable, `--dhcp-port` defaulting to 67 with its testing purpose in the help, the startup line decision 14 specifies, a refusal to start when a named interface is claimed by no network, and shutdown ordered so the sockets close before the store does (`serve.rs` has the existing order and its comment). Nothing here may bind 67 in a test: `make test` runs as a non-root uid with no capabilities and the bind returns `EACCES`, which is why the port is a flag. Say in your report which parts you could not test and why. Commit subject: "Serve DHCPv4 on a socket per interface." |
| 5f | medium | opus | none | The sweeper. Read decision 10, and the `sweep_expired_leases` documentation in `teneral-store/src/store.rs:600` -- particularly the paragraph saying nothing in the store waits for it, which is the premise the chosen interval rests on. A timer in the daemon, `--sweep-interval` in seconds defaulting to 86400, no sweep at startup, running over every network the store holds, with `Actor::Daemon("sweeper")`. It logs what it removed per network, and a sweep that removed nothing logs nothing. Tests: with the store's injectable clock (`teneral-store/src/clock.rs`), a lapsed lease is removed and its event appears; a live lease is not; a lease in one network is swept while the same address in another network is left alone; and a sweep that finds nothing writes no event and reports no position. Do not add a grace period -- decision 10 explains why it is future work rather than this step. Commit subject: "Sweep lapsed leases on a daily timer." |
| 5g | high | opus | none | The lab, the captures and the rig. **Do not deploy anything until the management session confirms the back brief's gate has been answered yes** -- this step touches real infrastructure. Then: `cd` to the `homelab-deployments-lfs` checkout, `git pull` (the local `main` is 13 commits behind the merge that carries the lab), deploy `playbooks/linux-bridge-lab-debian-13.yml`, and leave the gateways in the root configuration (`lbr-lab-gateway show` to confirm). Read `docs/linux-bridge-lab.md` in that repository and `docs/differential-rig.md` here, especially *How a captured pair gets into the corpus lawfully*, which is six numbered steps and is not optional: the versions, the `tcpdump` command, the pcap kept outside this repository with its sha256 recorded, the frames as annotated hex, and the write-up. Capture dnsmasq first, then stop it and run teneral on the same bridges with the same ranges -- a `make release` binary copied in, run with `CAP_NET_BIND_SERVICE` and `CAP_NET_RAW` -- and capture the same exchanges. What the phase needs: a DISCOVER/OFFER and a REQUEST/ACK from each of the two overlapping networks, a renewal, a RELEASE, an INFORM, and a NAK. Four behaviours came out of 5c with no provenance row and have to be measured here rather than guessed: whether the reference implementation pads a reply to the BOOTP minimum of 300 bytes, which teneral does not do and a 274-byte reply was accepted by one client; what it does with `hlen` of zero, with a `chaddr` of all zeroes and with a plain BOOTP request carrying no message type, all three of which teneral refuses; and what it does with an option whose value will not decode, where teneral drops the option and answers anyway. Add the provenance rows the captures support, with their write-ups, and the cases that cite them. Then the rig: the `[network]` table in `case.toml`, the subject signature carrying it, field-wise comparison reporting every difference rather than the first, and the existing `dhcpv4-discover-offer-dhclient` case flipped from `pending` to `compared`. Correct `docs/differential-rig.md:192-203`, which promises byte comparison. Nothing in this step reads dnsmasq's source, and no measurement is taken anywhere but the lab. Report the lab's identifier, the versions, and every row id you added. Commit subject: "Compare teneral's DHCPv4 bytes with dnsmasq's." |
| 5h | medium | sonnet | none | Documentation and fold-back. This step writes no front-end code. `docs/binding-store.md` is not in this step: the lease time was written into it when 5a landed, because leaving a sentence saying a network is a name and nothing else would have left a false claim in the tree for six steps. Read it rather than rewriting it. (1) `docs/dhcpv4.md`: the front-end as a reference for somebody with no plan in hand -- the three layers, which messages are answered and what the others get, the option set with each option's source, the lease time and where it is configured, the namespace `root`, why it is not `/`, and the collision it accepts, the capability set and how to run it by hand, the sweeper and the stickiness it chose, the recorded-holder behaviour from decision 11 with its cause and consequence, and the limits the parser imposes -- `hlen` of zero, an all-zero hardware address, a reply or a non-client message type, a plain BOOTP request, more than four undecodable options, and an option whose value will not decode -- each with what it costs. (2) `ARCHITECTURE.md`: the front-end layer's first member exists, with what it serves and what it does not. This is a shape change, so it belongs there. The stale claim about protocol options was already corrected by this plan's commit, in the paragraph that lists what is deferred -- check it and do not correct it twice. (3) `AGENTS.md`: only conventions that changed -- a fourth crate, the namespace `root` for an interface in no namespace, and the rule that a wire behaviour without a provenance row is not shipped if it is not already there. Nothing else; every line there costs context on every task. (4) `docs/development.md`: the new crate in the layout, and that `make test` cannot open a socket below the IP layer and has one interface, so the datapath's reply path is proved in the lab. It *can* bind a privileged port -- the build container runs in a network namespace of its own where unprivileged ports start at zero -- and `--dhcp-port` exists because a test must not take a well-known one, not because it may not. (5) Fold back into the master plan: phase 5's row, and the *Future work* entries this phase closes or rewrites -- the sweep-interval entry (now chosen, with the grace period named as what would decouple the two), the client-identifier entry (declined, with where it is now documented), the differential-rig entry (the lab was redeployed, with what it produced), and new entries for the deferred option table, the DECLINE exclusion, and the size of the DHCPv4 decoder's dependency subtree -- fifty-four crates counting `dhcproto` itself, twenty-seven of them the DNS and Unicode stack that `hickory-proto` brings in for the domain-name options, which phase 6 wants anyway and which arrives a phase early rather than needlessly. Measure it again rather than copying the number: the plan first said fifty-seven and the tree says fifty-four. Write each in full rather than naming it. The view-bound entry was already rewritten by this plan's commit, which is where it was found to name the wrong phase; read it rather than rewriting it. Commit subject: "Document the DHCPv4 front-end." |

## Risks and mitigations

**A cross-network defect in the decision layer.** This is the
project's signature failure and this phase is the first place a
packet reaches it. Mitigation: every test in 5d runs two networks
holding the same address, and the lab runs both networks at once
through one process so the end-to-end case is measured rather than
reasoned about. The management session re-runs the two-network
tests itself and reads the diff for any lookup that does not name
a network. `cross_network_answers` -- the counter phase 4 wired --
must still be zero after the lab run, and the diagnostics are read
to confirm it rather than assumed.

**A panic in the parser.** One process serves every network, so a
malformed packet that panics is a remote denial of service against
all of them. Mitigation: the `hlen` guard before any accessor, the
malformed corpus in 5c, `unwrap_used = "deny"` inherited from the
workspace, and a mutation run proving the corpus kills the guard's
removal. The management session reproduces one mutant itself, on a
copy, under `timeout`.

**The datapath is the part the repository's tests cannot reach.**
Finding 3 is measured: no `CAP_NET_RAW`, and one interface.
Mitigation is structural rather than procedural -- decision 2 puts
every judgement above the socket, so what cannot be tested is
"the bytes left the right fd" -- plus the lab, which tests exactly
that with a real client. The risk that remains is a lab that
cannot be deployed, which is why it is a back-brief gate rather
than an assumption.

**The namespace string is hard to change later.** Decision 8 picks
`root` and an operator's attachments are keyed by it, so changing it
after a deployment exists means a migration of rows a human typed.
Mitigation: it is decided here rather than in phase 8, the
collision argument is the reason rather than taste, and phase 8's
own file inherits it as a given rather than a choice. This risk
materialised inside the phase rather than after it -- the first
choice was refused by the store and the second was made with the
first one's reasoning in hand -- which is the cheapest time for it
to have happened.

**dhcproto publishes a release mid-phase.** Then the guard may be
droppable and the floor in `Cargo.toml` is the question.
Mitigation: the guard is correct either way and is not removed in
this phase; 5c records the version it resolved, and the *Future
work* entry already says to watch for the release.

**The lab drifts from what was measured.** The lab is deployed
from one command and deliberately not kept, so a second
deployment is a second set of MACs, uptimes and versions.
Mitigation: 5g records the versions with every capture, as
`docs/differential-rig.md` requires, and a case whose write-up
does not name a command and its output is not a case.

**Scope creep into an option table.** The temptation in 5a and 5d
is to add "just the domain name" or "just the MTU", and each one
is a schema decision made by the wrong person. Mitigation:
decision 4 names the nine options and their sources, the deferred
table's shape is written up as future work, and a tenth option in
this phase's diff is something the management session rejects.

## Definition of done

Every item is checkable, and the management session runs it rather
than ticking it.

1. `make test` passes, and the count is recorded in the execution
   notes alongside phase 4's 229 so the change is visible.
2. `make lint` is clean with `-D warnings`, including the final
   `cargo check --frozen -p teneral-store`.
3. `make deny` reports advisories, bans, licences and sources ok
   with `exceptions` still `[]`, over a tree that now includes
   `dhcproto` and whatever it brings.
4. `pre-commit run --all-files` passes.
5. `scripts/check-store-trait.sh` passes, reports `update_network`
   as network-scoped and cursor-returning, and a copy of the trait
   with that method's `network` parameter removed still fails it.
6. `scripts/check-api-docs.sh` reports every operation documented
   and every documented operation present, and fails when
   `update_network` is removed from either side. Running this found
   that it did not: the script took "named anywhere on the page in
   backticks" for the schema-to-page direction, so deleting an
   operation's row from the table left it passing on a mention in a
   paragraph. Both directions now read the enumeration, which is
   what the script's own header already said the check was for.
7. `1.89` is still the only place the MSRV floor is written outside
   the lockfile, and if `dhcproto` raised it the raise is in the
   same commit with a comment naming what needed it.
8. The schema version is 9, the migrations are forward-only, and a
   store created by 0001-0007 opens and reports the default lease
   time without an operator writing one. This item said 8 until the
   phase ran: 0008 sweeps attribution into the event states phase 4
   left without it, which this phase found while checking 0009's own
   backfill, so the lease time is 0009 and there are two migrations
   rather than one.
9. Every operation in the schema is reachable from the debugging
   client, asserted by a test that names them rather than by
   counting subcommands by hand, and the lab in 5g was configured
   and inspected with that client and nothing else.
10. `teneral-dhcp` depends on `teneral-store` and not on
   `teneral-api`, checked by reading its `Cargo.toml` and by
   `cargo tree -p teneral-dhcp` showing no `teneral-api`.
11. A DHCPv4 datagram with `hlen` of 17 and one of 255 are each
    refused with a named error, and neither panics, in a test whose
    name says which input it carries.
12. Every input in decision 5's malformed list has a test, and a
    copy of the tree with the `hlen` guard removed fails a named
    one. The mutant, the test that killed it and the script's path
    are recorded.
13. Two networks holding the same address, asserted for every row
    of decision 6's table: DISCOVER, REQUEST in both states,
    DECLINE, RELEASE and INFORM.
14. A binding configured with the selector value an operator would
    type is matched by a packet carrying the attribute it was
    configured from, for the hardware address and for the client
    identifier, proved end to end rather than by asserting the
    renderer against itself.
15. The nine options of `dhcp-unasked-option-set` appear in an
    OFFER in the measured order, with T1 at half the lease and T2
    at seven-eighths, asserted against the bytes rather than
    against the builder's inputs.
16. An INFORM is answered with options, no `yiaddr` and no lease
    time, and no lease row is written.
17. A REQUEST naming another server's identifier produces no
    reply, and a REQUEST for an address another client holds
    produces a NAK.
18. A non-zero `giaddr` produces no reply and exactly one log
    line naming the relay.
19. An interface with no address, or with more than one, is
    refused with a log line naming the interface rather than
    answered from a guess.
20. The sweeper removes a lapsed lease and leaves a live one,
    writes no event when nothing expired, and sweeps one network
    without touching the same address in another.
21. The daemon refuses to start when `--dhcp-interface` names an
    interface no network has claimed, and the refusal names it.
22. The daemon's startup line names, per served interface, the
    network, the namespace, the derived address and prefix, and
    the lease time -- read from a real run, not from the source.
23. `teneral serve` with no `--dhcp-interface` behaves exactly as
    phase 4 shipped it, proved by phase 4's own daemon tests still
    passing unchanged.
24. The lab was deployed, teneral served both overlapping networks
    through one process, and a real client bound an address on
    each -- with the client's own output recorded, not teneral's.
25. `cross_network_answers` is zero in the diagnostics after the
    lab run, read from the running daemon.
26. Every new provenance row cites an RFC section or a named
    observed-behaviour test. Each **observed** row is cited by at
    least one case, and the rig fails a case citing a row that does
    not exist. An RFC-cited row names the tests that assert it
    instead, which is what the rows already in the file do and what
    this item said wrongly until a row arrived to show it.
27. `dhcpv4-discover-offer-dhclient` is `compared` rather than
    `pending`, the rig's subject registration and the case's
    status agree, and a deliberately wrong field in the subject
    makes the rig fail naming that field.
28. No document says the binding store holds protocol options, and
    `grep -rn 'protocol options' ARCHITECTURE.md docs/` returns
    only accurate sentences.
29. `docs/dhcpv4.md` exists and states the lease time's home, the
    namespace `root`, the capability set, the sweeper's interval with
    what it chose, and the recorded-holder behaviour.
30. The master plan's phase 5 row and every *Future work* entry
    this phase closes or rewrites are updated, and each new
    deferral is written in full rather than named.
31. Nothing this phase started is still running: no teneral
    process on this host or in the lab, no container, no `cargo`
    on the host, and the lab either torn down or reported as
    deliberately left up.
## Back brief

My understanding of this phase, and the points that need an answer
before a step runs.

The phase builds a DHCPv4 front-end in a new crate, in three
layers, and the layers exist in that shape because the
repository's own test target cannot open an `AF_PACKET` socket and
has one interface -- so everything that decides anything lives
above the socket, where it can be tested, and the lab proves the
rest. Seven steps, strictly sequential, one commit each, reviewed
in this session. The store gains one column -- the lease time --
and every other option in the measured set is derived from the
interface a request arrived on, as dnsmasq was observed to do.

Four things want your answer.

**The lab.** Step 5g deploys
`playbooks/linux-bridge-lab-debian-13.yml` to real infrastructure,
runs teneral in it with `CAP_NET_BIND_SERVICE` and `CAP_NET_RAW`,
and captures the exchanges. Without it the phase ships a front-end
whose cases are still `pending`, which the plan's own success
criteria say is incomplete. I will not deploy anything until you
say so. The alternative, if now is the wrong time, is to land 5a
to 5f and 5h and leave 5g as a short follow-on phase.

**The lease time on the network rather than on the range**
(decision 3). It is where I would put it, and the argument for the
range is good enough that you should see it before 5a runs: that
is where dnsmasq's lives, and the measured 12h came from a
`dhcp-range`. My reasons against are that teneral has a real
network object where dnsmasq has none, and that a lease time on
the range has no answer for a reserved address outside every
range.

**DECLINE** (decision 6). This phase revokes the lease, logs the
event, and may offer the same address again -- which loops if the
conflict is permanent. The complete answer is an exclusion state
in the store with a retention policy of its own. I think the cheap
answer plus a written-up deferral is right for this phase; the
alternative is to add the exclusion here and accept a second
schema change.

**The namespace `/`** (decision 8). An operator will type it for
the rest of the project's life, and phase 8 inherits it. The
alternatives are an empty string, which looks like a forgotten
field, and `root`, which `ip netns` can also create.

## Execution notes

Written as the phase runs. The date, with its timezone, on each
entry.

**2026-11-26, AEDT. The back brief.** All four points were put to
the operator before any step ran and all four came back as the plan
had them: the lab is deployed in this phase rather than deferred,
the lease time goes on the network rather than on the address range,
DECLINE gets the revoke-and-log answer with the exclusion written up
as future work, and the namespace for an interface in no namespace
is `/`. Nothing in the plan changed because of the answers, so the
decisions stood as written rather than as amended -- but the
namespace did not survive contact with the store, and the note on
step 5d below records what replaced it and why the operator chose
again rather than being told.

**2026-11-26, AEDT. A step was added before any step ran, because
the plan was wrong.** Checking 5a's brief against the tree -- which
is done to each brief before it is sent, since an agent will build
on a false premise and hand it back as a finding -- found that the
claim it rested on was mine and false. The brief said the debugging
client would gain a subcommand "following the shape of the
address-range one". There is no such shape: `teneral/src/cli.rs`
offers `get`, `explain`, `diagnostics`, `mint-token`, `mint-epoch`
and `watch`, and no write of any kind. Phase 4 shipped a server
serving every write in the schema and a client that can reach none
of them.

That is not a cosmetic miss. Step 5g deploys a lab and configures
two overlapping networks over the control socket, and nothing in
this repository could have done it -- the phase would have arrived
at its most important step needing a throwaway program nobody had
planned to write. So the client's writes became step 5b, before
anything that depends on being able to configure a daemon, and the
rest of the table shifted by one letter. The definition of done
gained the item that makes it checkable: every write the server
serves is reachable from the client, asserted by a test rather than
counted by hand, and the lab is configured with that client and
nothing else.

**2026-11-26, AEDT. Step 5a: the lease time, and two migrations
rather than one.** Tests 229 to 234. The step landed the column, the
trait's `update_network`, the operation, the documentation and four
store tests -- the load-bearing one holding a single address in both
networks for one client and asserting each lease runs for its own
network's term, which a lease time kept anywhere but on the network
would collapse into one expiry.

Three things came out of the review rather than the step.

The migration brought the network row to the new shape and left the
event log in the old one. `NetworkState` is `deny_unknown_fields`
with no default and `of_event` refuses a state that does not parse,
deliberately, so every network event already in the log would have
stopped parsing and a consumer resuming a watch across the upgrade
would have been stopped until those events aged out. Backfilling is
honest here only because the column's default is retroactive: the
network did hold twelve hours at that position.

Checking that backfill's own assumption found the same defect one
phase back. Phase 4 taught every event writer to put `written_by`
into the state it composes, and its migration backfilled the
sentinel into all five resource tables and into `event_log.actor`
while leaving `event_log.state` alone -- so a database made in the
three days schema 5 existed holds five kinds of event that no
current build can read. It is unreachable in practice, nothing being
deployed, and it is the same defect as the one being fixed, so it
was swept rather than written up: `0008_event_state_attribution.sql`
closes it for every kind with the word the actor backfill chose, and
`0009_lease_time.sql` is this step's own. The schema version is
therefore 9 and not 8, and the plan's row is corrected at its
source. A test migrates a schema-5 database carrying a network
event, an attachment event and a deletion, and asserts the exact
state text of each after both migrations have chained.

The step's signature came from the plan and the plan was wrong.
`update_address_range` takes the expected position before the new
values; the row specified `update_network` with them the other way
round, so two adjacent compare-and-set methods would have read
differently. Reordered, and corrected in the row.

Three things the step did beyond its brief were kept, each on its
own argument rather than because it was offered: the upper bound at
`u32::MAX` with a validator that names the value and leaves the
CHECK as the backstop, since a longer lease is one an operator could
set and never see take effect; `create_network` writing the row
before its event, so the default lives only in the migration rather
than acquiring a second home in Rust; and the gate scripts checked
against mutants on copies.

`docs/binding-store.md` was corrected by the management session in
this step rather than left to 5h, because it said a network is a
name and nothing else, and a false sentence should not sit in the
tree for six steps. One claim in the step's report did not survive
checking: the schema table in that file does not omit the new
column, because it lists no columns for any table -- its subject is
each resource's key.

**A flake in phase 4's suite, reproduced and left standing.**
`a_stale_socket_file_does_not_stop_a_daemon_starting` fails on its
own precondition: it binds a `UnixListener`, drops it, and asserts
that connecting to the path now fails, and sometimes the connection
succeeds -- `connected to a socket nobody listens on: Ok(UnixStream
{ ..., peer: "/tmp/teneral-daemon-stale-18-0/teneral.sock" })` at
`teneral/tests/daemon.rs:290`. It happened twice in twenty runs of
the whole suite here, so it is a real intermittent failure and not
one bad run.

One mechanism is eliminated rather than argued about: closing a
listening Unix socket and immediately connecting to its path was run
twenty thousand times on this host, with a fresh path each time, and
every single connect was refused. So the kernel does not leave a
closed listener reachable, and something was genuinely holding that
path. What can hold it is a process that inherited the listening
descriptor, which would mean a child spawned by one of the five
tests running in parallel with this one, in the few microseconds
between the bind and the drop -- except that the standard library
creates the socket with `SOCK_CLOEXEC`, which should close it at
`exec`. The two facts do not yet fit together, and nothing is
committed that claims they do.

It is recorded rather than fixed because a fix chosen before the
mechanism is known would hide it: the obvious ones -- retry the
probe, or make the stale file some other way -- would both make the
test pass without anybody learning why it did not. What the failure
does *not* show is a defect in the daemon: the two tests that assert
the behaviour this one sets up for -- that a live daemon stops a
second starting on its socket, and that a stale file is replaced
rather than inherited -- passed in every run, including the failing
one. The next person to touch `teneral/tests/daemon.rs` should start
by dumping `/proc/net/unix` for that path at the moment the probe
succeeds, which names the holder and its state and is the one piece
of evidence nobody has.

**2026-11-26, AEDT. Step 5b: the client reaches the whole schema,
not just the writes.** Tests 234 to 239. The step was widened before
it ran, by the same check that created it: the client reached ten of
the schema's thirty-four operations, not twenty-two, because every
listing, every lookup, the digest and two token operations were
unreachable as well as every write. Closing only the writes would
have left the lab deployed in 5g with no way to see what had been
configured, so all twenty-four were closed at once and the
definition of done now asserts the whole schema rather than half of
it.

The gate is a test that parses `Request`'s oneof out of the schema,
asserts it holds thirty-four operations, and for each one parses a
command line with clap, builds the request, and compares what was
built against the schema's own name for it -- through an exhaustive
match with no default arm, so a new operation stops the crate
compiling as well as failing the test. Three mutants on copies
confirmed it bites: a thirty-fifth operation, a row deleted from the
reachability table, and a listing wired to the wrong request.

Two of the step's judgements were kept against what this plan said.
The writes take their arguments positionally, where decision 8's
example wrote them as flags: `get attachment` and `explain` already
take a namespace and an interface positionally, and a write that
spelled the same key differently from the read beside it is the
inconsistency a reader trips on. A transposition is caught quickly
anyway, because an attachment no real interface matches makes the
daemon refuse to start on it. Decision 8's example is corrected to
the form that now exists. And `run()` was split into a function that
says what was asked for and a dispatch that does it, which is a
design change made to fit the work and kept deliberately: it is what
lets the gate test the whole mapping with no daemon and no socket.

`docs/control-api.md` enumerated the six subcommands the client used
to have, which this falsified. The step reported it rather than
editing it, correctly, since that file documents the API and not the
client; the management session replaced the enumeration with the
property that is now true and testable.

**2026-11-26, AEDT. Step 5c: the wire layer, and a defect in the
decoder's contract.** Tests 239 to 284, of which 45 are the new
crate's. `teneral-dhcp` holds `wire` only: `parse` for bytes in,
`Answer` and `encode` for bytes out, no store, no socket and no
clock, which is what makes every question about a malformed packet
answerable in a unit test. The encoder is this project's own rather
than the dependency's, because that crate holds options in a map
keyed by code and so emits them in code order, while the order an
answer's options are written in is a measured property of the bytes;
the test walks the encoded frame and asserts the nine codes of
`dhcp-unasked-option-set` in the order they were observed.

The dependency costs fifty-seven crates and the lockfile went from
eighty-eight packages to a hundred and forty-five. `hickory-proto`
is non-optional in the decoder, for the domain-name options, and
brings the bulk -- including a URL parser, the ICU normaliser and an
Android binding that never builds on Linux. `make deny` passes with
`exceptions` still empty, so no licence decision was dodged, and
phase 6 wants that library anyway, so it arrives a phase early
rather than needlessly. It is recorded in *Future work* so that the
alternative -- a hand-written codec with no dependency -- stays a
decision somebody can take rather than one that was never noticed.

Three rounds of review, and each came from checking the round before
it rather than from the step being wrong.

The step found that the decoder stops at the first option it cannot
read and returns **success**, so a packet with one bad option parses
cleanly and short. What it loses is whatever followed, which can be
the requested address or the client's identifier -- the two fields
that decide which address a client gets. Nothing downstream could
have known. The fix is to walk the option records before decoding,
and it was the step's own find rather than the plan's.

The first review changed what that fix does. It refused the whole
datagram whenever a framed option was missing from the decoded set,
which would have denied service to any client sending a name the
decoder would not take -- a Windows machine with a non-ASCII name
does exactly that. Two facts were then checked rather than assumed:
an option code the decoder has no type for is surfaced as an unknown
rather than dropped, so the strict check was never firing on vendor
or private-use codes; and a declined value really did take the
records after it, so filtering the result could not have recovered
them. Hence the split now in decision 5, and a re-decode rather than
a filter, which keeps the decoder's concatenation rule for a value
split across several records.

The second review bounded that re-decode. A pass decodes the whole
datagram, and the passes are bounded only by the number of codes the
decoder has a type for, so a sender could buy some sixty full
decodes with one packet and spend them on the single thread that
answers every network. Four drops, then the datagram is refused.
Each of the three rounds is a case of the same thing: the cost of a
packet must be a constant the receiver chose.

**2026-11-26, AEDT. A decision was missing, found before 5d ran.**
Checking 5d's brief against the store turned up the question of what
form a packet's attributes take as selector values -- and the store's
own comment on `Selector::new` saying that the front-end owns the
choice, that the configuring side and the matching side have to agree
on it, and that the store is not in a position to check that they do.
Only the hardware address is normalised; everything else is whatever
string the caller passed.

That is the project's signature failure in its quietest form. Get the
form wrong and an operator configures a reservation, every test in
this repository passes, and no binding ever matches a real client.
The plan had nothing to say about it, so decision 15 now does, with
the client identifier taking the form the reference implementation's
own lease file was measured to use -- colon-separated hex -- so that
an operator moving across can paste what they already have. The
definition of done gained the only test that can see a divergence:
one that renders a value from a packet, configures a binding with the
value an operator would type, and matches the packet against it. A
test of the renderer alone would be asserting it against itself.

**2026-11-27, AEDT. Step 5d: the decision layer, and the namespace
that could not be.** Tests 284 to 313, of which 28 are the decision
layer's and 23 use two networks holding the same address. The five
that do not are the cases where no second network can leak: an
unclaimed interface, a hardware address that is not six octets, a
vendor class that is not text, an option the parser dropped, and the
front-end's own attribution on a lease it wrote.

**Decision 8 was unimplementable and the operator chose again.** The
step tried to attach an interface under `/` and the store refused
it. `attach_interface` runs `validate_name_part` on both halves, and
that function refuses the separator because an attachment's identity
reaches an event as the two halves joined by it -- with a comment
making precisely the argument this plan made for choosing `/`, and
drawing the opposite conclusion from it. Finding 7's premise was
false and decision 8 rested on it. The step did the right thing: it
documented the contradiction, used a declared placeholder, and left
the choice alone rather than quietly picking one. The management
session verified the refusal, put the three live options to the
operator -- a word that can collide loudly, a dot that cannot
collide but that URL normalisation would eat, or changing an
identity format every event already carries -- and he chose `root`.
Decision 8, finding 7, the risk entry, the done item, the help text
5b had already shipped and both doc comments in the new crate are
corrected. The spelling reaches one constant in this layer, which is
what the step's placeholder comment was written to make true.

**A second RFC-cited row, and the first that arrived the way the
provenance record expected.** The step's first version treated a
REQUEST with no server identifier as a NAK whenever the store had no
live lease, with "it lapsed" and "there never was one here" as one
case. RFC 2131 section 4.3.2 divides them: where the server has no
record of the client it MUST remain silent, for the stated reason
that two non-communicating servers have to coexist on one wire, and
it is only where there *is* a record that disagrees that a NAK is
right. The cost of getting that wrong is not pedantic -- two DHCP
servers on one bridge is what a migration looks like, and a teneral
that NAKed every other server's clients when they rebooted would
break the network it was added to. The branch now turns on the
fields the section names, which the parsed request already carried,
and `dhcp-init-reboot-silence-without-a-record` is the row, with
what counts as a record and the decision to emit no warning marked
as teneral's own.

That row also corrected a definition-of-done item of this plan's:
it asked that every new provenance row be cited by a differential
case, which is true of an observed row and not of an RFC-cited one.
The two rows already in the file name the tests that assert them
instead, and the item now says so.

**An extra kept on its own argument, verified rather than taken.**
The step added a check of a client's named address against the
network's ranges and bindings before acquiring a lease, saying the
store would otherwise hand out an address another client's binding
reserves. That is true: `acquire_lease` checks that the network
exists and that nobody holds a live lease, and nothing else. It is
also right that the store is permissive there -- a consumer that
chooses its own addresses may legitimately name one outside every
range -- and a client on the wire must not be able to talk its way
past a reservation, so the check belongs in the front-end. Also
kept: the request gaining the user and vendor class options that
decision 15 specifies and 5c did not carry, a dropped attribute
being noted rather than fatal, and a release or a decline asking
what the client holds rather than re-deriving its holder.

**2026-11-27, AEDT. Step 5e, and a premise of mine the step was
right to measure.** The brief told the step three things about the
build container: that binding UDP 67 fails with `EACCES`, that
creating an `AF_PACKET` socket fails with `EPERM`, and that
`SO_BINDTODEVICE` wants `CAP_NET_RAW` so the egress should be named
per message instead. Only the second was true. Docker gives each run
a network namespace of its own and sets
`net.ipv4.ip_unprivileged_port_start` to zero in it, so port 67
binds there as uid 1000; the 1024 in the brief was the host's, which
is where that measurement had actually been taken. And
`SO_BINDTODEVICE` needs no capability on this kernel, where it
historically needed one. The step measured all three and said so,
which is what a brief's premises are for. Finding 3, decision 2's
closing paragraph, the risk entry and the back brief are corrected
here, and so are two module doc comments in the crate that had
already shipped the claim. Nothing in the design moves: `--dhcp-port`
exists because a test must not take a well-known port, which is true
however the kernel feels about it, and the capability set is
unchanged because `AF_PACKET` is still the one hard barrier and the
protocol's port is still privileged where this will be deployed.

**2026-11-27, AEDT. The order a reply is addressed in is a
specified algorithm, and this session reordered two of its
branches.** The step reported two wire-visible behaviours that had
arrived without a row in `docs/provenance.md` -- that a refusal is
broadcast even to a client holding an address, and that an answer to
a client that holds one is unicast to `ciaddr` -- and did not invent
the rows, which was right. Reading the sections to write them found
that RFC 2131 section 4.1 specifies the whole order rather than
those two parts of it, and that the step's code departed from it in
one case: it treated the broadcast flag and the refusal as one
branch ahead of the address the client holds, where section 4.1
reads the condition off `ciaddr` and consults the flag only where
`ciaddr` is zero. So a client that holds an address and asks for a
broadcast is answered at the address it holds, and the branches are
now in the section's order with a test on the case the two orders
disagree about. The flag's own reason does not arise for such a
client: it answers address resolution for the address it has. One
row covers the whole order, `dhcp-reply-addressing-order`, with the
refusal's MUST cited to section 4.3.2 and the answer to a request
for configuration alone to section 4.3.5, and with the one part that
no section covers -- an answer carrying no address at all is
broadcast -- marked as teneral's own.

**2026-11-27, AEDT. What 5e changed in the layer above it, kept.**
`Reply::new` now takes the arrival, the request, the answer and the
datagram rather than six fields, and the reply carries three more:
which answer it is, the server's own address, and the address the
client already holds. The last is the one that mattered -- 5d's seam
could not tell a client being offered an address from one that
already holds one, which is the fact the whole delivery order turns
on -- and the shape is better than what it replaced, because a reply
can no longer be assembled from an answer and an interface that
answer was not derived from. Two startup refusals were added beyond
the row and kept on decision 14's own argument, both landing before
any socket opens: an interface named twice, which the kernel would
otherwise report as a port already in use naming neither the flag
nor the interface, and an interface whose address an answer cannot
be derived from. Each datagram's arrival index is resolved to a name
through the socket and checked against the interface served, so the
network a request is answered from follows the kernel's account of
where it came from rather than a string an operator typed. Also
corrected in review: the protocol number an Ethernet frame declares
had come to be written in two places, and is now written once.

**2026-11-27, AEDT. Step 5f, and a crash this phase inherited.** The
sweeper arrived as the row asked for it, and the review found that
arming its timer was `Instant::now()` plus the interval, which panics
for an interval the monotonic clock cannot name an instant that far
ahead of. `--sweep-interval 18446744073709551615` parses as a `u64`
and passes the flag's own `range(1..)`, so the panic was reachable
from the command line -- and because the timer is armed inside a
spawned task, the panic did not stop the daemon. It came up, logged a
backtrace, and ran with no sweeper. Reproduced before fixing, and the
sweep of that class found the same arithmetic in
`teneral-api/src/watch.rs`, where a stream's heartbeat timer is armed
the same way from `--heartbeat-seconds`: a pre-existing defect that
phase 4 shipped, reachable the same way, and costing a consumer's
stream rather than the sweeper. Both are now one refusal in
`serve.rs`, beside the one that already turned a flag into a `usize`,
under the comment there that says everything refusable is refused
before anything is created. No ceiling is invented: the check is
whether the clock can name the instant, which is exactly what the
timer needs. `sweep::start` takes the interval rather than the
arguments, so the validated value is the only one it can be given,
and a daemon test asserts the refusal for both flags and that neither
a socket nor a store is left behind.

**2026-11-27, AEDT. A test that had quietly stopped covering what it
was written for.** `help_names_every_flag_with_its_default` checks
`teneral serve --help` against "the table the daemon is specified by"
in `docs/control-api.md`, and it does so from a list of flags written
by hand -- so the three flags added in this phase were not covered by
it and it had gone on passing. `--dhcp-interface` and `--dhcp-port`
were also missing from the table itself, which is 5e's gap and is
fixed here rather than left for the documentation step, because the
table is what that test is checking against and a stale table makes
the test agree with the wrong thing. The flag with no default is
asserted separately, there being no default to look for.

**2026-11-27, AEDT. What 5f decided that its row left open, kept.**
The sweeper starts after every refusal the daemon makes, so a daemon
that is not going to come up has not started a timer holding a handle
on the store, and it is stopped before the DHCP sockets and before
the wait for the store to fall idle, because it is the one that
writes. A failure sweeping one network is logged and the loop carries
on -- a network deleted between the listing and the write is exactly
that case -- while a failure to list the networks ends that sweep and
the next tick retries. The line a sweep logs is a method on the
result rather than a log call inside the sweep, which is what lets
"a sweep that removed nothing logs nothing at all" be asserted
directly instead of inferred. Missed ticks are dropped rather than
run back to back, because whatever a missed sweep would have removed
is still there for the next one. The step reported, correctly, that
it added a third hand-rolled copy of the temporary directory and the
test clock, and that hoisting them means a test-support feature on
the store adopted by two crates; that is written up as future work
rather than done in a step whose subject is a timer. It also decided
deliberately that the sweeper owes no provenance row, which is right:
it deletes rows and appends events to a history whose wire is
teneral's own.

**2026-11-27, AEDT. Step 5g: the lab was deployed, as the operator
ruled in the back brief.** Deployment `lbrlab-1795723089`, from the
same playbook as the lab the earlier rows were taken in, after
merging the thirteen commits the plan said were outstanding -- which
they were, exactly thirteen, and the playbook and the write-up were
both only on the remote. teneral served both overlapping
`10.0.0.0/24` networks from one process as uid 1000 holding
`cap_net_bind_service,cap_net_raw` and nothing else, which is the
capability set this phase said it would need, with
`cross_network_answers` at zero afterwards. The lab is left running
on purpose: a re-measurement is two commands while it is up and
impossible once it is gone, and the decision below is exactly the
kind that wants one. Twelve observed rows and thirteen cases came out
of it; the provenance record scopes the two deployments separately,
because the second ran a later kernel and a later dnsmasq package and
a row that said otherwise would be claiming more than was measured.

**2026-11-27, AEDT. The replacement was emitting replies a
conforming receiver is told to discard.** The step measured that the
reference implementation pads every reply to the BOOTP minimum of
three hundred octets and that teneral padded nothing -- 292, 274 and
250 octets -- and it recorded the difference as a deliberate
divergence in four cases and asked the management session to decide,
which was right: decision 5's precedent is that the lab measures and
the design decides. The answer came from reading RFC 1542 section 2.1
rather than from comparing tastes. That section lists the consistency
checks a BOOTP entity SHOULD perform on arrival, the first being that
the IP and UDP lengths are large enough to hold the minimal
three-hundred-octet BOOTP header, and then says that a message
failing a check MUST be silently discarded. So this was not a
divergence from anybody's preference: it was teneral sending
datagrams a conforming client or relay agent is instructed to drop
without a word, which is the failure this whole project is shaped
around. Both clients in the lab accepted the short replies, and that
is luck rather than vindication.

Recording it as deliberate would have been worse than not recording
it at all, because a divergence that says it was chosen is one nobody
revisits. The reply is now padded, with the minimum as a floor and
never a limit -- the same section's NOTE requires a receiver to
accept a longer message -- and the padding goes after the end option
as zero octets, which is the pad option's own code, so a receiver
that keeps walking past the terminator reads padding rather than
nonsense. `dhcp-reply-meets-the-bootp-length-check` is the row, cited
to the section, and it names the tests that assert it as the other
RFC-cited rows do. The observed row stays beside it unchanged: one
says what was seen and the other what is required, and they are
different claims. The five cases whose lengths now agree moved
`payload-length` from the comparison's `diverges` list to its
`fields` list, and those five were exactly the five the rig failed
the moment the code changed, which is that list doing its job. Three
cases keep a length divergence, and it is now the twenty-two octets
of host name the reference adds and nothing else.

**2026-11-27, AEDT. The citations would not have survived the week.**
The six captures existed only in this session's scratchpad, which is
cleaned on a schedule, while each case cited its pcap by a filename
that existed nowhere on disk and the lab write-up recorded the
digests under a third set of names -- the working directories they
were taken in. Phase 1 had already set the convention, a directory
under `/srv/nobackups` holding the capture beside its digest, and all
six are now retained there under exactly the names the cases cite,
with every digest verified against the write-up. The write-up's
digest block names the retained files and says what the `run-*` names
were. This is the one part of the lawful-capture process that cannot
be checked by reading the diff, because what it asserts is that a
file will still be there later.

**2026-11-27, AEDT. What else the review changed in 5g, and the one
measurement that bears on a decision already written down.** The step
made `Delivery` and the function that chooses one public so that the
rig can compare where a reply went and not only what it said; that is
kept, because the delivery order is the one part of the send path
that is judgement rather than a system call, and nothing in this
repository may open the socket that would otherwise answer the
question. Two more doc comments still carried the disproved claim
that the test target cannot bind a privileged port, in the decision
layer's seam and in the rig's own document, and both now name the
barrier that is real. The step also surfaced a refusal in the store
that read "there is no lease a live lease on ...", the kind being
stated once by the error's own rendering and again by the identifier
handed to it; that is fixed, the other six constructions of the same
error were checked and were correct, and the test now asserts the
whole rendered sentence rather than the fragment that let it through.

Last, an unasked-for measurement worth keeping: a renewal ACK from
the reference carries T1 and T2 *below* a half and seven-eighths of
the lease, and lower again on each renewal -- 60/105, then 56/101,
then 53/98, with the lease option reading the same number every time.
The mechanism is not established and establishing it would mean
reading dnsmasq, so three measurements are recorded and nothing is
claimed about why. Decision 13 is still true of teneral and of a
first grant, and `dhcp-unasked-option-set` is now scoped to a first
grant so that it does not claim the fractions hold on a renewal.

**2026-11-27, AEDT. Step 5h, and a phase that tried to close
itself.** The step set this phase's row in the master plan to
`Complete` with an open-ended `Merged` range, offering to revert it,
and said phase 3's row was the precedent. It is not: phase 3's row
carries a closed range. The row is back to `In progress` and the
paragraph above the table now says why it stays that way in this
phase's own pull request -- the `Merged` cell holds the commit that
landed the phase, which nothing inside the phase can name, so the
next phase's first commit is what closes it. `docs/plans/index.md`
holds one row per master plan rather than per phase and already read
`In progress`, so the two records agree without being touched, which
is the condition that matters.

**2026-11-27, AEDT. A number of mine the step was right to refuse to
reproduce.** The brief for 5h told it that twenty-seven of the DHCPv4
decoder's fifty-four dependency crates were the DNS and Unicode
stack. It could not reproduce the twenty-seven, said so, wrote "most
of them" and left the figure out -- which is exactly right, and it is
the third time this phase that a step has declined to repeat
something the management session asserted. The figure was the count
of crate names matching a pattern this session had written, not of
crates reachable through the library. Measured properly, by taking
`hickory-proto`'s own subtree and subtracting: **fifty-two of the
fifty-four are in it, and the only two that are not are `dhcproto`
and its macro crate.** "Most" understates it, so both the master
plan's entry and `docs/dhcpv4.md` now carry the real share, and the
entry records whose mistake the old number was. The plan's original
fifty-seven was also stale and had already been corrected in the row
before the step ran.

**2026-11-27, AEDT. What 5h did beyond its row, kept.** A fourth copy
of the disproved port claim, in the decision layer's module doc, and
a line in `README.md` saying nothing yet speaks DHCP -- both found by
the step rather than by this session, which had already swept for the
first of them twice. Two future work entries the execution notes
above promised and nobody had written: the three hand-rolled copies
of the test directory and clock helpers, and the daemon-test flake
recorded earlier in the phase. An entry promised in a note and never
filed is the failure mode that note was written to prevent, so
finding both is worth more than the lines they cost. `AGENTS.md`
gained three lines and no more: a fourth crate, the namespace, and
the rule that a wire behaviour ships with a provenance row -- which
the step phrased better than the row did, extending it to a length,
an ordering or an addressing rule that merely seems incidental. Those
are precisely the three things this phase shipped without a row and
had to go back for.

**2026-11-27, AEDT. The definition of done, run.** Twenty-nine items
passed as written, one passed after being corrected, and one failed
and was fixed. The test count finishes at **355**, against phase 4's
229: 229 to 234 for the lease time, to 313 for the wire and the
decision layer, to 347 for the datapath and the daemon, to 352 for
the sweeper, and to 355 for the padding the lab found missing. The
rig runs thirteen cases against forty-three provenance rows and the
daemon suite is nine, of which phase 4's six are untouched -- the
item asking that `teneral serve` with no `--dhcp-interface` behave
as phase 4 shipped it is checked by their diff being additions only,
a hundred and twenty insertions and no deletions.

The item that failed was item 6, and it failed in the gate rather
than in the tree: deleting `update_network`'s row from the control
API page left `scripts/check-api-docs.sh` reporting "every operation
is documented" while its own output said the page enumerated
thirty-three against the schema's thirty-four. The script's header
describes the check as being for the section where the capability and
the scope rule live, and its implementation accepted the name
appearing anywhere on the page in backticks -- so a paragraph
mentioning an operation satisfied it. Both directions now read the
enumeration, and the gate was re-run against the row deleted from
each side in turn: it fails each way, naming the operation. The
leniency had no other purpose, every operation being in the table.

Two mutants were reproduced by this session rather than taken on
report, each on a copy of the tree and under `timeout`. Removing the
`hlen` guard from the wire layer is killed by
`a_hardware_length_past_the_end_of_the_field_is_refused` and
`a_hardware_length_past_the_end_of_the_field_is_refused_whatever_follows_it`,
107 of 109 tests still passing, which is the shape a good mutant has.
Changing the renewal time from half the lease to a third fails five
differential cases, each naming the field and both values -- `option-58
differs and the case asserts that it matches: the capture carries
00:00:54:60 and teneral produced 00:00:38:40` -- which is the item
asking that a deliberately wrong field make the rig fail naming that
field.

One item could not be checked the way it was written and is reported
as such rather than ticked: item 9's clause that the lab was
configured and inspected with the debugging client "and nothing
else". The client did the configuring, and the lab's own helper
script names the store file in a variable, so the claim is right in
substance; but a grep of a script is not a proof of what ran, and
the only honest statement is that nothing in the recorded output
shows another tool writing to the store.

**2026-11-27 (AEDT), the first review round, and the first CI this
repository has ever run.** `github.com/shakenfist/teneral` was created
during the close-out, so `develop` and the phase branch were pushed
and pull request #1 opened against `develop`. All four workflows
passed on the first attempt. The automated reviewer raised 12 items:
2 `fix`, 1 `document`, 8 `consider` and 1 `none`, plus one CodeQL
alert.

Both `fix` items were defects this phase introduced, and both
reproduced in the tree before anything was changed.

The first is the one that matters. In `renewing`, the only path to an
ACK was a successful `renew_lease`, so a client naming the address its
*own* reservation holds got `NotFound`, and then `has_a_record_of`
found the binding and turned that into a NAK. That contradicts the row
this phase wrote: `dhcp-init-reboot-silence-without-a-record` says
the division is between having no record of a client and one that
disagrees with it, and here the record agreed. The fix honours the
reservation, and the acquisition is now one method shared with the
selecting path rather than a second copy of it. Two tests: a
reservation the client has never held, and one whose lease lapsed
under it. The second test also pins which refusal a lapsed lease
produces, because the new branch is reached on `NotFound` and a
lapsed lease reporting a conflict would walk straight past it. A third
test was written and then deleted: it tried to reserve an address
another client was holding, which the store refuses outright, so the
conflict arm is not reachable from this path at all.

The second is that `answer_on` returned on any error from `receive`,
which dropped one interface's task and left the daemon running --
holding port 67 on a link it no longer answered on, which is the exact
state this module's startup refusals exist to prevent. Three of
`receive`'s four error returns are conditions the datapath synthesises
about one datagram, so the comment claiming only an unreadable socket
reached there was wrong. `ReceiveFailure` now divides a per-datagram
condition from a socket fault, read from whether the kernel gave an
errno at all, with `EINTR` as the one errno that says nothing about
the socket; a per-datagram failure costs its datagram and logs, and a
socket fault stops the whole daemon through a channel the main loop
selects on beside its signals. A bound of 64 consecutive failures
catches anything the classification did not foresee rather than
spinning on it.

Three `consider` items were taken because each was a defect in code
this phase added and small: a lease of one second produced renewal and
rebinding times of zero, which the store's validator now refuses with
the reason; a sweep logged every address it removed on one line, which
on a lapsed pool is a line of tens of kilobytes in a log documented as
one line per message, so it now names ten and counts the rest; and
`ip-source` was missing from the rig's list of assertable field names
although four cases assert it.

Five were declined and are in the master plan's *Future work*: the
offer-term change behind item 3, which needs a measurement first and
whose exposure is now documented instead; the per-datagram
`getifaddrs` enumeration; the eagerly-opened packet socket that keeps
an end-to-end DHCP test out of CI; the NAK for a renewal that also
names a server identifier; and the single unbacked-up copy of the
retained captures.

The CodeQL alert is on `interface.rs:271`, the dereference in the
`getifaddrs` walk, and nothing was changed for it. The pointer is
either the head of a list a successful `getifaddrs` wrote or a
previous entry's `ifa_next`, both valid while the owning value lives,
and the null check precedes the dereference. A list walk through FFI
cannot be written in a way that proves this to the query.
