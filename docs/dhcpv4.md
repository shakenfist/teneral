# The DHCPv4 front-end

What `teneral-dhcp` does, what it refuses to do and what each
refusal costs, and how to run it. It is written for somebody who has
not read the plan that built it. The behaviour is cited to an RFC
section or a measured row in [provenance.md](provenance.md); where
this page names a row, that row is the evidence and this page is only
the summary.

The front-end serves DHCPv4 on named interfaces of one host, in no
network namespace, answering each client from the network that
claimed the interface the request arrived on. It reads the binding
store and writes leases into it. It holds no per-network state of
its own, so there is nothing to configure on it except which
interfaces to serve.

It does not serve BOOTP-only clients, relayed requests, DHCPv6, PXE
or boot options, or arbitrary and vendor options. Each is covered
below with the reason.

## The three layers

`teneral-dhcp` is a library crate beside the store and the control
API, not a module of the `teneral` binary. A binary has no lib target,
so nothing in its integration tests could call it, and the
differential rig has to call whatever builds a reply. It depends on
`teneral-store` for the trait and not on `teneral-api`: a front-end is
a consumer of the store, and a dependency from the front-end onto the
control plane would make the layer diagram in
[ARCHITECTURE.md](../ARCHITECTURE.md) untrue.

Inside the crate there are three layers, and the seams are chosen so
that the part the repository's tests cannot reach is as small as it
can be made.

| Layer | What it does | What it may not do |
|-------|--------------|--------------------|
| `wire` | Bytes to a parsed request, and answer parameters to bytes. Every malformed-packet test and every option-encoding test lives here. | Read the store, open a socket or ask the time. |
| `decide` | A parsed request plus the interface it arrived on, asked of the store, producing an answer or a silence. It resolves the network, builds the client's identity, and writes leases. Every two-network test lives here. | Open a socket. It sends through a one-method trait, so it is tested against a recorder. |
| `datapath` | The sockets: receiving with the kernel's report of the arrival interface, naming the egress in each message, and the frame that reaches a client with no address. | Decide what a client is told. It decides where the bytes go. |

The reason for the shape is that `make test` cannot open an
`AF_PACKET` socket and has one interface, loopback. Anything decided
inside the datapath is decided where the repository's tests cannot
see it, so every judgement is pushed above that line, and what is left
untested is "the bytes went out of the right descriptor", which the
lab measures with a real client. See [development.md](development.md).

Decoding uses the `dhcproto` crate, behind guards that run first.
Encoding does not: that crate holds options in a map keyed by code and
so emits them in code order, while the order an answer's options are
written in is a measured property of its bytes, so the encoder is this
project's own.

## Which messages are answered

| Message | Answer |
|---------|--------|
| DISCOVER | An OFFER, and a lease written for the network's full lease time. The address is the one a binding reserves for this client if there is one, otherwise the store's choice, honouring an address the client asked for only at its place in RFC 2131 section 4.3.1's order. |
| REQUEST, naming this server | An ACK if the address is one this network may give this client, otherwise a NAK. |
| REQUEST, naming another server | Nothing. It is not this server's to answer. |
| REQUEST, naming no server, with `ciaddr` set (renewing or rebinding) | An ACK from a renewal; a NAK when the store refuses because the lease lapsed or another client holds the address. |
| REQUEST, naming no server, with the address in option 50 (checking after a reboot) | The same, except that a client this network has no record of at all gets silence, and a client naming the address its own reservation holds is given it. |
| RELEASE | The lease is revoked. No reply: the protocol specifies none. |
| DECLINE | The lease is revoked and the event is logged. No reply. |
| INFORM | An ACK carrying the derived configuration, with no address, no lease time and no lease written. |

A few of those want explaining.

**Silence to a client with no record.** RFC 2131 section 4.3.2 makes
it a MUST that a server with no record of a client stays silent to its
post-reboot request, and gives the reason: two servers that do not
talk to each other have to coexist on one wire. Two DHCP servers on a
bridge is what a migration looks like, and a teneral that NAKed every
other server's clients when they rebooted would break the network it
was added to. A *record* is a lease the client holds in this network,
live or lapsed, or a binding configured for it here. Where there is
one and it disagrees with the client, the answer is a NAK. A lease
held in another network is not a record in this one. The row is
`dhcp-init-reboot-silence-without-a-record`.

**An OFFER commits a lease.** The lease row is written before the
OFFER is sent, for the network's whole lease time, and it is visible
to `list-leases` and to a watching consumer immediately -- before the
client has accepted anything, and whether or not it ever does. That is
worth knowing in two places. An operator looking at a pool that seems
fuller than the number of working clients is probably looking at
offers nobody took up, and each of those holds its address until the
lease lapses. And a consumer counting leases is counting clients that
have been offered an address rather than clients using one.

**May this client have that address.** `allocate_lease` chooses an
address and `acquire_lease` records one somebody else chose, and
neither answers "may this client have the address it just asked for".
`acquire_lease` is deliberately permissive, because an operator
configuring a reservation may name an address outside every range. A
client on the wire must not be able to talk its way into one, so the
front-end checks the named address against the network's ranges and
against what its other bindings reserve before it acquires. That is
the one place this layer reads facts out of the store to answer a
question; it does not choose an address, which stays the store's job.

**DECLINE is the cheap answer.** A DECLINE says the address teneral
handed out is already in use by something the store does not know
about. The complete response is to stop offering that address, which
needs a state the store does not have: an exclusion that allocation
consults and that expires on its own. What teneral does is revoke the
lease and log the event, so the address returns to the pool and may be
offered again. If the conflict is permanent that is a loop, with the
client declining and being offered the same address until something
else changes. What it does not do is stay silent, because a DECLINE is
the clearest evidence a server ever gets that its belief is wrong.

**A relayed request is dropped.** A non-zero `giaddr` is not
answered. Answering a relay correctly means replying to the relay and
taking the subnet from `giaddr` rather than from the arrival interface,
which needs a subnet model teneral does not have: everything below is
derived from the interface. Answering one incorrectly means a lease in
the wrong subnet. The drop is logged once per relay and interface
rather than once per packet, so a misconfigured relay costs one line
and is not mistaken for a silence nobody chose. The record of which
relays have been reported is bounded, because its key comes from a
packet, and at its limit it forgets the oldest, which may cost a
second line about a relay last seen long ago.

**An unclaimed interface is a refusal.** A request arriving on an
interface no network has claimed is not answered, and the daemon logs
the interface by name. At startup the same condition refuses to start;
see below.

## The options in an answer

An answer to a client that asked for nothing carries nine options in
this order, which is what the reference implementation was measured
sending (`dhcp-unasked-option-set`). Each comes from the only honest
source, and none from configuration the front-end keeps.

| Option | Source |
|--------|--------|
| 53 message type | The answer being built. |
| 54 server identifier | The address of the interface the request arrived on. |
| 51 lease time | The network's lease time. |
| 58 renewal time (T1) | Half the lease time. |
| 59 rebinding time (T2) | Seven-eighths of the lease time. |
| 1 subnet mask | The prefix configured on that interface. |
| 28 broadcast address | That address and mask. |
| 3 router | The interface's own address. |
| 6 name server | The interface's own address. |

Router and name server default to the server's own address because
that is what the reference implementation does when nothing else is
configured, and teneral has nothing else to configure.

The two times are fractions of the lease time measured on a first
grant. A renewal acknowledgement from the reference implementation was
measured carrying T1 and T2 *below* those fractions, and lower again on
each renewal (`dhcp-renewal-lowers-t1-and-t2`); the mechanism was not
established, and teneral does not reproduce it.

An INFORM is answered with six: 53, 54, 1, 28, 3 and 6, with no lease
time and no times (`dhcp-inform-answered-without-a-lease`). A NAK
carries the message type and the server identifier and nothing else,
because a refusal that carried a mask and a router would be describing
a network the client is being told it is not on.

Reading an interface's address and prefix is observation, not
per-network state: it is read again for every request and nothing is
remembered, so a front-end that holds no state of its own still holds.
An interface with no IPv4 address, or with more than one, is a refusal
rather than a guess. A server that picked one of two addresses to call
itself would answer differently depending on the order the kernel
returned them, and answering inconsistently is worse than not
answering. The refusal is logged naming the interface, because it is a
configuration fault with an operator-shaped fix.

**Replies are padded to 300 octets.** RFC 1542 section 2.1 lists
consistency checks a BOOTP entity SHOULD perform on arrival, the first
being that the IP and UDP lengths be large enough to hold the minimal
300-octet BOOTP header, and says a message failing one MUST be
silently discarded. An unpadded OFFER is under 300 octets, so teneral
without padding was sending datagrams a conforming client or relay
agent is told to drop. The padding is zero octets after the end option,
which is the pad option's own code, and it is a floor and not a limit:
the same section requires a receiver to accept a longer message. The
rows are `dhcp-reply-meets-the-bootp-length-check`, which is the RFC
requirement, and `dhcp-reply-padded-to-bootp-minimum`, which is what
the reference implementation was seen to do. They are different claims
and are kept separate.

### What is deferred

A router or name server that is not this machine, a domain name, an
MTU, and any option outside the nine are not available. Each would be
a row in a table that does not exist, and its shape should be settled
by the first consumer that names an option it needs. The master plan's
*Future work* describes the obvious shape: an option code, a network
or a binding, and bytes.

## The lease time

The lease time is a column on the network, `lease_seconds`, with a
schema default of 43200 seconds (twelve hours, which is the lab's
`dhcp-range=...,12h`). It is set with `update-network`:

```
teneral update-network lab 3600 --expected EPOCH:SEQUENCE
```

The position is the `cursor` of a `get` of the network, and the write is refused if
the network has moved since. The value must be positive and no more
than 4294967295, since a longer lease is one an operator could set and
never see take effect. There is no `--lease-time` flag: a daemon-wide
flag for a per-network value would be configuration outside the API.
It is on the network rather than the address range because teneral's
range is not dnsmasq's. A binding's address need not lie inside any
range, so a lease time on the range leaves the question of what lease
time such an address gets with no answer, and the usual patch for that
is a network-level default, which is the thing the column is. A
per-range override waits for a consumer that wants two lease times in
one network. [binding-store.md](binding-store.md) describes the column.

The shortest lease is two seconds. Below that the renewal and
rebinding times derived from it come out as zero, which tells a client
to renew immediately and gives it no interval in which to do it.

**The lease time is also the blast radius of a flood.** Because a
DISCOVER commits a lease for the full term, any host on the link can
vary its `chaddr` and consume one pool address per packet, each held
until it lapses -- so a twelve-hour lease over a pool of fifty
addresses is fifty packets away from a pool that answers nobody for
twelve hours. Nothing in DHCP authenticates a client, so the exposure
is inherent rather than a defect, but the lease time is the dial that
sets how long it lasts and the pool size sets how little it takes. On
an untrusted link, prefer a short lease and a pool larger than the
number of clients. A shorter offer term that the REQUEST extends would
narrow it further; teneral does not do that, because what the
reference implementation does with an offer no client accepts has not
been measured here.

## How a packet becomes a selector value

The store normalises a hardware address and nothing else, and says
that the front-end which renders an option into a selector value owns
the choice of form. The configuring side and the matching side have to
agree on it and the store cannot check that they do. Get it wrong and
a reservation is configured, shows in every listing, and never matches
a real client. So these are the forms an operator types:

| Attribute | Form |
|-----------|------|
| Hardware address | `chaddr` truncated to the `hlen` octets the client declared, as lower-case colon-separated hex. The store accepts three written forms, so an operator may type any of them. |
| Client identifier (option 61) | Every byte of the option, including the leading type byte, as lower-case colon-separated hex. This is the form the reference implementation's lease file uses, so a value can be pasted from one. |
| User class (77), vendor class (60) | The option's bytes as text, when they are valid UTF-8 and the store will take them. |
| DUID and IAID | Never presented by a DHCPv4 client. |

An attribute the store will not take -- a vendor class with a control
character, a client identifier that is empty or over its limit, a
hardware address that is not six octets -- is not presented, and the
fact is recorded the way a dropped option is. The client loses that
attribute and keeps its lease. The test that checks the agreement
renders a value from a packet's bytes, configures a binding with the
value an operator would type, and matches the packet against it; a
test of the renderer alone would be asserting the renderer against
itself.

### A client that stops presenting its client identifier

A lease records one holder: the highest-precedence attribute the
client presented when it acquired the address. The lookup that asks
what a client holds does consult every attribute it presents, so a
client that begins presenting something of *higher* precedence is still
recognised. What cannot be fixed that way is the converse: a client
that supplied a client identifier and later stops supplying one
presents nothing that matches the attribute its lease was recorded
against. As far as anything can tell it is a different client, so it
is given a second address and the first stays held until it lapses.

This is declined rather than overlooked. Recognising the client again
would mean recording every attribute it presented at acquisition and
matching on any of them, which is a side table, a retention question,
and a deliberate decision to treat two identities as one machine --
the guess RFC 2131's own warning about keying on a hardware address is
about. The store cannot know, and one that guessed would hand a client
another client's address wherever the guess was wrong. If this shows
up in practice the fix is for the client to present the same
identifier consistently.

## The namespace `root`

An attachment is identified by a namespace and an interface name, and
the namespace is a required string. An interface in no namespace --
which is every interface this front-end serves today -- is attached
under `root`:

```
teneral attach-interface lab root br0
```

The startup line names the namespace, and `--dhcp-interface` looks the
interface up under it.

It is not `/`, which was the first choice and cannot work. An
attachment's identity reaches an event as its two halves joined by
`/`, so the store refuses that character in either half rather than
escaping it, to keep `a/b` plus `c` from becoming the same event as
`a` plus `b/c`. It is not an empty string, which is indistinguishable
from a field somebody forgot to fill in, and not `.`, which would be
collapsed by any consumer that put an attachment's identity in a URL
path.

`root` can collide, because `ip netns add root` succeeds. The
collision is loud: an attachment is keyed by `(namespace, ifname)`, so
an interface here and an identically named one inside a namespace
called `root` claim one identity, and the store refuses the second
claim naming the holder. Nothing leaks between networks and nothing is
answered wrongly; an operator is told they cannot attach that
interface, and can rename the namespace. That is cheaper than changing
an identity format every event already carries, and a deployment with
such a namespace and an identically named interface in both is
pathological.

## Running it

The daemon serves DHCP only for interfaces it is told to. With no
`--dhcp-interface` it is exactly the control-plane daemon: no
privileged port is bound and no raw socket is opened.

| Capability | For |
|------------|-----|
| `CAP_NET_BIND_SERVICE` | Port 67, on a host whose unprivileged port range starts above it. |
| `CAP_NET_RAW` | The `AF_PACKET` socket, which is how a reply reaches a client that has no address yet. |

Those two and no others. `CAP_SYS_ADMIN` is what serving a network
namespace will need and is not needed here. Teneral was run in the lab
serving two overlapping networks from one process, as an unprivileged
uid holding exactly those two capabilities.

By hand, with the interface already configured with exactly one IPv4
address:

```
# Grant the binary the two capabilities, once.
sudo setcap cap_net_bind_service,cap_net_raw+ep /path/to/teneral

teneral serve --store /var/lib/teneral/store.sqlite \
    --dhcp-interface br0
```

and, against the running daemon:

```
teneral create-network lab
teneral attach-interface lab root br0
teneral create-address-range lab pool 10.0.0.100 10.0.0.200
```

The interface must be attached before the daemon will serve it: a
named interface no network has claimed is a refusal to start, because
a daemon that bound port 67 and then answered nothing would be worse
than one that did not come up. An interface named twice, and an
interface with no single address to derive an answer from, are refused
at the same point and before any socket is opened. Packaging and how
the capabilities are granted in a deployment are not decided here.

At startup the daemon logs one line per served interface, naming the
network that claimed it, the namespace, the address and prefix it
derived, the lease time and the port. That line is the whole of where
an answer's contents come from, made visible, and it is the first
thing to read when a client got an answer nobody expected.

`--dhcp-port` defaults to 67 and is a flag so that a test can use a
port the kernel chose. A client cannot be told to use another port, so
a deployment has no reason to move it, and it is not a second place to
configure one.

### How a reply reaches the client

The delivery order is RFC 2131 section 4.1's, for a zero `giaddr`
(`dhcp-reply-addressing-order`):

1. A NAK is broadcast, whatever else is true of the client.
2. Where `ciaddr` is set, the reply is unicast to it, by UDP, with the
   egress interface named in the message so that a link with no default
   route still works.
3. Where `ciaddr` is zero and the BROADCAST flag is set, the reply is
   broadcast.
4. Otherwise it is sent below the IP layer, as an Ethernet frame to the
   client's hardware address carrying the offered address.

The BROADCAST flag is read as a request for broadcast and never as
permission to unicast: a client that cleared it accepted a broadcast
reply in all seven observed transactions. Receiving uses one
wildcard-bound socket per interface, because a socket bound to the
interface's own address never sees datagrams sent to `255.255.255.255`,
and takes the arrival interface and the datagram's destination from
`IP_PKTINFO`. The interface name comes from an `ioctl` on a socket and
never from `/sys/class/net`, which ignores the namespace of the thread
reading it; that costs nothing with one namespace.

Each interface is read and answered in turn. That bounds what one
client can make the process do: the number of requests being decided
at once is the number of interfaces an operator configured, and no
client can raise it. It also keeps the datapath out of the control
plane's view bound, whose permits it cannot take. The diagnostics'
live-view count is the store's own, across the whole process, so it
includes the datapath's views and is reported beside the API's bound:
the day the two disagree is visible. The reference
implementation is single-threaded too, and the load given up is not
load this protocol produces: a client speaks twice a lease time, so
four clients on a twelve-hour lease send eight datagrams a day. If a
consumer ever needs more, the fix is a bounded worker set per socket.

## The sweeper

`--sweep-interval` is in seconds and defaults to 86400. The daemon
sweeps every network's lapsed leases on that interval and not at
startup, and logs what each sweep removed per network; a sweep that
removed nothing logs nothing.

One interval sets two things that cannot be set apart: how soon a
consumer is told an address was given up, and how long a client that
has been away keeps getting its old address back. The store picks the
second step of its selection order by finding the client's lapsed
lease row, and the sweep deletes that row. Teneral picks the lowest
free address where dnsmasq picks by client identity
(`dhcp-address-by-identity`), so the lapsed row is the only source of
address stability it has.

A day optimises for the stickiness: a client that was away for a day
gets its address back. The expiry event is a convenience, since every
read evaluates expiry against the clock and a lease on the wire
carries both `expires_at` and `expired`. Sweeping promptly would
throw away the only mechanism that gives stability in order to make a
message timelier. There is no sweep at startup so that a restart is
not itself the stickiness bound, being the event most likely to
coincide with the outage a client is returning from.

Decoupling them is possible: emit the expiry when the lease lapses and
delete the row later, which is two writes, a grace period and a row
state the schema does not have. It is written up as future work.

An interval larger than the monotonic clock can name an instant that
far ahead is refused at startup, naming the flag.

## What the parser refuses

Every refusal is a named error that the daemon logs, not a silent
drop, and none of them is an incident: each means somebody put bytes on
a link that no server should answer. The parser refuses a datagram
longer than 1500 octets before it parses anything, and one shorter
than the fixed BOOTP header and its magic cookie. The limits below are
narrowings of what the reference implementation serves, each kept on
purpose.

| The parser refuses | Why, and what it costs |
|--------------------|------------------------|
| `hlen` over 16 | The decoder sizes a slice of the 16-octet `chaddr` field by this byte. Above 16 the owned accessor panics and the borrowed one returns neighbouring packet bytes as the client's hardware address. The fix is merged upstream and in no release this project can resolve, so the guard runs before any accessor. It is a refusal and not a clamp, which would invent a hardware address. `hlen` of exactly 16 is accepted. Costs nothing a real client sends. |
| `hlen` of zero | The packet presents no hardware address. A client with none is identifiable only by its client identifier (RFC 4390 specifies that for InfiniBand), but this front-end builds an Ethernet frame for a client with no address and has nothing to address it to. Costs such a client its lease. The reference implementation also does not answer it (`dhcp-hlen-zero-unanswered`). |
| An all-zero hardware address | The same reason. An all-zero address is not one a lease can be keyed by, and would otherwise become a selector matching every such client. Costs such a client its lease. The reference implementation serves it (`dhcp-zero-chaddr-is-served`), so this is a place teneral answers less than it does. |
| A message whose `op` is not a request, or whose type is not one a client sends | An OFFER or ACK arriving on the server's port is another server or a reflection, not a question. Costs nothing: the right answer to it is silence with a name on it. |
| A plain BOOTP request, with no message type option | RFC 2131 requires the option in every DHCP message. Answering a BOOTP client as though it had sent a DISCOVER would hand a lease to a client that cannot renew it. Costs every BOOTP-only client service. The reference implementation classifies it as BOOTP and declines it where no address is configured for it (`dhcp-no-message-type-is-bootp`), but what it does where one is was not measured; teneral serves no BOOTP-only client. |
| A magic cookie that is not DHCP's | The options area is not DHCP's. A vendor-specific BOOTP message otherwise decodes without complaint and its options are read as though they were DHCP's. |
| An option record that overruns the datagram, or has a code and no length | A framing error. The boundaries of every record after it are unknown, so there is nothing to drop and carry on from, and the whole datagram goes. |
| More than four undecodable options | Each option dropped costs a decode of the whole datagram, and the number of passes is bounded only by the number of option codes the decoder has a type for, some sixty. Without a limit one packet would buy its sender sixty decodes on the one thread that serves every network. Four drops is the bound; one or two is the ordinary case of an ordinary client, and a sender offering five is not one whose lease anybody is protecting. Costs such a client its lease, and bounds the work of a packet to a constant. |

An option whose framing is sound and whose value will not decode is
**not** a refusal. It is dropped, the request records which code was
dropped, and the client is answered. The distinction between framing
and value matters because the decoder stops at the first option it
cannot read and returns *success*, so a packet with one bad option
would otherwise parse cleanly and short, silently missing whatever
followed -- possibly the requested address or the client identifier,
which are the fields that decide which address a client gets. So the
options are walked before decoding, and the two failures are treated
differently. Value failures fall on ordinary clients: a Windows machine
with a non-ASCII name sends option 12 in a local codepage, and losing
that name should cost the name and not the lease. Nothing is lost on
the options that matter, since each is asked for by name and its
absence is already a refusal. The reference implementation answers such
a request in full and omits the name
(`dhcp-undecodable-option-value-still-answered`).

A panic in the parser would be a remote denial of service against every
network the process serves, which is why every one of these is an
error value, and why `unwrap_used` is `deny` in the workspace.

## Where this was measured

The behaviours above that no RFC specifies were measured against
dnsmasq in the Linux-bridge lab; [provenance.md](provenance.md) has the
rows and [differential-rig.md](differential-rig.md) describes how a
captured exchange becomes a case that compares teneral's reply with
the reference's field by field. The datapath's reply path is proved
there, with a real client, and not by `make test`.

The decoder's dependency subtree is 54 crates counting `dhcproto`
itself, and 52 of them are in the subtree of `hickory-proto`, which
the decoder needs for its domain-name options: the DNS library, and
the URL parser and ICU Unicode tables it builds on. The only two that
are not are `dhcproto` and its macro crate. The DNS front-end wants
that library anyway, so it arrives early rather than needlessly, and
`make deny` passes with no exceptions. The alternative, a
hand-written codec with no dependency, is recorded in the master
plan's *Future work*.
