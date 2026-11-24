# teneral

*Teneral*, of a newly emerged insect: present, soft, and not yet
hardened into what it will be. Which is the state of every machine
that has just booted onto a network and does not yet know its own
address, its own name, or where anything is.

Teneral is a daemon that answers those questions. One request from
an identified client, arriving on a known interface, gets back the
content configured for that client. DHCP, DNS and -- later -- TFTP
and HTTP metadata are front-ends over a single store of bindings,
rather than three services that each hold their own copy of who is
who.

**Status: early.** The repository carries its build, its lint and
licence gates, its CI lanes and the differential rig, and the
binding store every front-end will read is built. The daemon itself
is still a skeleton that prints its version: nothing yet speaks
DHCP, DNS or an API. See the
[implementation plan](https://github.com/shakenfist/teneral/blob/develop/docs/plans/PLAN-teneral-implementation.md).

## Why

The conventional way to serve DHCP and DNS to many isolated
networks is one dnsmasq process per network, because dnsmasq's
configuration is global state parsed at startup and because its
lease table has no network dimension: it cannot hold a lease of
`10.0.0.50` for one network and another for a second at the same
time. Asked to, it either serves the wrong network's pool without
complaint or refuses a network whose addresses are entirely free.
That model costs a process per network -- thousands of them in a
cloud -- and it means the service holds no queryable record of
what it has handed out.

Teneral keys its binding store by network from the first commit.
One process serves many networks, configuration arrives through an
API rather than a file, and the store is the single source of
truth that both the protocol front-ends and an operator's user
interface read. Once bindings can carry arbitrary content as well
as addresses, the same mechanism serves instance metadata.

## Who it is for

- Anyone running a home network who wants DHCP, DNS and an address
  register that agree with each other, with a real API underneath.
- Cloud and virtualisation projects currently running a DHCP
  process per tenant network. [Shaken
  Fist](https://github.com/shakenfist/shakenfist) is the first
  intended consumer of that kind.

## Documentation

- [Implementation plan](https://github.com/shakenfist/teneral/blob/develop/docs/plans/PLAN-teneral-implementation.md)
  -- the phases, the research spikes, and what is deliberately out
  of scope.
- [The binding store](https://github.com/shakenfist/teneral/blob/develop/docs/binding-store.md)
  -- the layer every front-end reads: its schema, its lease
  lifecycle, its history and its diagnostics.
- [Provenance](https://github.com/shakenfist/teneral/blob/develop/docs/provenance.md)
  -- where each behaviour came from, and why that question matters
  in an Apache-2.0 project that reimplements GPL software.
- [Architecture](https://github.com/shakenfist/teneral/blob/develop/ARCHITECTURE.md)
  -- the intended shape of the system.
- [Development](https://github.com/shakenfist/teneral/blob/develop/docs/development.md)
  -- how to build, lint and test, and what CI runs.
- [Releasing](https://github.com/shakenfist/teneral/blob/develop/docs/releasing.md)
  -- how a release is proposed, tagged, signed and published.
- [Agent guidance](https://github.com/shakenfist/teneral/blob/develop/AGENTS.md)
  -- conventions for working in this repository.

## Licence

Apache-2.0. No code is copied from dnsmasq or from Technitium DNS
Server, both of which are GPL; see
[the provenance record](https://github.com/shakenfist/teneral/blob/develop/docs/provenance.md)
for how that is kept true.
