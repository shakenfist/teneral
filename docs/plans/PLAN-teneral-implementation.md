# Teneral: implementation

## Prompt

Before responding to questions or discussion points in this
document, explore this repository thoroughly. Read the relevant
files and ground your answers in what they actually say. Do not
speculate about the repository when you could read it instead.
Flag any uncertainty explicitly rather than guessing.

This repository holds `teneral`: a single daemon that answers a
request from an identified client arriving on a known interface
with content determined by that client's identity. DHCP, DNS,
TFTP and eventually instance metadata are front-ends over one
binding store. It exists to replace the one-dnsmasq-per-network
deployment model, and to be half of a home IPAM.

Consult `AGENTS.md` for the conventions and the invariants that
are not visible in the code, and `ARCHITECTURE.md` for the shape
of the system. `PUSH-AUDIT.md` is the pre-push review runbook
that every plan's final phase runs.

Four things make planning here different from planning in an
ordinary service, and all four should shape any plan written
from this template:

* **A wrong answer is silent.** Nothing here returns an error to
  a user. A duplicated lease, a stale reservation or a name
  answered from the wrong network produces a client that cannot
  reach anything, hours later, with no stack trace. Plans state
  how a change would be *detected* in the field, not only how it
  is tested.
* **Cross-network isolation is the core security property.** The
  daemon deliberately holds every network's bindings in one
  process, which is the whole point and also the whole risk.
  Two tenants may legitimately use the same address. Any change
  that touches lookup, dispatch or the binding store says what
  keeps one network's answers out of another's, and the eventual
  metadata front-end serves credentials, so a cross-network leak
  there is a credential leak.
* **The input is hostile and unauthenticated.** Every byte of a
  DHCP or DNS request arrives from an unauthenticated client on a
  network segment. Rust removes the memory-safety class but not
  the logic class: a parser that trusts a length, a lease table
  that trusts a client identifier, or an option encoder that
  trusts its own arithmetic are all still reachable.
* **No GPL code may be copied in.** dnsmasq is GPL-2/3 and
  Technitium DNS Server is GPL-3; this repository is Apache-2.0.
  Reading either to build a work list is fine. Behaviour that no
  RFC specifies is derived by observing a running dnsmasq, not by
  reading its source, and `docs/provenance.md` records which RFC
  or which observed-behaviour test each behaviour came from. A
  plan that adds behaviour says which it is.

<!-- shared-block: plan-file-conventions v2 -->
Plan file conventions (shared block; do not edit -- the canonical
copy lives in shakenfist/development at
`templates/shared-blocks/plan-file-conventions.md`):

- All planning documents live in `docs/plans/`.
- Detailed planning gets one plan file per phase. Phase files are
  named for their master plan, sit in the same directory as it,
  and append `-phase-NN-descriptive` before the `.md` extension.
- The master plan tracks its phases in a table under its Execution
  section. `Merged` is last, and the push audit is the last row,
  for the reasons given in `plan-push-audit-phase`:

  | Phase | Plan | Status | Merged |
  |-------|------|--------|--------|
  | 1. Schema migration | PLAN-thing-phase-01-schema.md | Not started | |
  | 2. Public API | PLAN-thing-phase-02-api.md | Not started | |
  | 3. Push audit | - | Not started | |

- One commit per logical change, and at minimum one commit per
  phase. Unrelated changes are not batched into a single commit.
  Each commit is self-contained: it builds, passes tests, and has
  a message explaining what changed and why.
<!-- shared-block-end -->

**In this repository.** The fleet default applies unchanged:
plans live in `docs/plans/`, and detailed planning gets one phase
file per phase, named for its master plan. This project is large
enough to want phase files, so it uses them. The plan index is
`docs/plans/index.md`.

## Situation

Nothing has been built. This repository holds its templates, its
documentation skeleton and this plan. What follows are the
measurements taken on 2026-11-21 that the plan rests on; a later
phase that finds one of them wrong should say so here rather than
quietly working around it.

**dnsmasq is large, and almost none of it is used.** The upstream
tree at `10fbc39` is 47,890 lines of C across 48 files, with 286
options documented in `dnsmasq.8` and roughly 120 RFCs referenced
from the source. Where those lines go:

| Subsystem | Lines | Wanted? |
|-----------|-------|---------|
| Plumbing (event loop, netlink, sockets, logging, allocation) | 10,287 | Rewritten, not ported |
| DNS core (forward, rfc1035, cache, domain-match, edns0) | 10,428 | Mostly from `hickory` |
| Config/option parser (`option.c`) | 6,360 | **None** -- there is no config file |
| Optional integrations (dbus, ubus, ipset, nftset, conntrack, lua, auth zones, bpf, tftp) | 6,265 | TFTP only |
| DHCPv4 (rfc2131, dhcp, dhcp-common, lease) | 6,970 | Yes -- the substance |
| DHCPv6 + RA/SLAAC (rfc3315, radv, dhcp6, slaac) | 4,649 | Out of scope for this plan |
| DNSSEC (dnssec, crypto) | 2,931 | Never |

**The consumers use a couple of dozen directives between them.**
Shaken Fist's `managed_executables/dnsmasq.py` emits 14:
`domain-needed`, `bogus-priv`, `no-hosts`, `no-resolv`,
`filterwin2k`, `port=0`, `pid-file`, `interface`,
`listen-address`, `domain`, `local=`, `dhcp-leasefile`,
`dhcp-range=...,static`, `dhcp-option`, `dhcp-hostsfile`. Note
`port=0`: Shaken Fist usually disables DNS entirely. Neutron's
`agent/linux/dhcp.py` emits 23, adding `bind-dynamic`,
`addn-hosts`, `dhcp-optsfile`, `dhcp-match`, `dhcp-userclass`,
`dhcp-option-force`, `dhcp-lease-max`, `dhcp-broadcast`,
`local-service`, `bridge-interface`, `server` and the log flags.
The union is roughly 28 of 286.

**Shaken Fist does not use Open vSwitch at all.**
`shakenfist/network/bridged_vxlan_network.py` builds Linux
bridges, vxlan devices and veth pairs, and moves the inner veth
into a per-network namespace (`ip link set ... netns`,
`ip netns exec`). Any datapath that assumes OVS does not apply to
it.

**Neutron's flow-based DHCP is opt-in, so the problem is live.**
`neutron/conf/agent/agent_extensions_manager.py` declares the
extension list `default=[]`, and the `dhcp` L2 extension is only
an available entry point in `setup.cfg`. The default ML2/OVS
deployment therefore runs `neutron-dhcp-agent`, which runs one
dnsmasq per network per agent, each in its own `qdhcp-<net-id>`
namespace. ML2/OVN is a different driver with its own native DHCP
(`put_dhcp_opts`), so deployments that moved to OVN do not have
this problem -- which means the population that still has it is
precisely the large non-OVN one.

**The Rust ecosystem covers the DNS third and the DHCP wire
format, with licences that fit.** Measured against the crates.io
API on 2026-11-21:

| Crate | Version | Licence | Use |
|-------|---------|---------|-----|
| `hickory-proto` / `-resolver` / `-server` | 0.26.3 (2026-09-10) | MIT OR Apache-2.0 | The DNS front-end |
| `dhcproto` | 0.15.0 (2026-05-19) | MIT | DHCP parse/encode only; no server |
| `dora-dhcp` | 0.1.0 (2022-10-23), 1,537 downloads | MPL-2.0 | Reference only -- stale, and the licence is not ours |
| `toe-beans` | 0.12.0 | LGPL-2.0-only | Rejected on licence |

Hickory's 2026 advisory record is in its DNSSEC validator
(non-IN records skipping verification, NSEC3 closest-encloser
matching on hash alone, an unbounded loop). That is a reason to
leave DNSSEC switched off rather than a reason to avoid hickory,
but it is also the clearest available evidence for why this
project should never write its own validator.

**A lab exists as code, and it is the wrong lab.**
`playbooks/ovs-lab-debian-13.yml` in
`~/src/private/homelab-deployments-lfs` was built for the
adjacent question and deployed on 2026-11-08 (identifier
`ovslab-1794115958`). It settled that flows alone cannot answer
DHCP, that an access port's VLAN tag is invisible in table 0, and
that a pure-flow ARP responder works. The deployment is gone --
that namespace is no longer in `sf-client namespace list`
(checked 2026-11-21) -- and its experiments 3, 5 and 6 are
unrun, which is the same fact twice: a lab that does not outlive
the session that deployed it takes its unrun experiments with it.
But it is built on OVS, and no consumer of teneral uses OVS, so
its endpoint tooling is reusable and its bridge is not.
Experiment 6's unrun second half -- serving two overlapping
`10.0.0.0/24` networks from one dnsmasq and watching it fail --
is the measurement that justifies this entire plan. Phase 1b has
now taken it, in the Linux-bridge lab rather than this one, and
it came out narrower than this section assumed: dnsmasq
distinguishes the two networks perfectly well when told to, and
fails on the lease table instead. See phase 1's Findings.

**The closest prior art is GPL.** Technitium DNS Server is
authoritative and recursive DNS plus a multi-network DHCP server
plus a web console running on its own documented HTTP API, and it
creates a DNS record when a device takes a lease. It is GPL-3.0.
It is the right thing to study for API shape and the wrong thing
to copy from, exactly like dnsmasq.

## Mission and problem statement

Build a single daemon that answers a request from an identified
client arriving on a known interface with content determined by
that client's identity, and that serves many networks from one
process. DHCP, DNS and later TFTP and HTTP metadata are
front-ends over one binding store; the store is keyed by network
from the first commit, which is the thing dnsmasq cannot do and
the reason this is worth building. Measured in phase 1b: one
dnsmasq cannot hold a lease of `10.0.0.50` for one network and a
lease of `10.0.0.50` for another at the same time, because its
lease table and its lease file have no network dimension. It is
not that it cannot tell the two networks apart, and not that one
process cannot hold both sockets; both of those turned out to be
possible. Configuration arrives through
an API rather than a file. The first consumer is a home IPAM
deployment, because it is the smallest path to something useful
and the most demanding test of the API; Shaken Fist and Neutron
come after, because they add namespaces and overlapping address
space on top of a design that should already work without them.

The order is deliberate and is the main thing this plan asserts.
Leading with Shaken Fist would mean building the hardest datapath
first, against a consumer that disables DNS, and discovering the
API's shape from a single programmatic client that writes a
hosts file and parses a lease file to police it. Leading with
home IPAM means DNS is exercised from
the start, the API is driven by a user interface that wants to
read and write everything, and the whole thing runs daily on a
network whose owner will notice when it is wrong.

**This plan does not cover:** DHCPv6, router advertisement or
SLAAC (Neutron needs them, home IPAM mostly does not, and they
are 4,649 lines of dnsmasq's problem); DNSSEC validation, now or
ever; any flow-based or OVS datapath; and dnsmasq
configuration-file compatibility, which was considered and
rejected -- it would have signed this project up for 286
directives and for reproducing an option grammar that is the one
part of dnsmasq where "interface compatibility" is a judgement
call rather than a settled one.

## Open questions

Each carries the decision the plan takes by default if nobody
answers, so that no phase blocks waiting for one.

1. **Control-plane transport: HTTP REST, or protobuf over a Unix
   domain socket?** Phase 1e decides it with the API design note.
   *Default: protobuf over UDS*, matching `ryll` and Shaken Fist,
   on the grounds that the control plane is local-only and needs a
   cheap streaming channel for watch/subscribe. The thing that
   would overturn it was first written here as the home IPAM user
   interface not being able to speak to a UDS, and that is not
   the argument: phase 1e found that a reverse proxy bridges a
   browser to a Unix socket for nothing, so long as what is on
   the socket is HTTP. The real objection is that a proxy
   bridging a browser to protobuf must *translate*, and so holds
   a second implementation of cross-network scoping in the
   component least likely to be tested with two networks. Phase
   1e therefore recommends HTTP with JSON bodies on the Unix
   socket, keeping the socket and discarding the encoding; the
   operator has not yet ruled on it, so this question stays open
   and the default above stands until he does. The encoding
   matters much less than the watch/subscribe primitive, which
   is required either way -- though not for the reason first
   written here. Shaken Fist does not learn an address from
   DHCP: it chooses the address and writes it into a hosts file,
   with the range in `static` mode
   (`managed_executables/dnsmasq.py`, `_enumerate_leases` and
   `dhcphosts.tmpl`). It reads the lease file to *police* that --
   `_remove_invalid_leases` hunts an unexpired lease held by an
   unauthorised MAC and repairs it by killing dnsmasq and
   deleting the file. So what is missing is not discovery but any
   way to ask the server what it believes, to be told when that
   changes, or to correct it without killing it. Phase 1e
   established this by reading the code; see `docs/api-design.md`.

2. **Is it Rust?** *Default: yes*, per the operator's
   instruction, and the plan is written on that basis. Phase 1d
   is the only place this is revisited, and only on evidence:
   `hickory` unable to serve authoritative local names alongside
   forwarding, `dhcproto` unable to build a conformant OFFER, or
   no workable path to sending a frame to a client that cannot
   yet answer ARP. Absent one of those, Rust stands.

3. **Where does the user interface live?** ~~*Default: its own
   repository.*~~ **Resolved 2026-11-21, against the default:**
   the daemon serves a simple web interface itself, built from the
   `sfui` design system vendored into its static assets, alongside
   the REST API, with both behind an authenticating reverse proxy
   doing OIDC -- the arrangement the `private-ci` conductor
   dashboard already runs.

   The default existed to stop the interface acquiring a
   privileged back door into the store. That risk does not go
   away by being in one repository, so the protection is restated
   as a rule instead of a repository boundary: the web interface
   gets no API surface of its own, and every byte it shows arrives
   over the same endpoints, with the same network scoping, as any
   other client's. `docs/api-design.md` carries it, and a handler
   that exists only for the UI is the thing to reject in review.

   **Amended 2026-11-23.** This repository is teneral's core
   libraries, and the home IPAM becomes its own repository once it
   is complete -- a separate product that happens to use teneral
   underneath. The resolution above still holds, with one word
   changed: the binary that serves the web interface is the IPAM's,
   built from these libraries, rather than this repository's. The
   split strengthens the rule rather than reopening it, because a
   user interface in another repository cannot reach the store even
   by accident: it has no access to the crate. The IPAM's name is
   not settled. `tinyipam` was proposed; `skep` was offered as an
   alternative, on the grounds that "tiny" is a promise a project
   with DNS, a web interface, OIDC and packaging will outgrow, and
   that the ecosystem's names are words rather than categories.
   Nothing in phases 3 to 6 depends on the answer.

4. **Does an OVS or flow-based front-end ever get built?**
   *Default: no.* It would serve only Neutron's non-default
   `dhcp` L2 extension, and OVN already occupies that design
   space. Revisit only if a consumer appears that cannot use a
   socket.

5. **One storage backend or two?** *Default: SQLite behind a
   trait*, chosen for the home case, with the trait existing from
   phase 3 so that phase 8 can measure whether Shaken Fist's
   scale needs something else rather than guessing now. The risk
   being accepted is that a trait designed against one backend
   usually fits only that backend; phase 8 should expect to
   change it.

6. **IPv6.** *Default: out of this plan.* It is the obvious
   follow-on master plan and the obvious reason Neutron would
   decline to adopt teneral, so phase 9 should record what
   Neutron would still need.

## Execution

Phase files are written one at a time, as each phase is reached,
rather than up front: phase 1's findings are expected to change
phases 3 and 5, and a phase file written today would be fiction
by the time it was executed.

| Phase | Plan | Status | Merged |
|-------|------|--------|--------|
| 1. Research spikes | [PLAN-teneral-implementation-phase-01-research-spikes.md](PLAN-teneral-implementation-phase-01-research-spikes.md) | Complete | `homelab-deployments-lfs 658caab (!12)`, teneral `fa77a8e..1337e79` |
| 2. Repository foundations | [PLAN-teneral-implementation-phase-02-foundations.md](PLAN-teneral-implementation-phase-02-foundations.md) | Complete | teneral `482edf7..8fa1b4b` |
| 3. The binding store | PLAN-teneral-implementation-phase-03-binding-store.md | Proposed | |
| 4. The control API | PLAN-teneral-implementation-phase-04-control-api.md | Proposed | |
| 5. DHCPv4 front-end | PLAN-teneral-implementation-phase-05-dhcpv4.md | Proposed | |
| 6. DNS front-end | PLAN-teneral-implementation-phase-06-dns.md | Proposed | |
| 7. Home IPAM v1 | PLAN-teneral-implementation-phase-07-home-ipam.md | Proposed | |
| 8. Namespace datapath | PLAN-teneral-implementation-phase-08-netns-datapath.md | Proposed | |
| 9. Shaken Fist integration | PLAN-teneral-implementation-phase-09-shakenfist.md | Proposed | |
| 10. Generic content front-ends | PLAN-teneral-implementation-phase-10-content.md | Proposed | |
| 11. Push audit | - | Proposed | |

Phases 1 and 2 are the only phases scheduled. The operator has
other projects in flight and has said explicitly that this one
should not proceed past the prototype until those are further
along, so phases 3 to 11 are `Proposed` rather than `Not started`.
That is the honest status and it should not be promoted to make
the table look busier.

Both phases landed on `develop` without a pull request, because
there is still no remote, so both `Merged` cells carry a
`first..last` range rather than a pull request number. Neither had
a review pass or a CI run; the management session's read of the
diff was the only check.

### Phase 1: Research spikes

The gating phase. Its job is to take three measurements and
answer one question, and to do so before any Rust is written. If
1b and 1c come out the wrong way, there is no project: 1b is the
evidence that one dnsmasq cannot do what we want, and 1c is the
evidence that one process can. 1b has been run, and the evidence
is the lease table rather than the socket layer -- which narrows
the claim without weakening it, since a lease table with no
network dimension is precisely what this project replaces.

Most of this phase lands in `homelab-deployments-lfs`, not here.
Under the `plan-push-audit-phase` block above, each such landing
records `<repo> <sha> (#pr)` in the `Merged` column and is
audited against that repository's default branch as part of the
pull request that lands it. A phase that landed elsewhere and
recorded nothing is the failure mode to watch for, because this
repository's own diff will look clean.

Nothing in this phase runs on the workstation. Every experiment
runs in a Shaken Fist instance on `sfcbr`, for the reason the
operator gave: creating namespaces, bridges and raw sockets on a
machine we depend on is a way to break it. The instance is
created and torn down by playbook, as every other lab in that
repository is.

| Step | Effort | Model | Isolation | Brief for sub-agent |
|------|--------|-------|-----------|---------------------|
| 1a | high | opus | none | Write `playbooks/linux-bridge-lab-debian-13.yml` and `roles/linux_bridge_lab` in `~/src/private/homelab-deployments-lfs`, as a sibling of the existing `ovs-lab-debian-13.yml` and `roles/ovs_lab`. Read that playbook and `docs/ovs-lab.md` first, and read `AGENTS.md`. (That file's "reuse, not duplicate" rule is about copying logic out of the three public shakenfist repositories into the private overlay, not about two of this repository's own roles resembling each other -- but adapting the endpoint helper that creates `ns-<name>` namespaces with veth pairs, rather than rewriting it, is right on its own merits.) Differences from the OVS lab: two **Linux** bridges rather than one portless OVS bridge, no VLAN tags and no OpenFlow, four endpoints in two groups of two, and the two groups deliberately sharing `10.0.0.0/24` with each group's gateway at `10.0.0.1/24` -- which is Shaken Fist's topology from `bridged_vxlan_network.py`, veth-into-netns and all. Note that a gateway confined to its own namespace cannot produce 1b's measurement, because a dnsmasq in one namespace cannot see the other network at all; the phase file's decision 1 resolves this by moving the gateway between the root namespace and a per-network one with a helper. The bridges stay portless for the reason the OVS lab's header gives: enslaving the instance NIC cuts the address Ansible is connected over. Install `dnsmasq-base` (not `dnsmasq`), `python3`, `tcpdump`, `dhcpcd` or `udhcpc`, and a venv with `scapy`. Write `docs/linux-bridge-lab.md` in that repository stating the topology, the tools and the experiments, following the shape of `docs/ovs-lab.md`. `playbooks/teardown.yml` is already identifier-generic, so it needs no change. |
| 1b | high | opus | none | In the 1a lab, take the baseline measurement that justifies the project, and write it up in that repository's `docs/linux-bridge-lab.md`. Three things, each with the command and its actual output recorded: (i) run one dnsmasq serving one of the two `10.0.0.0/24` networks and confirm a client in that namespace gets a lease; (ii) attempt to serve **both** overlapping networks from that single dnsmasq process and record exactly how it fails -- the error, or the wrong answer, or the interface it refuses to bind; (iii) SIGHUP a running dnsmasq and record which files it re-reads and which it does not (expected: `--dhcp-hostsfile` yes, `--dhcp-range` no) and capture a copy of its lease file format. This is observation of a running program, not source reading: that distinction is the whole provenance story, so the write-up says so and cites no dnsmasq source line. |
| 1c | high | opus | none | The gating experiment, in the 1a lab. Prove that ONE process can serve BOTH overlapping `10.0.0.0/24` networks, by creating one socket per network inside that network's namespace and keeping them all in a single process. Python is the right language here -- `os.setns()` exists in 3.12+, the lab has 3.13, and the kernel semantics under test are language-independent, so Rust would add a toolchain for no information. Specifically: hold an fd per netns, `setns()` to each in turn, create and bind a UDP socket on `10.0.0.1:67` in each, `setns()` back to the original namespace, then prove all sockets still serve their ORIGINAL namespace after the process has moved away -- that last part is the actual question, not whether bind succeeds. Then answer a DHCP DISCOVER from a client in each namespace and confirm each gets its own network's answer. Record whether `SO_BINDTODEVICE` and `IP_PKTINFO` are needed in addition, and what happens on the reply path to a client with no address yet (expect to need `AF_PACKET` and a hand-built frame -- dnsmasq's `send_via_bpf` exists for the BSD form of this problem and the Linux form is `AF_PACKET` in `dhcp.c`). Write the finding and the script into `docs/linux-bridge-lab.md`. A negative result here is a project-ending finding and should be reported as loudly as a positive one. |
| 1d | high | opus | none | The Rust viability check, and the only place the language decision is revisited. In a throwaway Cargo project (Docker, per the operator's preference for not installing Rust toolchains on the host -- follow the `ryll`/`instar` pattern), establish four things and write them up in `docs/plans/PLAN-teneral-implementation-phase-01-research-spikes.md` in THIS repository: (i) can `hickory-server` 0.26.3 serve a small authoritative zone for local names AND forward everything else upstream, in one process, with DNSSEC off; (ii) can `dhcproto` 0.15.0 parse a real DISCOVER captured in 1b and build an OFFER that a real client accepts; (iii) can Rust send a DHCP reply to a client that cannot yet answer ARP, via `AF_PACKET` with a hand-built Ethernet/IP/UDP frame, and what privilege that needs; (iv) does `cargo deny check` over the resulting dependency tree report any GPL or AGPL crate. Any of (i)-(iii) failing is evidence to reconsider Rust and must be reported as such rather than worked around. |
| 1e | high | opus | none | Write the API design note as `docs/api-design.md` in this repository, and resolve open question 1. Read Technitium DNS Server's `APIDOCS.md` for API shape only -- it is GPL-3.0, so this is a study of an interface, and no text or code is copied. Then specify: the resource model over the binding store (networks, interfaces, client identities, bindings, leases, content blobs), the watch/subscribe primitive and its delivery semantics on reconnect, how a client learns the address a given instance was assigned (Shaken Fist assigns it rather than learning it, and parses the lease file only to police the result), the authentication model, and the transport decision with its reasoning. State explicitly what a browser-based UI needs, because that is the argument against UDS. Recommend one transport and say what would change the recommendation. |

Exit condition: 1b and 1c have been run and written up with their
actual output; 1d has a go or no-go on Rust with evidence; 1e
recommends a transport. Phase 2 does not start until the operator
has read the findings, because a no-go on 1c changes the project
rather than the phase.

### Phases 3 to 10

These are sketched rather than planned, because phase 1 was
expected to change them. Each gets its own phase file, written
when it is reached, by `/next-phase`.

Phase 2 was sketched here and is now complete; its own file is the
record of what it shipped and why. In summary: the Cargo
workspace with `unwrap_used = "deny"` inherited by every crate,
the Docker build image and the `Makefile` that is the only way
`cargo` is ever invoked, `clippy.toml`, `deny.toml` and the Rust
`pre-commit` hooks, two-tier CI with the merge-queue gates, a
scheduled supply-chain lane, CodeQL, the
`release-automation-rust` and `review-tracking` templates, and the
differential rig skeleton. Three things it did not do, each for a
stated reason: the GitHub side of either template, which needs a
repository that does not exist; a crate split, which would
prejudge phases 3 and 4; and any dnsmasq run by the rig, because
the lab is the only lawful place dnsmasq runs and phase 5
redeploys it.

- **Phase 3: The binding store.** The layer everything else is a
  projection of: `(network, interface) x client identity -> facts`,
  where identity is the union of MAC, client-id, DUID/IAID,
  user-class and vendor-class, and facts are addresses,
  hostnames, lease state, options and arbitrary content blobs.
  SQLite behind a trait. The lease lifecycle and its durability
  point. Two-network tests from the first commit.
- **Phase 4: The control API.** The transport chosen in 1e, CRUD
  over the store, watch/subscribe, authentication.
- **Phase 5: DHCPv4 front-end.** The socket datapath without
  namespaces. DISCOVER, REQUEST, ACK, NAK, RELEASE, INFORM.
  Static reservations and dynamic pools. Option encoding. The
  `AF_PACKET` reply path from 1d. Differential cases for
  everything wire-visible.
- **Phase 6: DNS front-end.** `hickory`: authoritative for local
  names, forwarding for the rest, forward and PTR records
  registered from DHCP leases, and the equivalents of
  `domain-needed`, `bogus-priv` and `local=`. DNSSEC stays off.
- **Phase 7: Home IPAM v1.** A single binary, a systemd unit,
  packaging, and the first real deployment on the operator's own
  network. The milestone at which this becomes useful rather than
  interesting. Includes the simple web interface, built from
  vendored `sfui`, and the authenticating proxy in front of it;
  see `docs/api-design.md` for the identity model and the rule
  that keeps the interface from acquiring an API of its own.
  Per open question 3 as amended, this phase lands in the IPAM's
  own repository rather than here, and what lands here is whatever
  it needed from the libraries and could not get.
- **Phase 8: Namespace datapath.** `setns` and one process over
  many networks, per 1c. The consolidation payoff, and the phase
  that tests whether phase 3's storage trait survives contact
  with a second backend.
- **Phase 9: Shaken Fist integration.** Teach Shaken Fist to
  configure teneral through the API instead of writing dnsmasq
  config files, or write the shim. Lands in the `shakenfist`
  repository and records its merge there. Should also record what
  Neutron would still need, which is mostly IPv6.
- **Phase 10: Generic content front-ends.** TFTP, then the HTTP
  metadata front-end that was the original motivation: publish a
  content blob against a client identity and serve it to that
  client on that interface. Note that the labs cannot test this
  half -- they boot no guest, so cloud-init datasource probing
  and PXE need a different rig, and this phase has to build one.

<!-- shared-block: plan-status-vocabulary v1 -->
Plan status vocabulary (shared block; do not edit -- the canonical
copy lives in shakenfist/development at
`templates/shared-blocks/plan-status-vocabulary.md`):

A status cell -- in the master plan's own Execution phase table, and
in the row `docs/plans/index.md` carries for the plan -- holds
exactly one of these terms and nothing else:

- `Proposed` -- written down as a concept, not yet scheduled.
- `Not started` -- scheduled, but no work has begun.
- `In progress` -- work has begun and has not finished.
- `Blocked` -- cannot proceed until something outside the plan
  changes. Say what, in the plan.
- `Complete` -- the work is done.
- `Abandoned` -- deliberately dropped without being done.
- `Superseded` -- replaced by another plan, which the plan names.

The term is the whole cell. No dates, no phase arithmetic, no
parenthetical qualifiers, no summary of what happened: a status is
read to decide whether a plan still wants attention, and prose in
that column has repeatedly grown until it could no longer be read
either by a person scanning the table or by tooling. Detail belongs
in the plan file, and a one-line summary belongs in the index's own
Intent column.

Matching is case-insensitive, so `In Progress` is accepted, but the
spelling above is the one to write.
<!-- shared-block-end -->

**In this repository.** The same term is written twice: once in
the master plan's own phase table, and once in the row the plan
carries in `docs/plans/index.md`. The index row is the whole-plan
status, so it only reaches `Complete` once every phase has been
completed, abandoned or superseded. `Proposed` is the right term
for a phase that is written down but deliberately not scheduled,
and this project has several: do not promote one to `Not started`
to make the table look busier.

<!-- shared-block: plan-push-audit-phase v4 -->
Push audit phase (shared block; do not edit -- the canonical
copy lives in shakenfist/development at
`templates/shared-blocks/plan-push-audit-phase.md`):

- Every master plan ends with a phase that runs the repository's
  `PUSH-AUDIT.md` over the whole plan's work. It is the last row of
  the Execution table and it is not optional. The rule binds every
  plan that carries the phase, which is decidable from the plan file
  alone: a plan that is already `Complete`, `Abandoned` or
  `Superseded` and does not carry the phase is not reopened to
  acquire one, and a plan that has the phase runs it even if it
  reaches `Complete` before the phase does.
- That phase audits the accumulated diff of every phase in the plan
  against the default branch, not the diff of the last phase alone.
  Auditing one phase at a time would miss what the phases did to
  each other -- the duplicated helper that only exists once phases
  three and six have both landed, the doc page that phase two made
  wrong and phase five never revisited.
- Once the plan's phases have merged, a diff against the default
  branch is empty and would read as a clean audit. The range is not
  reliably derivable after the fact either: unrelated work lands on
  the default branch between phases, so anything anchored on "since
  the plan file appeared" is far too wide. It has to be recorded. As
  each phase lands, what put it on the default branch goes into the
  plan: the merge commit of its pull request, whose diff against its
  first parent is the whole of what landed, or -- where the phase
  landed directly -- every commit of the phase, or its `first..last`
  range. A single commit is only ever enough when it is a merge
  commit.
- Where the Execution phases are a table, that record is a `Merged`
  column, added last so that a row which omits it still reaches
  `Status`; where they are prose sections it is a `Merged:` line in
  the phase's own section. The `Status` column keeps its single
  vocabulary term and nothing else (see `plan-status-vocabulary`).
  A phase that landed in another repository records `<repo> <sha>
  (#pr)` and is audited against that repository's default branch, as
  part of the pull request that lands it; the plan's own push-audit
  phase cites that audit rather than re-running it.
- Phases that landed before the plan started recording them are
  reconstructed rather than left blank. Recover what you can from
  `gh pr list --state merged` and `git rev-list --first-parent`, and
  say in the plan that the range was reconstructed. Do not trust a
  path-filtered `git log` on its own: it lists the commits that
  touched a path without saying which arrived directly and which
  arrived inside a pull request, and recording a commit that came in
  under a merge audits one commit of that pull request rather than
  the pull request. A reconstructed record may be a summary table in
  the audit phase's own section rather than a column or a line in
  the Execution table, which keeps retrospective archaeology out of
  a table that tracks live status. Where a phase accreted over
  months of unrelated commits and no range is recoverable, say that
  instead and name the paths the audit read -- an audit that says
  what it could not scope is a result; one that silently audits
  nothing is not.
- Findings land as their own pull request against the default
  branch, and the plan is not complete until they are resolved or
  explicitly declined in writing. A finding that is declined says
  why, in the plan, where the next reader will find it.
- Where the audit finds nothing, record that in the plan in one
  sentence. It is a real result, and a run of them is the evidence
  for making the phase conditional rather than mandatory.
- A repository with no `PUSH-AUDIT.md` still carries the phase, and
  the phase says that the runbook does not exist yet and what was
  done instead. Silently omitting it is what let the audit go
  untriggered for as long as it did. What is done instead is at
  least the checklist below, applied to the recorded ranges: it is
  a floor, not a substitute for a runbook, and a plan adds the
  questions its own repository's blast radius raises. Each item
  gets a verdict, or a sentence saying why it does not apply.
  Do not borrow another repository's runbook instead: a runbook is
  written for one repository's blast radius.
  1. Every recorded landing takes one of the forms above: a merge
     commit with two parents, whose diff is read against its first
     parent; every commit of a direct landing, or its `first..last`
     range; a landing in another repository, whose own audit is
     cited; or a statement that no range is recoverable, naming the
     paths the audit read.
  2. Anything the diff added that runs unattended -- cron, a timer,
     a scheduled workflow, a self-update -- reports its own failure
     somewhere a human sees, or is named as not doing so.
  3. No secret or token entered any repository, and no private
     hostname or IP address entered a repository more public than
     the one it came from.
  4. Anything the diff removed or renamed has no consumer left,
     checked by grep across every repository that could consume it
     rather than by assumption, with the greps stated.
  5. Where the repository publishes something -- an image, a
     release, a package -- a broken change fails closed: one that
     publishes nothing is recoverable, one that publishes something
     wrong is not.
<!-- shared-block-end -->

**In this repository.** `PUSH-AUDIT.md` exists at the repository
root and is referenced from `AGENTS.md`, so the final phase runs
it rather than explaining its absence. Note that every diff
command in it is written against `origin/develop...HEAD` via
`AUDIT_RANGE`: a stale local `develop` silently widens the audit to
unrelated history, so `git fetch origin` before starting.

As of 2026-11-21 there is no `origin`. `shakenfist/teneral` does
not exist on GitHub and this clone has no remote, although
`docs/development.md` documents the GitHub settings as though it
did. Until the repository is created, a phase here cannot land as
a pull request, its `Merged` cell records a `first..last` range
on `develop`, and `AUDIT_RANGE` has to be set explicitly because
its default will not resolve.

One wrinkle is specific to this project. Some phases land in
**other** repositories -- the research spikes add a playbook to
`homelab-deployments-lfs`, and the Shaken Fist integration
changes `shakenfist`. The shared block above covers it: such a
phase records `<repo> <sha> (#pr)` and is audited against that
repository's own default branch, as part of the pull request that
lands it, and this plan's push-audit phase cites that audit
rather than re-running it. `homelab-deployments-lfs` is on the
private GitLab rather than GitHub, with `main` as its default
branch, so for it that record reads `homelab-deployments-lfs
<sha> (!NN)` and names a merge request. A phase that landed
elsewhere and recorded nothing is the failure mode to watch for
here, because this repository's own diff will look clean.

<!-- shared-block: plan-phase-landing v1 -->
Phase landing (shared block; do not edit -- the canonical copy
lives in shakenfist/development at
`templates/shared-blocks/plan-phase-landing.md`):

A plan's status and a repository's review state both live in files
that every branch would otherwise rewrite. Left alone, that turns
each of them into a merge-conflict hot spot, and it spends a pull
request and a full CI run on a change that is entirely prose.
Three rules keep them out of the way.

- **A phase is closed out in the first commit of the next phase,
  not in a pull request of its own.** By the time the next phase
  branches, the previous one has merged, so its merge commit is
  known and its `Merged` cell can record the thing the push-audit
  phase actually needs. This is the only ordering that works: a
  phase cannot record its own merge commit, and a separate
  close-out pull request buys that record at the price of a round
  trip. The close-out sets the finished phase's `Status` and
  `Merged` cells and the plan's row in `docs/plans/index.md`, and
  it is committed before the next phase's own work, so that the
  branch never claims the plan is further along than the default
  branch is.

- **The last phase closes itself out.** The push-audit phase is
  the last row of every plan, so no next phase will carry its
  close-out. Where the audit raises findings, the plan is not
  complete until they are resolved or declined, and those land as
  their own pull request after the audit phase has merged -- so
  that pull request is the carrier, and it can record the audit
  phase's merge commit, which by then is known. Where the audit
  finds nothing there is no carrier, and no follow-up pull
  request is opened for the sake of one cell: the phase sets its
  own `Status`, and the plan's index row, to `Complete` in its
  own pull request, and records no `Merged` cell. It is the only
  row permitted to omit one. The column exists so that the
  push-audit phase can reconstruct what to audit; the audit phase
  is last, so nothing ever reads its own row.

- **`REVIEWS.md` is not pruned or regenerated in a pull request
  that changes code or documentation.** Editing a reviewed file
  stales its mark, and adding or removing an in-scope file moves
  the header count, but neither is the landing pull request's
  business. `prune` regenerates the file whether or not it dropped
  anything, so the `prune-reviews` workflow heals both on the next
  push to the default branch. Pruning from a branch is also wrong
  more often than it is right, though not for the reason it first
  appears: `prune` compares each stamp against `HEAD`, which on a
  branch is the branch tip, so it drops the marks for the files the
  pull request itself touched while keeping marks the default
  branch has already pruned. Committing that state merges a review
  file computed from a stale tree, and can resurrect marks
  `prune-reviews` has already removed. Accumulated staleness is
  reported by the `review-coverage` audit, which recomputes
  coverage against `HEAD` and raises an issue once the backlog is
  worth a review session.

  **A review session is the exception**, and it is not optional
  tidiness: `stamp` regenerates `REVIEWS.md` as well as writing the
  marks, and the rows, the sidecars and the marks are committed
  together (see `docs/code-review-tracking.md`). Where a repository
  requires a pull request to reach its default branch, that is how
  a review session lands, so "not in a pull request" is about the
  kind of change, not the mechanism.

These rules assume phases land one after another. Where two phase
branches are open at once, each closes out only the phase it
directly follows.
<!-- shared-block-end -->

**In this repository.** The plan index is `docs/plans/index.md`.
Review tracking is not set up yet, so the `REVIEWS.md` rule above
is inert until it is; the phase that adopts the `review-tracking`
template from shakenfist/development is what makes it live.

## Agent guidance

### Execution model

<!-- shared-block: subagent-execution-model v1 -->
Sub-agent execution model (shared block; do not edit -- the
canonical copy lives in shakenfist/development at
`templates/shared-blocks/subagent-execution-model.md`):

All implementation work is done by sub-agents, never in the
management session. The management session is reserved for
planning, review, and decision-making. This keeps the management
context lean and avoids drowning it in implementation diffs.

The workflow is:

1. **Plan** at high effort in the management session.
2. **Spawn a sub-agent** for each implementation step with the
   brief from the plan, at the recommended effort level and model.
3. **Review** the sub-agent's output in the management session.
   Check the actual files -- the sub-agent's summary describes
   what it intended, not necessarily what it did.
4. **Fix or retry** if the output is wrong. Diagnose whether the
   brief was insufficient (improve it) or the model was too light
   (upgrade it), then re-run.
5. **Commit** once the management session is satisfied.

This applies to all steps, including high-effort ones. If a
sub-agent cannot succeed even with a detailed brief and the right
model, that is a signal the brief needs improving, not that the
management session should do the implementation itself.

Use `isolation: "worktree"` for sub-agents when the change is
risky or experimental; the worktree is discarded if the output is
unsatisfactory. For safe, well-understood changes, sub-agents can
work directly in the main tree.
<!-- shared-block-end -->

### Planning effort

<!-- shared-block: plan-planning-effort v1 -->
Planning effort (shared block; do not edit -- the canonical copy
lives in shakenfist/development at
`templates/shared-blocks/plan-planning-effort.md`):

The master plan itself is always created at **high effort** -- it
requires broad codebase understanding, cross-referencing several
source files, and judgment calls about scope and sequencing.

Each phase plan states the recommended effort level for planning
that phase. Phases that turn on design decisions, cross-component
coordination, protocol changes, or subtle correctness questions
should be planned at high effort. Phases that are mechanical, or
that follow a pattern already established elsewhere in the
codebase, can be planned at medium effort.
<!-- shared-block-end -->

**In this repository.** High effort is anything that touches the
binding store's schema or its lookup path, anything that parses
or emits bytes from the network, anything that decides which
network a request belongs to, and the API's shape. Those are the
places where a defect is silent, cross-network or both. Medium
effort covers adding a DHCP option that follows the shape of an
existing one, packaging, a documentation sweep, or adopting a
template from shakenfist/development with a worked example
already in the fleet.

### Step-level guidance

<!-- shared-block: subagent-step-guidance v1 -->
Sub-agent step guidance (shared block; do not edit -- the
canonical copy lives in shakenfist/development at
`templates/shared-blocks/subagent-step-guidance.md`):

Each phase plan includes a table like this:

| Step | Effort | Model | Isolation | Brief for sub-agent |
|------|--------|-------|-----------|---------------------|
| 1a | medium | sonnet | none | One-sentence summary of what to do and which files to touch |
| 1b | high | opus | worktree | Why this needs high effort: requires understanding X to do Y |

**Effort levels**, from cheapest to most thorough:

- **low** -- Purely mechanical changes: rename, reformat, add a
  log line, regenerate generated code. The brief is a complete
  instruction.
- **medium** -- The plan provides enough context to follow a clear
  brief. The sub-agent may read a few files, but the approach is
  already decided.
- **high** -- Requires reading several files, making judgment
  calls, or understanding non-obvious invariants. The sub-agent
  needs to think about edge cases.
- **xhigh** -- The setting for hard coding and agentic steps:
  long-horizon changes, or steps where the sub-agent must both
  research and implement.
- **max** -- Correctness matters more than cost. Expect
  diminishing returns and occasional overthinking; reserve it for
  steps where a wrong answer would be expensive to detect.

**Brief for sub-agent:** this is the key field. Write it as if
briefing a colleague who has never seen the codebase. Include what
to change, which files to touch, what patterns to follow, and any
non-obvious constraints.

A good brief front-loads the research the planner already did, so
the implementing agent does not repeat it. Instead of "add storage
functions for the new object", name the functions to add, the file
they belong in, the existing equivalent to mirror (with line
numbers), and any registration the change also needs.

The better the brief, the lower the effort level needed and the
lighter the model that can succeed.
<!-- shared-block-end -->

**In this repository.** A worked brief: instead of "add support
for DHCP option 121", write "add a `ClasslessStaticRoute` variant
to the option enum in `src/dhcp/options.rs`, encoding it per
RFC 3442 section 2 (destination descriptor: one prefix-length
byte, then only the significant octets of the destination, then
four octets of router), mirroring the existing
`SubnetMask` variant's `encode`/`decode` pair and its round-trip
property test in the same file, register it in the option
registry's match arm, and add a case to the differential rig in
`tests/differential/` asserting a real dnsmasq emits the same
bytes for the same configured route." A brief that names the RFC
section and the existing variant to mirror is the difference
between sonnet succeeding and opus being needed.

Two project-specific demands on every brief:

- **Name the provenance.** A brief that adds protocol behaviour
  says which RFC and section it comes from, or which
  observed-behaviour test in the differential rig pins it. "Match
  what dnsmasq does" is not an acceptable brief, because the only
  lawful way to find that out is to run it and record the result.
- **Name the network scope.** A brief that touches lookup or the
  binding store says whether the code it adds is per-network or
  global, because the two are indistinguishable in a single-network
  test and that is exactly how a cross-network bug ships.

### Model choice

<!-- shared-block: subagent-model-roster v1 -->
Sub-agent model roster (shared block; do not edit -- the canonical
copy lives in shakenfist/development at
`templates/shared-blocks/subagent-model-roster.md`):

The planner recommends which model is best suited to each step.
This is a judgment call, not a rigid rule -- the right model
depends on what the step requires, not on whether it is "planning"
or "implementation". The models available to sub-agents are:

- **fable** -- The most capable model available, for the hardest
  reasoning and the longest-horizon work: multi-step changes a
  single sub-agent must carry end to end, or steps whose
  correctness depends on holding a whole subsystem in mind at
  once. It costs materially more than opus, so reserve it for
  steps that have already defeated opus or are expected to.
- **opus** -- The default for steps needing deep reasoning,
  architectural understanding, subtle correctness judgment
  (locking, state machines, migrations), or intricate
  implementation that would be costly to debug if it were wrong.
- **sonnet** -- A good default for well-briefed implementation
  work. Faster and cheaper than opus, and effective when the plan
  front-loads the research and the brief leaves no broad judgment
  calls to make.
- **haiku** -- Suitable for purely mechanical tasks:
  search-and-replace, regenerating generated code, adding log
  lines, running commands. The brief must be a near-complete
  instruction.

Model choice interacts with effort level and brief quality. A
detailed brief compensates for a lighter model -- sonnet at medium
effort with a thorough brief often matches opus at medium effort
with a vague brief. The planner's job is to write briefs good
enough that the recommended model can succeed.

The model also determines the context window: fable, opus and
sonnet have 1M tokens, haiku has 200K. A step that must hold many
files in context at once may need one of the larger-context models
for that reason alone, even when the reasoning itself is
straightforward.

**When in doubt, skew to the more capable model.** Saving money
only matters if the outcome is still acceptable. A failed or
low-quality implementation wastes more time -- and therefore more
money -- than the heavier model would have cost. Recommend a
lighter model only when you are confident the brief is detailed
enough for it to succeed.
<!-- shared-block-end -->

**In this repository.** The project-specific checks referred to
above are:

- [ ] `pre-commit run --all-files` passes, and `ci.yml` runs the
      same command on every pull request.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets --
      -D warnings` and `cargo test --all` are clean. Clippy
      warnings are errors here; a genuine exception carries an
      `#[allow]` with a comment saying why.
- [ ] `cargo deny check` passes. It is the licence gate as well as
      the advisory gate: a transitively GPL or AGPL dependency is
      a licence defect in an Apache-2.0 project, not a style
      preference.
- [ ] Where the change alters bytes on the wire, the differential
      rig was run against a real dnsmasq and the plan records the
      result. A protocol change with no differential case is
      incomplete.
- [ ] Where the change touches the binding store or lookup, there
      is a test with **two** networks holding the **same** address,
      asserting each gets its own answer. One network proves
      nothing about isolation.

### Management session review checklist

<!-- shared-block: plan-review-checklist v1 -->
Management session review checklist (shared block; do not edit --
the canonical copy lives in shakenfist/development at
`templates/shared-blocks/plan-review-checklist.md`):

After a sub-agent completes, the management session verifies:

- [ ] The files that were supposed to change actually changed --
      read them, do not trust the summary.
- [ ] No unrelated files were modified.
- [ ] The changes match the intent of the brief: not merely
      syntactically correct, but semantically right.
- [ ] The project's own pre-merge checks pass, including any
      generated code that has to be regenerated and committed
      (see the project-specific checks below).
- [ ] The commit message follows project conventions, including
      the `Co-Authored-By` line recording model, context window,
      and effort level.
<!-- shared-block-end -->

## Administration and logistics

### Success criteria

We will know when this plan has been successfully implemented
because the following statements will be true:

* `pre-commit run --all-files` passes, and CI runs the same
  command on every pull request.
* `make lint`, `make test` and `make deny` are all clean. Every
  `cargo` invocation goes through the `Makefile` into the build
  image; nothing Rust runs on the host.
* No dependency, direct or transitive, carries a GPL or AGPL
  licence, and `cargo deny` is what proves it rather than
  inspection.
* Every behaviour the daemon implements is traceable in
  `docs/provenance.md` to an RFC section or to a named
  observed-behaviour test. No entry says "as dnsmasq does" without
  one of the two.
* Every change to lookup or to the binding store carries a test
  with two networks holding the same address.
* Where the change alters bytes on the wire, the differential rig
  carries a case comparing those bytes with a dnsmasq exchange
  captured in the lab, and the case names the provenance rows it
  supports.
* Documentation in `docs/` describes any user-visible change.
  `AGENTS.md` changes only if a convention changed;
  `ARCHITECTURE.md` only if the shape of the system changed;
  `README.md` only if the pitch, the install story or the
  documentation links changed.

### Documentation index maintenance

When creating a new master plan from this template, add one row to
the table in `docs/plans/index.md`: the date the plan was written,
a link to it, a one-line intent, and its status from the
vocabulary above. Rows run oldest first. One row per master plan,
never one per phase -- the phases are tracked in the plan's own
Execution table, and duplicating them in the index is how the two
drift apart.

The index row carries the whole-plan status, so it only reaches
`Complete` once every phase has been completed, abandoned or
superseded. Update it as the plan progresses, not only at the end.

Once a plan has been finished for 90 days it is retired as
shakenfist/development's `docs/plan-retirement.md` describes: its
reasoning is moved into docs/ and its *Future work* is filed as
issues. Write both so they survive being read by someone who was
not here.

<!-- shared-block: plan-closeout-sections v1 -->
Plan close-out sections (shared block; do not edit -- the
canonical copy lives in shakenfist/development at
`templates/shared-blocks/plan-closeout-sections.md`):

### Future work

We should list obvious extensions, known issues, unrelated bugs we
encountered, and anything else we should one day do but have
chosen to defer to here, so that we do not forget them.

* **Nobody has measured dnsmasq's DNS side.** Phase 1 measured
  DHCP only, so `docs/provenance.md` has no DNS rows at all: the
  hosts-file quirks, what `--local`, `domain-needed` and
  `bogus-priv` actually do to a query, which DNS files a SIGHUP
  re-reads, and the shape of a reply for a name the server does
  not hold are all unmeasured. Phase 6 needs them, and the
  cheapest time to take them is in a redeployed Linux-bridge
  lab rather than from a crate's behaviour after the fact.
* **A network namespace holding more than one interface was
  never tested.** Phase 1c found `SO_BINDTODEVICE` and
  `IP_PKTINFO` unnecessary on the receive path, but only because
  each gateway namespace in the lab held exactly one interface.
  The case teneral will actually meet is a namespace with
  several, where the arrival interface has to come from
  `IP_PKTINFO` and the reply has to name its egress per message.
  Phase 8 should run that configuration before it trusts the
  per-socket answer.
* **The differential rig needs a lab, and one now exists.**
  `playbooks/linux-bridge-lab-debian-13.yml` in
  `homelab-deployments-lfs` deploys two overlapping networks,
  four clients and a real dnsmasq from one command, and it
  converges. Phase 5's differential testing should redeploy that
  rather than invent a rig of its own, and the measurements it
  takes should land in `docs/provenance.md` as the phase 1 ones
  did.
* **The `dhcproto` defect is fixed in an upstream pull request,
  and that fix is not released.** `Message::chaddr()` panics on
  a DISCOVER whose `hlen` exceeds 16, and the zero-copy path
  over-reads into attacker-chosen bytes instead. The patch
  staged under `upstream/dhcproto/` was filed as
  bluecatengineering/dhcproto#111 on 2026-11-22, with minor
  edits; the drafted issue was not filed, because that project
  has not used its issue tracker since 2023. Phase 5 must still
  reject `hlen > 16` at ingress, because #111 is unreviewed,
  unmerged and unpublished, and 0.15.0 remains the only
  release.
* **Renovate cannot see the Rust toolchain tag, so the base image
  will never be bumped automatically.** The tag lives once, in the
  `Makefile`, as `RUST_IMAGE` passed to a default-less `ARG` in
  the `Dockerfile` -- which is the single place it was meant to
  live, but Renovate's dockerfile manager reads `FROM` lines and
  cannot see a version arriving as a build argument. Fixing it
  means a `customManagers` rule in `renovate.json` matching the
  `Makefile` assignment, with `datasource: docker` and
  `depName: rust`. Until then the toolchain only moves when
  somebody edits that line by hand, and nothing says so.
* **The GitHub side of the build and review arrangement is not
  set up, and cannot be until the repository exists.** Seven
  steps, in order, with their exact commands and the reason each
  detail matters: see *GitHub-side adoption* in the execution
  notes of
  [the foundations phase file](PLAN-teneral-implementation-phase-02-foundations.md).
  They cover the `develop` ruleset, the `review-marks` environment
  and its branch policy, the review-marks App and key, the merge
  queue, and the `release` environment and tag ruleset. None of
  them is ticked, worked around or disabled. Whoever creates
  `shakenfist/teneral` does them; `docs/development.md` summarises
  what is waiting for a reader who never sees this plan.
* **Teneral is not registered in the fleet's own records.** Two
  changes to shakenfist/development that this phase could not make
  from here: a row for teneral in the "Projects using these
  templates" table in `templates/review-tracking/README.md`, with
  its default branch and the identity its prune job pushes as; and
  bringing teneral into scope for the weekly consistency audit,
  which is what would notice a drifted template copy here in
  future. Both are cross-repository work and so land in that
  repository, not this one.
* **ryll's CodeQL workflow is the drifted copy, not teneral's.**
  Establishing that teneral's bytes were right turned up that ryll
  carries the same workflow with floating `@v4` action tags where
  the fleet template pins by commit. Worth a one-line fix in ryll,
  and worth noticing that nothing reported it.
* **A release ships one unpackaged binary.** The release workflow
  builds `make release` and uploads the binary; there is no `.deb`,
  no `.rpm`, no tarball and no registry, because teneral has
  nothing worth installing yet and choosing a package format in a
  workflow is a decision made by the wrong person. Phase 7 is where
  packaging is actually decided, and it should revisit the release
  artefact at the same time.

### Bugs fixed during this work

This section should list any bugs we encounter during development
that we fixed. You should also scan the project's issue tracker,
where one exists, for directly related issues that we should
either resolve as part of this master plan or at least be aware of
while planning it.

...

### Back brief

Before executing any step of this plan, please back brief the
operator as to your understanding of the plan and how the work you
intend to do aligns with that plan.
<!-- shared-block-end -->
