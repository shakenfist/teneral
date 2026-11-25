# Architecture

**This describes the intended shape of the system. The binding
store at the middle of it is built; everything above and below it
is not yet.** The plan that builds the rest, and the research
spikes that gate it, are in
[docs/plans/PLAN-teneral-implementation.md](docs/plans/PLAN-teneral-implementation.md).

## The idea

DHCP, DNS, TFTP and an instance metadata service are all the same
shape of problem: *a request arrives from an identified client on
a known interface, and the answer is determined by that client's
identity.* They are conventionally four services, each holding its
own idea of who is who. Teneral makes them four front-ends over
one store.

## The layers

```
    DHCPv4      DNS      TFTP     HTTP metadata     <- front-ends
      \          |        |         /
       \         |        |        /
        +--------+--------+-------+
                     |
              the binding store                     <- layer 0
                     |
        +--------+--------+-------+
       /                           \
  socket per interface      socket in a netns       <- datapaths
  (home, flat networks)     (Shaken Fist, Neutron)
```

**The binding store** is the authoritative mapping from
`(network, interface) x client identity` to a set of facts. Client
identity is the union of the things a client presents: MAC,
client-id, DUID/IAID, user-class, vendor-class. Facts are
addresses, hostnames, lease state, protocol options, and arbitrary
key-to-content blobs. Everything else in the system is a
projection of this, which is why it is keyed by network from the
first commit rather than acquiring that key later.

It exists, as the `teneral-store` crate: SQLite behind an
asynchronous trait, holding networks, attachments, address ranges,
bindings and leases, with the event log that gives the control
plane's cursor a total order. It also holds the tokens a consumer
authenticates with, and every write names the actor that made it --
a token, a local peer, or a named part of the daemon -- on both the
event and the row. Zones and records and content blobs are deferred
to the layers that will consume them. See
[docs/binding-store.md](docs/binding-store.md).

**The front-ends** each turn a wire request into a lookup and a
lookup result back into a wire response. They share socket
plumbing and nothing else. A front-end holds no per-network state
of its own; if it needs to remember something, that belongs in the
store.

**The datapaths** are how a request reaches a front-end. There is
one design, with a variation: a socket per interface, created
inside a network namespace where one exists. Home networks have no
namespaces; Shaken Fist and Neutron's DHCP agent both put each
network in its own. A socket created in a namespace keeps serving
that namespace after the process moves away, which is what lets a
single process serve networks whose addresses overlap. That was the
assumption the whole design rested on, and it was confirmed
experimentally on 2026-11-21: the measurement is the
`netns-socket-bound-at-creation`, `netns-setns-moves-thread` and
`netns-held-fd-keeps-namespace` rows in
[docs/provenance.md](docs/provenance.md). A namespace holding more
than one interface was not covered, and is still open; see
[the implementation plan](docs/plans/PLAN-teneral-implementation.md).

A flow-based datapath (OpenFlow packet-in, or tc-bpf keyed on
ifindex) was considered and is not planned. It would serve only
Neutron's non-default `dhcp` L2 extension, and OVN already
occupies that design space.

**The control plane** is an API, not a configuration file. Config
files, a database watcher and an operator's user interface are all
interchangeable clients of the same API, and the API is the only
way into the store. The daemon also serves a simple web interface
over that same API, from the shared `sfui` design system vendored
into its static assets; it is a client of the API like any other
and has no privileged path of its own.

Human identity comes from an authenticating reverse proxy, which
terminates TLS, runs the OIDC flow and forwards the authenticated
user as request headers -- the arrangement the `private-ci`
conductor dashboard runs. Those headers are evidence only because
the proxy is the only thing that can reach the listener, so the
listener that trusts them is separate from the one programmatic
clients use with a bearer token. Authorisation -- which networks
an identity may touch -- stays in the daemon, because it is the
only component that knows what a network is.

The API carries a watch/subscribe primitive, because a consumer
needs to learn which address a given client was assigned, and to
be told when the store's view of it changes.
Shaken Fist today assigns the address itself and parses dnsmasq's
lease file only to police the result, repairing a disagreement by
killing the process -- so what it lacks is not discovery but any
way to ask what the server believes, or to correct it.

## Why this shape

dnsmasq is one process per network for two reasons, both
properties of its architecture rather than of the protocols, and
both measured rather than assumed -- see
[docs/provenance.md](docs/provenance.md). Its configuration is
global state parsed at startup: a `dhcp-range` change needs a
restart, where a hosts file is re-read on SIGHUP. And its lease
table has no network dimension, so it cannot hold a lease of one
address for two networks at once -- the lease file's five fields
name an expiry, a MAC, an address, a hostname and a client id, and
nothing names a network.

Keying the store by `(network, identity)` removes the second,
which is the one that cannot be worked around from outside; an API
instead of a file removes the first. Note what is not a reason:
two networks using `10.0.0.0/24` can share a namespace perfectly
well, one socket can serve both, and the kernel answers ARP on the
interface a request arrived on, so each client still reaches its
own gateway. Per-network namespaces are how the consumers already
build their networks, not something teneral needs in order to tell
two networks apart. Memory safety is a bonus of the implementation
language, not the reason for the project.

## Related documents

- [docs/binding-store.md](docs/binding-store.md) -- layer zero as
  built: the schema and its keys, the lease lifecycle and its
  durability point, the history and its bound, and the diagnostics.
- [docs/provenance.md](docs/provenance.md) -- where each behaviour
  came from, and the licence reason that matters.
- [docs/differential-rig.md](docs/differential-rig.md) -- how a
  behaviour gets measured and kept, and how a case in the corpus is
  tied to the row it is evidence for.
- [docs/plans/](docs/plans/) -- what is being built, in what
  order, and what is deliberately excluded.
- [AGENTS.md](AGENTS.md) -- the invariants that follow from this
  shape.
