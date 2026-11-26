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

**3. `make test` cannot bind port 67, and this is measured rather
than assumed.** The Makefile runs the build image as the invoking
uid (`Makefile:57`, `-u $(UID):$(GID)`) with no `--cap-add`, under
`--network none`. `net.ipv4.ip_unprivileged_port_start` is 1024,
and a `bind` of UDP 67 as uid 1000 on this host returns `EACCES`
(errno 13), which was run rather than reasoned about. `AF_PACKET`
needs `CAP_NET_RAW`, which the container also lacks. So neither
half of the datapath can be exercised by the repository's own test
target. This is a design input, not an obstacle: it is why
decision 2 puts everything that decides anything above the socket,
and why the socket layer is as thin as it can be made.

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
no namespace.** An attachment's identity is `(namespace, ifname)`,
the namespace is a required string everywhere it appears
(`teneral/src/cli.rs:126,203`), nothing validates it and nothing
defaults it. A datapath with no namespaces has to pass something,
and whatever it passes becomes the name an operator must type to
attach an interface -- and the name phase 8 has to agree with.
Decision 8 picks it, and it is the decision in this plan with the
longest reach for its size.

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

This is the shape finding 3 forces: `make test` cannot bind port
67 and cannot open an `AF_PACKET` socket, so anything decided
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
`chaddr` of all zeroes -- asserting that each is refused and none
panics. A panic in this layer is a remote denial of service
against every network this process serves, which is why
`AGENTS.md` makes that lint `deny` rather than `warn`.

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

### 8. The namespace an interface with no namespace is attached under is `/`

An attachment is identified by `(namespace, ifname)`; the
namespace is required, opaque and unvalidated (finding 7). This
phase has to pass something, and the string it picks is the one an
operator types for the rest of the project's life.

It is `/`. Not an empty string, which is indistinguishable from a
field somebody forgot to fill in and which the CLI would have to
be taught to accept. Not `root`, because `ip netns add root`
succeeds, so a deployment with a namespace of that name would have
two different things with one identity -- and the only defence
would be validation in the store, which is a schema rule invented
to protect a naming choice. No name `ip netns` can create contains
a `/`, because those names are filenames under `/var/run/netns`,
so `/` cannot collide with a real namespace and needs no
validation anywhere.

The cost is that it reads like a typo in `teneral attach-interface
--namespace / --ifname br-n1`. That is paid in the help text, in
`docs/`, and in the line the daemon logs at startup naming the
namespace it is serving.

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
| 5b | medium | opus | none | Let the debugging client write what it reads. The premise this step exists for was found by checking another step's brief: `teneral/src/cli.rs` exposes `get`, `explain`, `diagnostics`, `mint-token`, `mint-epoch` and `watch`, and **no write of any kind**, so nothing in this repository can create a network, attach an interface or configure a range -- and step 5g cannot configure the lab it deploys. Read `teneral/src/client.rs` in full first: `one()` sends a `request::Kind` and prints the response, `get()` builds the read requests, and `selector`, `cursor_from`, `scope_from` and `capability_from` are the argument parsers to reuse rather than duplicate. Add one subcommand per write operation the server already serves -- the networks, attachments, address ranges and bindings in `docs/control-api.md`'s tables, the lease revocation, and `update_network` from 5a -- taking `--expected EPOCH:SEQUENCE` where the operation needs a position, with the help text saying it comes from a `get`. Nothing new goes in `teneral-api`: every one of these requests exists in the schema and the server serves it already, so a change in that crate means something has been misread. Add a test that names each write operation in the schema and asserts the client builds a request for it, so an operation added later without a subcommand fails rather than being quietly unreachable. Commit subject: "Let the debugging client configure a daemon." |
| 5c | high | opus | none | The wire layer, and nothing above it. Read decisions 2, 4, 5 and 13, and `docs/provenance.md`'s `dhcp-unasked-option-set` row. Create the `teneral-dhcp` crate: `publish = false` (phase 4 found that cargo-deny's wildcard-path allowance does not reach publishable crates), `rust-version.workspace = true`, the workspace lints inherited, `dhcproto = "0.15.0"` and nothing else beyond `teneral-store` -- and **not** `teneral-api` (decision 1). Add it to the workspace members and to `[workspace.dependencies]`. Then `wire`: a parse that refuses `hlen > 16` **before** any `chaddr` accessor is called and a datagram over 1500 bytes before it is parsed, returning a named error per cause; an `Answer` type holding the parameters decision 4's table lists; and an encoder producing the nine options in the measured order with T1 and T2 at a half and seven-eighths. The malformed corpus is the point of this step, not a side-effect: the inputs decision 5 names, each asserting a named refusal and no panic, as table-driven tests with the input described in the test name. Mutate your own guard to prove the tests see it -- copy the tree, remove the `hlen` check, confirm a named test fails, restore from the copy, each run under `timeout` -- and report which test killed it and the script's path. `make lint`, `make test` and `make deny` must all be clean, including the per-crate store check at the end of lint; run `make fetch` before `make deny` so the new dependency tree resolves. Commit subject: "Parse and build DHCPv4 messages safely." |
| 5d | high | opus | none | The decision layer: a parsed request plus its arrival interface, against a `BindingStore`. Read decisions 2, 6 and 11, finding 10, `docs/binding-store.md`'s lookup and lease-lifecycle sections, and the `lease-address-selection-order` and `selector-client-id-over-chaddr` provenance rows. The store already implements address selection, reservations, pools and the precedence order -- **do not reimplement any of it**; assemble a `ClientIdentity` from what the packet presents and ask. The path is: `interface_owner(namespace, ifname)` to get the network, refuse with a log line if nothing claimed it, then `lookup_binding_on_interface` for a reservation, then `allocate_lease` or `acquire_lease` or `renew_lease` or `revoke_lease` as decision 6's table says. The actor is `Actor::Daemon("dhcpv4")`. The send side is a trait with one method, so this layer is tested against a recorder. Tests: every row of decision 6's table, and **every one of them with two networks holding the same address** -- a DISCOVER arriving on each network's interface from the same MAC gets each network's own answer, a REQUEST for an address held in the other network is NAKed rather than ACKed, and a RELEASE in one network leaves the other's lease alone. That is the project's central correctness property and this is the layer where it is decided. Also: a request whose `giaddr` is non-zero is dropped with one log line, and an INFORM gets options and no lease. Report the test count. Commit subject: "Answer a DHCPv4 client from the binding store." |
| 5e | high | opus | none | The datapath and the daemon. Read decisions 7, 8, 9 and 14, and the `sock-*` and `netns-*` provenance rows -- every behaviour here is already measured and none of it is to be rediscovered. The receive path: one wildcard-bound UDP socket per served interface (a socket bound to the interface's own address never sees `255.255.255.255`), `IP_PKTINFO` for the arrival ifindex and the datagram's destination, and the interface name from an `ioctl` on a socket rather than from `/sys/class/net`. The reply path: `AF_PACKET` with a hand-built Ethernet/IPv4/UDP frame when the client has no address, and UDP with the egress named per message when it does; the BROADCAST flag is a request for broadcast and never permission to unicast. Phase 1d's spike (iii) in `docs/plans/PLAN-teneral-implementation-phase-01-research-spikes.md:974` built exactly this in Rust and a real client accepted it -- read it and follow what it found, including the privilege. Then the daemon: `--dhcp-interface` repeatable, `--dhcp-port` defaulting to 67 with its testing purpose in the help, the startup line decision 14 specifies, a refusal to start when a named interface is claimed by no network, and shutdown ordered so the sockets close before the store does (`serve.rs` has the existing order and its comment). Nothing here may bind 67 in a test: `make test` runs as a non-root uid with no capabilities and the bind returns `EACCES`, which is why the port is a flag. Say in your report which parts you could not test and why. Commit subject: "Serve DHCPv4 on a socket per interface." |
| 5f | medium | opus | none | The sweeper. Read decision 10, and the `sweep_expired_leases` documentation in `teneral-store/src/store.rs:570` -- particularly the paragraph saying nothing in the store waits for it, which is the premise the chosen interval rests on. A timer in the daemon, `--sweep-interval` in seconds defaulting to 86400, no sweep at startup, running over every network the store holds, with `Actor::Daemon("sweeper")`. It logs what it removed per network, and a sweep that removed nothing logs nothing. Tests: with the store's injectable clock (`teneral-store/src/clock.rs`), a lapsed lease is removed and its event appears; a live lease is not; a lease in one network is swept while the same address in another network is left alone; and a sweep that finds nothing writes no event and reports no position. Do not add a grace period -- decision 10 explains why it is future work rather than this step. Commit subject: "Sweep lapsed leases on a daily timer." |
| 5g | high | opus | none | The lab, the captures and the rig. **Do not deploy anything until the management session confirms the back brief's gate has been answered yes** -- this step touches real infrastructure. Then: `cd` to the `homelab-deployments-lfs` checkout, `git pull` (the local `main` is 13 commits behind the merge that carries the lab), deploy `playbooks/linux-bridge-lab-debian-13.yml`, and leave the gateways in the root configuration (`lbr-lab-gateway show` to confirm). Read `docs/linux-bridge-lab.md` in that repository and `docs/differential-rig.md` here, especially *How a captured pair gets into the corpus lawfully*, which is six numbered steps and is not optional: the versions, the `tcpdump` command, the pcap kept outside this repository with its sha256 recorded, the frames as annotated hex, and the write-up. Capture dnsmasq first, then stop it and run teneral on the same bridges with the same ranges -- a `make release` binary copied in, run with `CAP_NET_BIND_SERVICE` and `CAP_NET_RAW` -- and capture the same exchanges. What the phase needs: a DISCOVER/OFFER and a REQUEST/ACK from each of the two overlapping networks, a renewal, a RELEASE, an INFORM, and a NAK. Add the provenance rows the captures support, with their write-ups, and the cases that cite them. Then the rig: the `[network]` table in `case.toml`, the subject signature carrying it, field-wise comparison reporting every difference rather than the first, and the existing `dhcpv4-discover-offer-dhclient` case flipped from `pending` to `compared`. Correct `docs/differential-rig.md:192-203`, which promises byte comparison. Nothing in this step reads dnsmasq's source, and no measurement is taken anywhere but the lab. Report the lab's identifier, the versions, and every row id you added. Commit subject: "Compare teneral's DHCPv4 bytes with dnsmasq's." |
| 5h | medium | sonnet | none | Documentation and fold-back. This step writes no front-end code. `docs/binding-store.md` is not in this step: the lease time was written into it when 5a landed, because leaving a sentence saying a network is a name and nothing else would have left a false claim in the tree for six steps. Read it rather than rewriting it. (1) `docs/dhcpv4.md`: the front-end as a reference for somebody with no plan in hand -- the three layers, which messages are answered and what the others get, the option set with each option's source, the lease time and where it is configured, the namespace `/` and why, the capability set and how to run it by hand, the sweeper and the stickiness it chose, and the recorded-holder behaviour from decision 11 with its cause and consequence. (2) `ARCHITECTURE.md`: the front-end layer's first member exists, with what it serves and what it does not. This is a shape change, so it belongs there. The stale claim about protocol options was already corrected by this plan's commit, in the paragraph that lists what is deferred -- check it and do not correct it twice. (3) `AGENTS.md`: only conventions that changed -- a fourth crate, the namespace `/` for an interface in no namespace, and the rule that a wire behaviour without a provenance row is not shipped if it is not already there. Nothing else; every line there costs context on every task. (4) `docs/development.md`: the new crate in the layout, and that `make test` cannot bind privileged ports so the datapath's socket layer is proved in the lab. (5) Fold back into the master plan: phase 5's row, and the *Future work* entries this phase closes or rewrites -- the sweep-interval entry (now chosen, with the grace period named as what would decouple the two), the client-identifier entry (declined, with where it is now documented), the differential-rig entry (the lab was redeployed, with what it produced), and new entries for the deferred option table and the DECLINE exclusion. Write each in full rather than naming it. The view-bound entry was already rewritten by this plan's commit, which is where it was found to name the wrong phase; read it rather than rewriting it. Commit subject: "Document the DHCPv4 front-end." |

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
Finding 3 is measured: no privileged port, no `CAP_NET_RAW`.
Mitigation is structural rather than procedural -- decision 2 puts
every judgement above the socket, so what cannot be tested is
"the bytes left the right fd" -- plus the lab, which tests exactly
that with a real client. The risk that remains is a lab that
cannot be deployed, which is why it is a back-brief gate rather
than an assumption.

**The namespace string is hard to change later.** Decision 8 picks
`/` and an operator's attachments are keyed by it, so changing it
after a deployment exists means a migration of rows a human typed.
Mitigation: it is decided here rather than in phase 8, the
collision argument is the reason rather than taste, and phase 8's
own file inherits it as a given rather than a choice.

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
   `update_network` is removed from either side.
7. `1.89` is still the only place the MSRV floor is written outside
   the lockfile, and if `dhcproto` raised it the raise is in the
   same commit with a comment naming what needed it.
8. The schema version is 8, the migration is forward-only, and a
   store created by 0001-0007 opens and reports the default lease
   time without an operator writing one.
9. Every write operation the server serves is reachable from the
   debugging client, asserted by a test that names them rather
   than by counting subcommands by hand, and the lab in 5g was
   configured with that client and nothing else.
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
14. The nine options of `dhcp-unasked-option-set` appear in an
    OFFER in the measured order, with T1 at half the lease and T2
    at seven-eighths, asserted against the bytes rather than
    against the builder's inputs.
15. An INFORM is answered with options, no `yiaddr` and no lease
    time, and no lease row is written.
16. A REQUEST naming another server's identifier produces no
    reply, and a REQUEST for an address another client holds
    produces a NAK.
17. A non-zero `giaddr` produces no reply and exactly one log
    line naming the relay.
18. An interface with no address, or with more than one, is
    refused with a log line naming the interface rather than
    answered from a guess.
19. The sweeper removes a lapsed lease and leaves a live one,
    writes no event when nothing expired, and sweeps one network
    without touching the same address in another.
20. The daemon refuses to start when `--dhcp-interface` names an
    interface no network has claimed, and the refusal names it.
21. The daemon's startup line names, per served interface, the
    network, the namespace, the derived address and prefix, and
    the lease time -- read from a real run, not from the source.
22. `teneral serve` with no `--dhcp-interface` behaves exactly as
    phase 4 shipped it, proved by phase 4's own daemon tests still
    passing unchanged.
23. The lab was deployed, teneral served both overlapping networks
    through one process, and a real client bound an address on
    each -- with the client's own output recorded, not teneral's.
24. `cross_network_answers` is zero in the diagnostics after the
    lab run, read from the running daemon.
25. Every new provenance row cites an RFC section or a named
    observed-behaviour test, each new row is cited by at least one
    case, and the rig fails a case citing a row that does not
    exist.
26. `dhcpv4-discover-offer-dhclient` is `compared` rather than
    `pending`, the rig's subject registration and the case's
    status agree, and a deliberately wrong field in the subject
    makes the rig fail naming that field.
27. No document says the binding store holds protocol options, and
    `grep -rn 'protocol options' ARCHITECTURE.md docs/` returns
    only accurate sentences.
28. `docs/dhcpv4.md` exists and states the lease time's home, the
    namespace `/`, the capability set, the sweeper's interval with
    what it chose, and the recorded-holder behaviour.
29. The master plan's phase 5 row and every *Future work* entry
    this phase closes or rewrites are updated, and each new
    deferral is written in full rather than named.
30. Nothing this phase started is still running: no teneral
    process on this host or in the lab, no container, no `cargo`
    on the host, and the lab either torn down or reported as
    deliberately left up.

## Back brief

My understanding of this phase, and the points that need an answer
before a step runs.

The phase builds a DHCPv4 front-end in a new crate, in three
layers, and the layers exist in that shape because the
repository's own test target cannot bind port 67 or open an
`AF_PACKET` socket -- so everything that decides anything lives
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
decisions stand as written rather than as amended.

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
