# Architecture

**This describes the intended shape of the system. None of it is
built.** The plan that builds it, and the research spikes that
gate it, are in
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
single process serve networks whose addresses overlap. That has not
been confirmed experimentally yet, and it is the assumption the whole
design rests on; the research spike that settles it is in
[the implementation plan](docs/plans/PLAN-teneral-implementation.md).

A flow-based datapath (OpenFlow packet-in, or tc-bpf keyed on
ifindex) was considered and is not planned. It would serve only
Neutron's non-default `dhcp` L2 extension, and OVN already
occupies that design space.

**The control plane** is an API, not a configuration file. Config
files, a database watcher and an operator's user interface are all
interchangeable clients of the same API, and the API is the only
way into the store. It carries a watch/subscribe primitive,
because a consumer needs to learn which address a given client was
assigned -- the thing Shaken Fist does today by reading a lease
file.

## Why this shape

dnsmasq is one process per network because its configuration is
global state parsed at startup, and because two networks using
`10.0.0.0/24` cannot share a namespace. Both are properties of its
architecture rather than of the protocols. Keying the store by
network removes the first; creating each network's socket in its
own namespace removes the second. Memory safety is a bonus of the
implementation language, not the reason for the project.

## Related documents

- [docs/provenance.md](docs/provenance.md) -- where each behaviour
  came from, and the licence reason that matters.
- [docs/plans/](docs/plans/) -- what is being built, in what
  order, and what is deliberately excluded.
- [AGENTS.md](AGENTS.md) -- the invariants that follow from this
  shape.
