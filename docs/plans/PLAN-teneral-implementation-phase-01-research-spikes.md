# Teneral implementation, phase 1: research spikes

Phase 1 of
[PLAN-teneral-implementation.md](PLAN-teneral-implementation.md).

The gating phase. It takes three measurements and answers one
question, before any Rust is written, because two of those
measurements can end the project: 1b is the evidence that one
dnsmasq cannot do what we want, and 1c is the evidence that one
process can.

## Planning effort

**High.** Every step here decides something the rest of the plan
rests on, and three of the five produce evidence rather than
code -- a measurement taken wrongly is worse than one not taken,
because it will be cited for years. The master plan sets high
effort for anything that decides which network a request belongs
to and for the API's shape, which is 1b, 1c and 1e between them.

No review effort is specified for this phase in the master plan.

## Scope

**In scope.**

- A Linux-bridge lab in `~/src/private/homelab-deployments-lfs`,
  deployed on `sfcbr`, with two bridges, two overlapping
  `10.0.0.0/24` networks and four client endpoints.
- The dnsmasq baseline: that it serves one network, how it fails
  on two overlapping ones, and what it re-reads on SIGHUP.
- The `setns` experiment: that one process can hold a socket per
  network namespace and keep serving each after moving away.
- A Rust viability check on `hickory-server`, `dhcproto` and
  `AF_PACKET`, in Docker.
- The API design note, `docs/api-design.md`, resolving the
  master plan's open question 1.
- The first rows of `docs/provenance.md`, from 1b.

**Out of scope, deliberately.**

- Any Rust that is kept. 1d is a throwaway; the Cargo workspace
  is phase 2's job and this phase does not pre-empt it.
- Any teneral source code at all.
- OVS, OVN and anything flow-based. The existing OVS lab's
  unrun experiments 3, 5 and 6 stay unrun: no consumer of
  teneral uses OVS, so they inform nothing here.
- DHCPv6, RA and SLAAC. The lab installs no v6 tooling.
- Creating the `shakenfist/teneral` GitHub repository. See
  decision 8 and the risk below; it is the operator's action.
- Booting a guest. A namespace holding one veth is
  indistinguishable from a VM for DHCP and DNS, which is the
  reasoning the OVS lab already records; PXE and cloud-init
  datasource probing need a different rig and are phase 10's
  problem.

## What the survey found

Taken 2026-11-21, against the trees as they stand. The master
plan's phase 1 section was written the same day, so most of it
holds; six things did not, and they are corrected at source in
the master plan as part of this phase's planning commit.

**Verified, unchanged.**

- `playbooks/ovs-lab-debian-13.yml`, `roles/ovs_lab` (defaults,
  `tasks/{main,install,bridge,endpoints,tooling}.yml`, three
  templates) and `docs/ovs-lab.md` all exist as the master plan
  describes, as does `playbooks/teardown.yml`.
- The crate table is exact. `hickory-proto`, `-resolver` and
  `-server` are all at 0.26.3, published 2026-09-10, `MIT OR
  Apache-2.0`; `dhcproto` is at 0.15.0, published 2026-05-19,
  `MIT`. Neither has moved since the master plan was written.
- Technitium's `APIDOCS.md` is reachable on `master` and is an
  HTTP API with bearer-token authentication, which is the shape
  1e studies.
- `os.setns` and `os.unshare` exist on the workstation's Python
  3.13.5, so they will exist on the lab's Debian 13 Python.
- `sfcbr` answers, `sf-client` works, and `debian:13` is the
  image the OVS lab deployed from on 2026-11-08.
- Shaken Fist's topology is as described:
  `bridged_vxlan_network.py:501-519` creates a veth pair, moves
  the inner end into the per-network netns with `ip link set
  ... netns`, and enslaves the outer end to the bridge with `ip
  link set ... master`. `managed_executables/dnsmasq.py`'s
  `new()` sets `interface = owner_network._vx_veth_inner`, so
  dnsmasq runs against the veth inside that namespace, and
  `provide_dns` defaults to `False`, which is the `port=0` the
  master plan notes.

**Corrected.**

1. **The OVS lab is not deployed.** The master plan's Situation
   says "A lab exists", and names identifier `ovslab-1794115958`.
   That namespace is not in `sf-client namespace list`: the lab
   was torn down. The playbook exists; the deployment does not.
   This matters more than it first looks -- see decision 9.
2. **`playbooks/teardown.yml` is already identifier-generic.**
   The 1a brief says to add a teardown path "if it is not
   identifier-generic already". It is: the playbook takes
   `-e identifier=<ns>` and runs `sf-client namespace clean`
   then `namespace delete`, with no per-deployment branches.
   The conditional resolves to no change, and 1a should not
   invent one.
3. **`isolation: worktree` is wrong for 1a.** The master plan
   sets it, but the sub-agent's worktree isolation cuts a
   worktree of *this* repository, and 1a's work is entirely in
   `homelab-deployments-lfs`. Every step here is `none`, and the
   brief names the working directory and the branch instead.
4. **The homelab repository is GitLab, not GitHub.** Its remote
   is `git@gitlab.home.stillhq.com:private/homelab-deployments-lfs.git`,
   its default branch is `main`, and it lands work through merge
   requests (`05a20e0 Merge branch 'ovs-lab' into 'main'`). The
   `plan-push-audit-phase` block's `<repo> <sha> (#pr)` form
   therefore reads `homelab-deployments-lfs <sha> (!NN)` for
   this phase's landings.
5. **`shakenfist/teneral` does not exist on GitHub, and this
   repository has no remote.** `gh repo view shakenfist/teneral`
   cannot resolve it and `git remote -v` is empty, yet
   `docs/development.md` documents the GitHub settings as though
   they were configured and `PUSH-AUDIT.md` writes every diff
   against `origin/develop`. So: this phase's worktree was cut
   from local `develop` rather than `git fetch origin
   develop:develop`; the teneral half of this phase cannot land
   as a pull request; and the push-audit phase's default
   `AUDIT_RANGE` will not resolve until a remote exists. See
   decision 8.
6. **The homelab "reuse, not duplicate" rule is narrower than
   the 1a brief implies.** `AGENTS.md` there says the rule is
   about copying logic *out of the three public shakenfist
   repos* into the private overlay -- not about two of this
   repository's own roles resembling each other. The brief's
   substantive point (adapt the endpoint helper rather than
   rewrite it) stands on its own merits; its citation does not.
   See decision 4.

**Found, and load-bearing.**

7. **The 1a topology as written cannot produce the 1b(ii)
   measurement.** The brief puts each network's gateway at
   `10.0.0.1/24` "in its own namespace", which is Shaken Fist's
   shape and is what 1c needs. But a dnsmasq inside one of those
   namespaces cannot *see* the other network's interface at all,
   so "how does one dnsmasq fail on two overlapping networks"
   answers itself trivially and proves nothing. The interesting
   failure needs both gateway interfaces visible in one
   namespace. Decision 1 builds the lab so both shapes are
   reachable without a redeploy.
8. **dnsmasq may be able to do this, and 1b must give it the
   chance.** The master plan's justification rests on one
   dnsmasq being unable to serve two overlapping `10.0.0.0/24`
   networks. dnsmasq sets an implicit tag naming the interface a
   request arrived on, and `--dhcp-range` accepts `tag:`
   constraints, so a configuration that disambiguates the two
   ranges by interface tag is at least conceivable. If it works,
   the plan's central claim weakens and has to be restated. 1b
   tries it, and decision 2 makes that a reporting gate rather
   than a footnote.
9. **`hickory-server` 0.26 renamed its central trait.** There is
   no `Authority` any more: the trait is
   `zone_handler::ZoneHandler`, and the implementations 1d needs
   are `store::in_memory::InMemoryZoneHandler` and
   `store::forwarder::ForwardZoneHandler` (with `ForwardConfig`
   and `ForwardZoneHandlerBuilder`), composed in
   `zone_handler::Catalog` and served by `server::Server`.
   `zone_handler::LookupControlFlow` is how a chain of handlers
   decides to answer or fall through, which is exactly the
   "authoritative for local names, forwarding for the rest"
   mechanism 1d(i) is testing. 1d's brief names these, so the
   sub-agent does not go looking for `Authority` and conclude
   the crate cannot do it.
10. **The two crates already share a dependency.** `dhcproto`
    0.15.0 depends on `hickory-proto ^0.26.1`, so the DHCP and
    DNS halves will not drag in two competing DNS wire
    implementations. It also exposes a `v4::borrowed::Message`
    alongside `v4::Message`, which is a zero-copy parse path
    worth knowing about in phase 5.
11. **`hickory-server` ships metrics.** The crate has a
    `metrics` module with a dozen counters
    (`REQUEST_OPERATIONS_TOTAL`, `RESPONSE_CODES_TOTAL`,
    `ZONE_LOOKUPS_TOTAL` and so on) behind an optional
    `metrics` feature. The master plan demands that every change
    say how it would be *detected* in the field; that this comes
    for free on the DNS side is worth recording now, and 1d
    should confirm the feature flag's name.

## Decisions

1. **The lab carries one veth pair per network, and a helper
   moves its inner end between the root namespace and a
   per-network namespace.** This is the answer to survey finding
   7, and it is the decision most likely to be argued with, so
   the reasoning is set out at length.

   Shaken Fist's shape is a gateway veth whose inner end lives
   in a per-network netns holding `10.0.0.1/24`
   (`bridged_vxlan_network.py:501-519`). 1c needs exactly that,
   because the whole experiment is about sockets in namespaces.
   1b needs the opposite: both `10.0.0.1/24` interfaces visible
   to one process in one namespace, because otherwise dnsmasq's
   failure is "the interface does not exist" and the measurement
   is worthless.

   Rather than build two labs or redeploy between experiments,
   the lab creates one veth pair per network -- outer end
   enslaved to that network's bridge, inner end carrying
   `10.0.0.1/24` -- and a helper, `lbr-lab-gateway`, that moves
   the inner ends between the root namespace and `ns-gw1` /
   `ns-gw2` with `ip link set <if> netns <ns>` and back with `ip
   link set <if> netns 1`. One topology, two configurations, one
   command to switch.

   Two interfaces in the root namespace both holding
   `10.0.0.1/24` is a deliberately incoherent routing table.
   That is the point, and it is harmless here because both
   bridges are portless and the management network is
   `10.0.4.0/24`, so nothing the lab host needs is reachable
   through either. The lab documentation says so, because it
   will look like a mistake to the next reader.

   The alternative considered and rejected: two labs, one per
   shape. It doubles provisioning, and it makes 1b and 1c
   non-comparable -- the measurements would be taken against
   different bridges, different MACs and different kernels'
   uptime. Running both against one deployment is what lets
   1c's result be read as a direct answer to 1b's failure.

2. **1b probes the overlap in four configurations and reports a
   dnsmasq success as loudly as a failure.** Survey finding 8 is
   a live possibility and the honest thing is to design for it.
   The four, in order, stopping at the first that works:

   1. Both gateway inners in the root namespace, one dnsmasq,
      two `--interface=` and two `--dhcp-range=` in the same
      subnet, default binding.
   2. The same with `--bind-interfaces`, then with
      `--bind-dynamic` -- the binding mode decides whether
      dnsmasq has per-interface sockets at all, and Neutron
      passes `--bind-dynamic`.
   3. The same with the ranges constrained by interface tag
      (`--dhcp-range=tag:gw-n1,...`), which is the configuration
      that might actually work.
   4. One dnsmasq inside one gateway namespace, recording that
      it cannot see the other network -- the trivial failure,
      written down so nobody later claims configuration 1 was
      the only thing tried.

   For each, record the exact argv, the exact output, and *where
   it broke*: at configuration parsing, at socket binding, in
   the lease table (two networks both wanting `10.0.0.100` in
   one lease file), or on the reply path. A configuration that
   starts and then serves one network's pool to the other is a
   stronger finding than one that refuses, because a silent
   wrong answer is this project's stated failure mode -- so say
   which happened.

   If configuration 3 works end to end, stop and report before
   starting 1c. See the back brief gate.

3. **1c establishes that `setns` is per-thread, and says what
   that means for an async Rust daemon.** The master plan asks
   1c whether sockets keep serving their original namespace
   after the process moves away. They do -- a socket is bound to
   the netns it was created in -- so the experiment is likely to
   succeed, and the valuable part is the constraint it uncovers
   rather than the yes.

   `setns(CLONE_NEWNET)` changes the *calling thread's*
   namespace, not the process's. Python with one thread hides
   that; a Tokio runtime with a multi-threaded scheduler does
   not, and a phase 8 that discovers it late will have built the
   wrong thing. So 1c records explicitly: that the move is
   per-thread, that a socket created in a namespace outlives its
   creator thread's move, and therefore that the workable shape
   is to create every socket on one dedicated thread and hand
   the file descriptors to the runtime. That sentence is the
   most useful thing this step can produce for phase 8.

   It also records the capability set (`CAP_SYS_ADMIN` for
   `setns`, `CAP_NET_RAW` for `AF_PACKET`), because phase 7 has
   to ship a systemd unit and the difference between
   `AmbientCapabilities` and running as root is a packaging
   decision made once.

4. **Two sibling roles, each with its own endpoint helper.** The
   Linux-bridge lab gets `roles/linux_bridge_lab` with
   `lbr-lab-endpoint`, adapted from `ovs-lab-endpoint` rather
   than rewritten: the namespace, veth, MAC-before-up and
   `addr replace` logic carry over unchanged, and the
   `created`/`exists` output contract is kept because the
   Ansible `changed_when` depends on it. What changes is
   everything OVS: `ovs-vsctl port-to-br` becomes a check on
   `/sys/class/net/<if>/master`, `ovs-vsctl add-port ... tag=`
   becomes `ip link set <if> master <bridge>`, the VLAN column
   in `list` becomes a bridge column, and the positional VLAN
   argument becomes a positional bridge argument.

   No shared bash library between the two roles. The shared part
   is about thirty lines; a library sourced across two Ansible
   roles in one repository buys deduplication at the cost of a
   file neither role owns, and survey finding 6 shows the rule
   being cited for this does not actually ask for it. Two
   helpers that each read straight through are the better
   artifact.

5. **1d's spike source lands in this plan file, not in the
   tree.** The write-up quotes the spike's `Cargo.toml`
   dependency lines and its `main.rs` verbatim, in fenced
   blocks, and no Cargo project is committed.

   The argument for committing it is real: the ~100 lines that
   prove `InMemoryZoneHandler` and `ForwardZoneHandler` compose
   in one process is directly reusable in phase 6, and prose
   describing code is a poor substitute. The argument against is
   stronger here. A `spikes/` directory would sit outside the
   workspace phase 2 creates, outside `clippy`, outside
   `cargo deny`, and outside the `unwrap_used` lint that
   `AGENTS.md` carries as an invariant because a panic is an
   outage for every network at once. It would be the one corner
   of the repository exempt from every rule, and it would rot by
   phase 6 anyway as hickory moves. Pasting it into the plan
   gets it into git, under review, with no build obligation and
   no pretence that it is maintained.

6. **1e must specify the watch/subscribe primitive whichever
   transport it recommends.** The transport is 1e's to decide
   and this plan does not pre-empt it. But the primitive is
   required either way -- though not because Shaken Fist reads
   a lease file to learn an address, which 1e established it
   does not do: it assigns the address and parses the lease
   file only to police the result. What is missing is any way
   to ask the daemon what it believes, or to be told when that
   changes. So 1e is not finished without its delivery
   semantics on reconnect: whether a reconnecting
   client is sent a snapshot, a resume point, or nothing, and
   what happens to events that occurred while it was away. A
   note that recommends a transport and leaves that open has
   answered the easy half.

7. **1d and 1e run concurrently with 1b and 1c.** 1a, 1b and 1c
   are strictly sequential: they share one lab. 1d and 1e touch
   no lab, so they run in parallel with it, each as its own
   sub-agent. The exception is that 1d's part (ii) wants a real
   DISCOVER captured in 1b, so 1d starts with (i), (iii) and
   (iv) and takes the capture when 1b produces it. If 1b slips,
   1d records (ii) as pending rather than fabricating a packet.

8. **The teneral half of this phase lands directly on
   `develop`.** Survey finding 5 leaves no pull request to land
   it in: there is no remote. Put to the operator on 2026-11-21
   as a choice between creating the GitHub repository first and
   landing locally, he chose to land locally for now. So the
   teneral commits go on local `develop`, and the master plan's
   `Merged` cell for this phase records a `first..last` range
   for them alongside the GitLab merge request for the homelab
   half. The consequence to accept knowingly: the teneral half
   of this phase gets no review pass and no CI run, so the
   management session's own read of the diff is the only check
   on it. If the repository is created before the phase closes,
   the cell records a merge commit instead and nothing else
   changes.

9. **Every experiment is written up as it is run, not
   afterwards.** The OVS lab's experiments 3, 5 and 6 were
   specified in `docs/ovs-lab.md` on 2026-11-08 and have never
   been run, and survey finding 1 says why: the deployment is
   gone. "Deploy a lab, document the experiments, run them
   later" has already failed once in this exact repository on
   this exact question. So 1b and 1c each commit their output
   into `docs/linux-bridge-lab.md` as they take it, in the same
   session as the deploy, and the definition of done asks for
   the recorded output rather than for a running lab. The lab is
   torn down at the end of the phase with
   `playbooks/teardown.yml`; the playbook and the write-up are
   the deliverables.

## Step plan

All steps are `none` for isolation, per survey finding 3: the
sub-agent's worktree isolation would cut a worktree of teneral,
and three of the five steps work in another repository
altogether. Each brief names its working directory instead.

Ordering: 1a, then 1b, then 1c, sequentially, sharing one lab.
1d and 1e run concurrently with them, per decision 7. 1f is last
and folds everything back into this repository.

| Step | Effort | Model | Isolation | Brief for sub-agent |
|------|--------|-------|-----------|---------------------|
| 1a | high | opus | none | Build the Linux-bridge lab. Work in `~/src/private/homelab-deployments-lfs` (GitLab remote, default branch `main`); create a worktree and a branch `linux-bridge-lab` there yourself -- do not work on `main`. Read `playbooks/ovs-lab-debian-13.yml`, `roles/ovs_lab/**`, `docs/ovs-lab.md` and `AGENTS.md` first; the new work is a sibling of the OVS lab, not a modification of it. Write `playbooks/linux-bridge-lab-debian-13.yml` and `roles/linux_bridge_lab`, following the OVS playbook's five-play structure exactly: preflight with `roles/shakenfist_instance` plus the `wait-for-cloud-init.yml` gate check, provision (`sf_namespace`, `sf_network` registered for its UUID, `sf_instance` with `float=true` and no `user_data` -- both traps are documented in the OVS playbook's comments and both cause an instance rebuild on re-run), the readiness play that includes `{{ shakenfist_actions_dir }}/ansible/tasks/wait-for-cloud-init.yml`, the configure play, and a report play. Host `lbr1`, management address `10.0.4.2` on a `10.0.4.0/24` network, `debian:13`, 2 vCPU / 4096 MB / 30 GB. TOPOLOGY: two **Linux** bridges `br-n1` and `br-n2` (`ip link add name br-n1 type bridge`), both portless as far as the host NIC is concerned -- enslaving the instance NIC cuts the address Ansible is connected over, which is the reason the OVS lab's header gives and it applies unchanged. No VLAN tags, no OpenFlow, no OVS packages. Four client endpoints: `ns-c1a` `10.0.0.11/24` and `ns-c1b` `10.0.0.12/24` on `br-n1`, `ns-c2a` `10.0.0.11/24` and `ns-c2b` `10.0.0.12/24` on `br-n2` -- the overlap is deliberate and is the whole point of having two networks. Plus one gateway veth pair per network: outer `g-n1` / `g-n2` enslaved to the matching bridge, inner `gw-n1` / `gw-n2` each carrying `10.0.0.1/24`. HELPERS, both installed from `templates/` as the OVS role does: (1) `lbr-lab-endpoint add <name> <bridge> <mac> [cidr] \| del <name> \| list \| sh <name> [cmd...]`, adapted from `templates/ovs-lab-endpoint.j2` per decision 4 -- keep the namespace/veth/MAC-before-up/`addr replace` logic and the `created`/`exists`/`deleted`/`absent` output contract verbatim (the role's `changed_when` greps for `created`), replace `port_on_bridge` with a read of `/sys/class/net/<if>/master`, replace `ovs-vsctl add-port ... tag=` with `ip link set <if> master <bridge>`, and make `list` print a BRIDGE column instead of VLAN; (2) `lbr-lab-gateway root \| netns \| show`, which moves `gw-n1` and `gw-n2` between the root namespace and `ns-gw1` / `ns-gw2` with `ip link set <if> netns <ns>` and `ip link set <if> netns 1`, re-asserting `10.0.0.1/24` and `up` after each move because moving an interface between namespaces clears its addresses, and printing which namespace each inner is in for `show`. Both helpers idempotent and both reporting which they did. Leave the lab in the `root` gateway configuration, which is what 1b starts from. PACKAGES: `dnsmasq-base` (NOT `dnsmasq` -- the full package enables a system-wide resolver that fights anything else on `:53`), `isc-dhcp-client`, `busybox` (for `busybox udhcpc`, a second much simpler client), `tcpdump`, `iproute2`, `bridge-utils`, `iputils-ping`, `iputils-arping`, `jq`, `python3-venv`, and a venv at `/opt/lbr-lab/venv` with `scapy` only (no `os-ken`: it is an OVS/OpenFlow library and nothing here speaks OpenFlow). Confirm the guest's `python3 --version` is 3.12 or newer and that `python3 -c 'import os; os.setns'` resolves -- 1c depends on it and finding out now is free. Write `docs/linux-bridge-lab.md` following the shape of `docs/ovs-lab.md`: topology block, a tools table, the experiments 1b and 1c will run (left as specifications with empty result sections for those steps to fill), and a Gotchas section that must include the two-interfaces-with-one-subnet point from decision 1 and the fact that a netns move clears addresses. Do NOT touch `playbooks/teardown.yml`: survey finding 2 establishes it is already identifier-generic. Deploy it (`ansible-playbook playbooks/linux-bridge-lab-debian-13.yml -e "identifier=lbrlab-$(date +%s)" -e "homelab_ci_ssh_key=~/.ssh/id_general"`), then run it a second time and confirm it reports zero changed tasks. Report the identifier and the floating address; do not tear the lab down -- 1b and 1c need it. |
| 1b | high | opus | none | The dnsmasq baseline, in the lab 1a deployed. Work on 1a's branch in the same `homelab-deployments-lfs` worktree, and commit each result into `docs/linux-bridge-lab.md` as you take it, not at the end -- decision 9 explains why, and the lab will not outlive this phase. This is observation of a running program: cite no dnsmasq source line anywhere in the write-up, and say in the document that the method was observation. Record for every command both the exact argv and its verbatim output. (i) BASELINE: with the gateways in the root namespace (`lbr-lab-gateway root`), run one dnsmasq serving `br-n1` only -- roughly `dnsmasq --no-daemon --conf-file=/dev/null --interface=gw-n1 --bind-dynamic --no-hosts --dhcp-range=10.0.0.100,10.0.0.150,12h --log-dhcp` -- and confirm a client gets a lease, with `lbr-lab-endpoint sh c1a ip addr flush dev eth0` first and then both `dhclient -1 -d eth0` and `busybox udhcpc -i eth0 -q -f -n`. Capture the DISCOVER to a pcap with `tcpdump -i br-n1 -w /tmp/discover.pcap port 67 or port 68` and keep the file: 1d needs a real DISCOVER and this is the only place it comes from. Report its path and copy it back to the worktree (NOT committed to the repo -- report the local path to the management session). (ii) THE OVERLAP, which is the measurement that justifies the project. Run the four configurations of decision 2 in order, stopping at the first that works end to end: default binding; `--bind-interfaces` and then `--bind-dynamic`; ranges constrained by interface tag (`--dhcp-range=tag:gw-n1,10.0.0.100,10.0.0.150,12h` and `tag:gw-n2,10.0.0.200,10.0.0.250,12h`, dnsmasq setting an implicit tag naming the arrival interface); and finally one dnsmasq inside one gateway namespace (`lbr-lab-gateway netns`) recording that it cannot see the other network. For each, say WHERE it broke -- configuration parsing, socket binding, the lease table (both networks wanting `10.0.0.100` in one `--dhcp-leasefile`), or the reply path -- and whether it refused or started and answered wrongly. A configuration that starts and serves one network's pool to the other is the more important result, because a silent wrong answer is this project's stated failure mode. **If the interface-tag configuration works end to end, STOP and report to the management session before doing anything else**: it does not kill the project but it does change what the project is for, and that is the operator's call, not yours. (iii) SIGHUP: with one dnsmasq running and a `--dhcp-hostsfile` and a `--dhcp-leasefile`, edit the hostsfile, `kill -HUP`, and record which files were re-read and which were not (expected: hostsfile yes, `--dhcp-range` no); verify with `strace -f -e trace=openat -p <pid>` or by watching the behaviour change, and say which method you used. Then capture the lease file verbatim -- its exact format, field by field -- because phase 9 has to read what Shaken Fist's consumers read. Do not touch teneral's `docs/provenance.md`: 1f writes it. Instead, end the write-up with a short "What a provenance row would cite" list naming each observed behaviour and the heading in `docs/linux-bridge-lab.md` that records it, so 1f can transcribe without re-deriving. |
| 1c | high | opus | none | The gating experiment, in the lab 1a deployed, after 1b. Same branch and worktree; same write-as-you-go rule. Prove that ONE process can serve BOTH overlapping `10.0.0.0/24` networks. Start from `lbr-lab-gateway netns`, so `gw-n1` is in `ns-gw1` and `gw-n2` is in `ns-gw2`, each with `10.0.0.1/24` -- which is Shaken Fist's topology (`bridged_vxlan_network.py:501-519` moves the inner veth into the per-network netns and that is where its dnsmasq runs). Python, not Rust: `os.setns` is in 3.12+, the guest has 3.13, and the semantics under test are the kernel's, so a Rust toolchain would add cost for no information. Write the script to `/opt/lbr-lab/examples/` on the host and into the write-up. THE EXPERIMENT: open an fd on `/var/run/netns/ns-gw1` and `/var/run/netns/ns-gw2` (`os.open(..., os.O_RDONLY)`), `os.setns(fd, os.CLONE_NEWNET)` into each in turn, create and bind a UDP socket on `10.0.0.1:67` in each, `setns()` back to the original namespace (keep an fd on `/proc/self/ns/net` BEFORE moving -- you cannot get back without it), and then prove all sockets still serve their ORIGINAL namespace with the process sitting in the root namespace. That last clause is the actual question; bind succeeding is not the result. Then answer a DHCP DISCOVER from a client in each network -- `lbr-lab-endpoint sh c1a busybox udhcpc -i eth0 -q -f -n` and the same for `c2a` -- from the one process, with both sockets in one `select`/`epoll` loop, and confirm each client gets its own network's answer and not the other's. Hand-build the reply with `scapy` or `struct`; you are not writing a DHCP server, you are proving the socket plumbing, so a canned OFFER with the right `xid` and `yiaddr` is enough. RECORD, each as its own finding: whether `SO_BINDTODEVICE` or `IP_PKTINFO` were needed in addition to the namespace (each namespace holds one interface here, so they may not be -- but teneral will meet namespaces holding several, so say what `IP_PKTINFO` gives you); what happens on the reply path to a client with no address yet, which is expected to need `AF_PACKET` with a hand-built Ethernet/IP/UDP frame (note whether the client's BROADCAST flag lets a 255.255.255.255 reply work instead, and whether an `AF_PACKET` socket is also namespace-bound at creation -- it should be, which is what makes the same trick work for the reply path); and the capability set needed (`CAP_SYS_ADMIN` for `setns`, `CAP_NET_RAW` for `AF_PACKET`), because phase 7 ships a systemd unit and this decides `AmbientCapabilities` versus root. CRITICALLY, and this is the most valuable thing the step produces: establish and state that `setns(CLONE_NEWNET)` moves the **calling thread**, not the process, and that a socket created in a namespace keeps serving it regardless of what its creator thread does afterwards -- then say in one explicit sentence what that means for an async Rust daemon, namely that sockets are created on one dedicated thread and the file descriptors handed to a multi-threaded runtime whose threads' own namespaces are then irrelevant. Demonstrate it: create a socket in `ns-gw1` on a `threading.Thread`, let that thread exit, and show the socket still answers. A NEGATIVE RESULT ENDS THE PROJECT and must be reported as loudly as a positive one -- if sockets do not survive the move, say so immediately and stop. |
| 1d | high | opus | none | The Rust viability check, and the only place the language decision is revisited. Work in the teneral worktree at `/srv/kasm_profiles/mikal/vscode/src/shakenfist/teneral-wt-p01-spikes`, but build the throwaway Cargo project under the session scratchpad, NOT in the repository -- decision 5 explains why nothing Rust is committed. Rust toolchains are deliberately not installed on this host; build in Docker, following the `ryll` and `instar` pattern (read one of their Dockerfiles or Makefiles first). Establish four things and write each up in `docs/plans/PLAN-teneral-implementation-phase-01-research-spikes.md` under a "Findings" heading, quoting the `Cargo.toml` dependency lines and the `main.rs` verbatim in fenced blocks. (i) Can `hickory-server` 0.26.3 serve a small authoritative zone for local names AND forward everything else upstream, in one process, with DNSSEC off? NOTE: 0.26 has no `Authority` trait -- survey finding 9 -- the trait is `zone_handler::ZoneHandler` and the pieces are `store::in_memory::InMemoryZoneHandler`, `store::forwarder::ForwardZoneHandler` with `ForwardConfig` and `ForwardZoneHandlerBuilder`, composed in `zone_handler::Catalog` and served by `server::Server`; `zone_handler::LookupControlFlow` is how a chain decides to answer or fall through, which is the mechanism under test. Prove it with `dig` against the running process: one local name answered authoritatively, one public name forwarded. Record which Cargo features you had to enable, and confirm the name of the optional `metrics` feature that gates the crate's `metrics` module (survey finding 11) -- the master plan demands that changes say how they would be detected in the field, and free DNS counters are worth knowing about. (ii) Can `dhcproto` 0.15.0 parse the real DISCOVER 1b captured and build an OFFER a real client accepts? Use `v4::Message` (and note `v4::borrowed::Message`, a zero-copy path, exists). "A real client accepts" means feeding the OFFER to `busybox udhcpc` or `dhclient`; if the lab is gone by then, say so and settle for a byte-level comparison against the OFFER dnsmasq emitted in 1b, stating the weaker evidence honestly. (iii) Can Rust send a DHCP reply to a client that cannot yet answer ARP, via `AF_PACKET` with a hand-built Ethernet/IP/UDP frame, and what privilege does it need? Compare notes with 1c's answer to the same question; if they disagree, that disagreement is the finding. (iv) Does `cargo deny check` over the resulting dependency tree report any GPL or AGPL crate? You will have to write a `deny.toml` to run it -- write the one phase 2 should adopt, with the GPL/AGPL denial explicit, and quote it in the write-up. ANY OF (i) TO (iii) FAILING IS EVIDENCE TO RECONSIDER RUST and must be reported as such rather than worked around; the master plan's open question 2 names these three as the only things that would overturn the language choice. |
| 1e | xhigh | opus | none | Write the API design note as `docs/api-design.md` in the teneral worktree at `/srv/kasm_profiles/mikal/vscode/src/shakenfist/teneral-wt-p01-spikes`, and resolve the master plan's open question 1. Read `docs/plans/PLAN-teneral-implementation.md` (the Mission, the Open questions and the phase 3/4/9 sketches), `ARCHITECTURE.md` and `AGENTS.md` first. Study Technitium DNS Server's `APIDOCS.md` (<https://github.com/TechnitiumSoftware/DnsServer/blob/master/APIDOCS.md>) for API SHAPE ONLY -- it is GPL-3.0, so copy no text and no code, and say in the note that it was studied as an interface rather than a source; it is a flat HTTP API with bearer-token auth and `status: ok|error|invalid-token` responses, and the interesting part is how it models zones, records and DHCP scopes as one surface. Also read Shaken Fist's `shakenfist/managed_executables/dnsmasq.py` and `shakenfist/network/bridged_vxlan_network.py` in `~/src/shakenfist/shakenfist` to see what the existing consumer actually needs: `DnsMasq.new()` keys on `owner_network._vx_veth_inner`, `provide_dns` defaults to `False`, and note that Shaken Fist assigns the address itself and parses the lease file only to police the result -- what it lacks is a way to ask the daemon what it believes, not a way to discover an address. SPECIFY: the resource model over the binding store -- networks, interfaces, client identities, bindings, leases, content blobs -- where a client identity is the union of MAC, client-id, DUID/IAID, user-class and vendor-class, and every resource is scoped by network from the first version because that is the property the whole project exists for; the watch/subscribe primitive, in full, including its delivery semantics on reconnect (snapshot, resume point, or nothing, and what happens to events missed while away -- decision 6 makes this non-optional, because it is what replaces the lease file); how a client learns the address a given instance was assigned; the authentication model; and the transport decision with its reasoning. State explicitly what a browser-based UI needs, because that is the argument against a Unix domain socket: the master plan's default is protobuf over UDS matching `ryll` and Shaken Fist, and the thing that would overturn it is that a browser cannot speak to a UDS, so an HTTP layer appears somewhere regardless. Recommend ONE transport, say what would change the recommendation, and do not hedge -- the operator decides, but an unmade recommendation gives him nothing to decide against. Note also where the API's shape constrains phase 3's storage trait, since open question 5 accepts that a trait designed against one backend usually fits only that backend. |
| 1f | medium | opus | none | Fold the findings back into teneral. Work in `/srv/kasm_profiles/mikal/vscode/src/shakenfist/teneral-wt-p01-spikes`, after 1b through 1e have all reported. Four things, and nothing else -- this step writes no new findings, it only transcribes and corrects. (1) Add a "Findings" section to `docs/plans/PLAN-teneral-implementation-phase-01-research-spikes.md` holding: 1b's result in a few sentences with the GitLab commit or merge request that carries the full write-up, 1c's go/no-go with the per-thread `setns` constraint stated explicitly, 1d's four answers and its go/no-go on Rust, and the transport 1e recommends with one sentence of its reasoning. 1d and 1e will have written their own parts already; your job is the 1b and 1c summaries and making the whole section read as one piece. (2) Add the first rows to `docs/provenance.md`, transcribed from the "What a provenance row would cite" list at the end of 1b's write-up. Every row cites either an RFC section or a named observed-behaviour measurement in `docs/linux-bridge-lab.md`; a row saying "as dnsmasq does" is not acceptable, which is the file's own rule. Expect at least two: the lease file format, and what SIGHUP re-reads. (3) Correct the master plan, `docs/plans/PLAN-teneral-implementation.md`, where phase 1 proved it wrong: update its Situation section if 1b or 1c contradicted a measurement there (the shared blocks are off limits -- they re-sync from shakenfist/development), and set phase 1's `Status` to `Complete` with its `Merged` cell per decision 8, which is the GitLab merge request as `homelab-deployments-lfs <sha> (!NN)` plus the teneral `first..last` range or pull-request merge commit. Also set the plan's row in `docs/plans/index.md` -- it stays `In progress`, because ten phases remain. (4) If 1b or 1c produced work worth doing that this phase declined, add it to the master plan's *Future work* section; silent disappearance is the failure mode the master plan names. Run `pre-commit run --all-files` and fix what it reports. |

## Findings

The phase's four measurements, in the order they were taken. 1b
and 1c were run in the Linux-bridge lab and written up in
`docs/linux-bridge-lab.md` in `homelab-deployments-lfs`, which
landed as merge request **!12**, merge commit **`658caab`**; the
summaries here are deliberately short, because that document
holds every command and its verbatim output. 1d and 1e ran
against no lab, and their evidence is in this file and in
`docs/api-design.md` respectively.

The headline: the project survives phase 1, with a narrower
justification than it started with. 1c is a clean go, 1d is a go
on Rust, 1e recommends a transport that keeps half of the master
plan's default, and 1b is the one that moved -- what one dnsmasq
cannot do turned out to be smaller, and better evidenced, than
the plan claimed.

### 1b: what one dnsmasq can and cannot do

Two results carry the phase. First, **two untagged
`--dhcp-range` in one subnet are accepted and merged into a
single pool**: a client on the `gw-n2` gateway was offered and
ACKed an address out of `gw-n1`'s range, with both ranges logged
as available for a request that had arrived on `gw-n2`, and with
no warning anywhere -- not on the command line, not in the log.
That is a silent wrong answer, which is this project's stated
failure mode, produced by a configuration an operator would
write without hesitating.

Second, **`tag:` does disambiguate by arrival interface but does
not give each network its own address space**. dnsmasq sets an
implicit tag naming the interface a transaction arrived on,
whether or not the configuration asks for it, and two tagged
ranges served the two networks end to end from their own halves
of the subnet. But with both tagged ranges set to the same
single address, the second network's client was refused with `no
address available` -- on a network whose addresses were entirely
free. Tagging partitions one `/24` between the two networks,
which removes the overlap rather than serving it.

Together these are why the master plan's justification is now
that *one dnsmasq cannot hold two leases of one address for two
networks at once, because its lease table and its lease file
have no network dimension*, rather than that one dnsmasq cannot
serve overlapping networks. Two things the plan previously
implied are false and are now denied in it explicitly: dnsmasq
can tell two same-subnet networks apart when told to, and one
process can hold both networks' sockets -- 1c proved the second
directly. The narrower claim is exactly what a store keyed by
`(network, identity)` fixes.

1b also recorded the lease file field by field and established
that SIGHUP re-reads the `--dhcp-hostsfile` and nothing else --
not the configuration file, and never the lease file, which is
written and rewritten whole but never read back while the
process runs. Those two, with twenty-four other measurements
from 1b and 1c, are now rows in `docs/provenance.md`.

### 1c: one process, two namespaces

**Go, and the gating question is settled.** One process sitting
in the root namespace served both overlapping `10.0.0.0/24`
networks from two sockets, each bound to `10.0.0.1:67` inside
its own gateway namespace, in one `epoll` loop: `10.0.0.101` on
n1 and `10.0.0.202` on n2, both from server identifier
`10.0.0.1`. No socket in the experiment set `SO_REUSEADDR` or
`SO_REUSEPORT`, and the root namespace held no `10.0.0.0/24`
address at all, so neither bind can have quietly happened at
home.

**The constraint phase 8 has to be built around, stated so it
can be acted on:** `setns(CLONE_NEWNET)` moves the calling
*thread*, not the process, and a socket keeps serving the
namespace it was created in for as long as it is open, whatever
its creator thread does afterwards -- including after that
thread has exited. So the shape that works for an async runtime
is to create every socket on one dedicated thread that enters
the right namespace, and to hand the file descriptors to the
runtime, whose worker threads' own namespaces are then
irrelevant; nothing in the runtime needs to know a namespace
exists. Demonstrated rather than reasoned: a worker thread
entered `ns-gw1`, bound a DHCP socket, handed over the
descriptor and exited without ever moving back, two threads of
the one process sat in two namespaces at the same instant, and
the main thread -- which never left the root namespace -- then
served a real client on the dead thread's socket.

The capability set is `CAP_SYS_ADMIN` for `setns`,
`CAP_NET_BIND_SERVICE` for port 67, `CAP_NET_RAW` only if
`AF_PACKET` is used, and nothing else. It was measured as a
six-row matrix at uid 1000, and the two-network exchange then
ran unprivileged holding the first two. So phase 7's systemd
unit wants `AmbientCapabilities`, not root. Opening
`/var/run/netns/<ns>` needs no privilege at all.

The three socket-option questions, each answered:

- **`SO_BINDTODEVICE` was not needed to receive.** It is
  sufficient to reply, but it is per-socket, so it is the wrong
  instrument for a namespace holding more than one interface --
  which is the case teneral will meet.
- **`IP_PKTINFO` was not needed to receive here**, where each
  namespace holds exactly one interface, and *was* needed to
  reply, as a per-message `ipi_ifindex`. On receipt it reports
  the arrival interface index and the datagram's destination
  address, so a broadcast is distinguishable from a unicast
  without parsing the packet, and it is the per-message
  mechanism a multi-interface namespace needs. An interface
  index is only meaningful inside its own namespace.
- **The reply to a client with no address needs the egress
  interface named**, by `IP_PKTINFO` or `SO_BINDTODEVICE`;
  without either, `sendto` to `255.255.255.255` from a namespace
  with one interface and no default route returns `ENETUNREACH`.
  `AF_PACKET` was *not* required: in every one of seven
  successful transactions the client accepted a reply broadcast
  to `255.255.255.255` even though it had cleared the BOOTP
  BROADCAST flag, so a clear flag is not permission to unicast.
  What `AF_PACKET` buys is a *unicast* reply to an address-less
  client, which is deliverable only below the IP layer, and it
  is bound to the namespace it was created in exactly as a UDP
  socket is -- so the same trick works on the reply path. This
  agrees with 1d's independent answer to the same question.

Two incidental findings worth carrying forward: `/sys/class/net`
does not follow `setns` -- it shows the namespace its mount was
made in, while an `ioctl` on a socket follows -- so interface
enumeration must not go through sysfs; and a held file
descriptor keeps a deleted network namespace alive, with its
interfaces, after `ip netns del` has removed the name and the
mount.

### 1d: Rust viability

**Go on Rust.** All three things the master plan's open
question 2 names as able to overturn the language choice were
demonstrated working against real clients and a real upstream
resolver -- `hickory-server` 0.26.3 answered a local name
authoritatively and forwarded a public one from one process,
`dhcproto` 0.15.0 built an OFFER and an ACK that a real
`busybox udhcpc` accepted, and a hand-built Ethernet/IP/UDP
frame reached a client with no address over `AF_PACKET` needing
only `CAP_NET_RAW` -- so nothing here argues against Rust.

| Part | Result | The evidence that settles it |
|------|--------|------------------------------|
| (i) `hickory-server` authoritative and forwarding in one process | **Pass** | `dig host1.teneral.lab` returned `flags: qr aa rd` with the local A record; `dig www.debian.org` through the same socket returned `flags: qr rd ra` (no `aa`) with an upstream answer. |
| (ii) `dhcproto` parses a DISCOVER and builds an accepted OFFER | **Pass**, on a real client's DISCOVER but *not* 1b's lab capture | `udhcpc: lease of 10.0.0.100 obtained from 10.0.0.1, lease time 43200`, then `event=bound ip=10.0.0.100 subnet=255.255.255.0 router=10.0.0.1 dns=10.0.0.1 serverid=10.0.0.1`. |
| (iii) `AF_PACKET` reply to a client that cannot answer ARP | **Pass**; needs `CAP_NET_RAW` and nothing else | As uid 1000 with no capabilities: `socket(AF_PACKET, SOCK_RAW): Operation not permitted (os error 1)`. With `cap_net_raw=ep` as a file capability on the same binary, same uid: `AF_PACKET socket opened, euid=1000`, and the exchange above completed. |
| (iv) `cargo deny check` over the tree | **Pass**, no GPL or AGPL | `advisories ok, bans ok, licenses ok, sources ok`. The whole built tree is `Apache-2.0` (70 crates), `MIT` (82), `Unicode-3.0` (19) and `Zlib` (1). |

The optional feature gating the crate's `metrics` module is
named exactly **`metrics`**, confirming survey finding 11: in
`hickory-server` 0.26.3's `Cargo.toml` it reads
`metrics = ["hickory-resolver?/metrics", "dep:metrics"]`, and
enabling it put `metrics 0.24.6` in the built tree. The
counters are `metrics` crate macros, so they are published
through whichever recorder the daemon installs rather than by
hickory itself -- phase 7 chooses the exporter, not the
counters.

#### How it was built and run

Nothing Rust is installed on the development host. The exact
image was **`rust:1.100.0-trixie`**
(`rust@sha256:223cc32ddca8f3ee90c436dbc94b6b5f052ea7f9653c5201e9612f08ad7d21b8`,
`rustc 1.100.0 (0ef1bc386 2026-11-09)`), the plain official
image and not a derived devcontainer. Four invocations, and
which one did what matters:

1. **Fetch, network up, cache writable.** Resolves and
   downloads only; compiles nothing, so no dependency's
   `build.rs` runs.

   ```
   docker run --rm \
     -v "$D/spike":/workspace -w /workspace \
     -u 1000:1000 -e HOME=/build -e CARGO_HOME=/build/.cargo \
     -v "$D/home":/build \
     -v "$D/cargo-cache/registry":/build/.cargo/registry \
     -v "$D/cargo-cache/git":/build/.cargo/git \
     rust:1.100.0-trixie cargo fetch
   ```

2. **Compile, `--network none`, cache read-only.** Every
   dependency's `build.rs` runs arbitrary code at compile
   time, which is the supply-chain surface; severing the
   network namespace means a compromised build script cannot
   reach out, and the read-only cache means it cannot poison
   the download for later runs.

   ```
   docker run --rm --network none \
     -v "$D/spike":/workspace -w /workspace \
     -u 1000:1000 -e HOME=/build -e CARGO_HOME=/build/.cargo \
     -v "$D/home":/build \
     -v "$D/cargo-cache/registry":/build/.cargo/registry:ro \
     -v "$D/cargo-cache/git":/build/.cargo/git:ro \
     rust:1.100.0-trixie cargo build --offline
   ```

3. **Run the DNS spike, network up.** Compile-time isolation is
   not run-time isolation, and part (i) has to resolve a public
   name through a real upstream resolver, so the container that
   *ran* the server had the network even though the one that
   *built* it did not. The port is published to loopback and
   `dig` runs on the host.

   ```
   docker run --rm -d --name teneral-dns-spike \
     -p 127.0.0.1:15353:15353/udp \
     -v "$D/spike":/workspace -w /workspace -u 1000:1000 \
     -e HOME=/build -v "$D/home":/build \
     rust:1.100.0-trixie ./target/debug/dns_spike 0.0.0.0:15353 1.1.1.1
   ```

4. **Run the DHCP and `AF_PACKET` spike, `--network none`, with
   `NET_ADMIN` and `NET_RAW`.** This one needs no outside
   network at all: it builds its own veth pair inside the
   container's empty network namespace and the client is in
   there with it. The image is the `rust` image above plus
   `busybox iproute2 libcap2-bin tcpdump util-linux` from
   `apt`, built as `teneral-spike-runtime:1d`; nothing Rust is
   compiled in it.

   ```
   docker run --rm --network none \
     --cap-add NET_ADMIN --cap-add NET_RAW \
     -v "$D/spike":/workspace:ro -v "$D/tools":/opt/tools:ro \
     -v "$D/out":/out -w /workspace \
     teneral-spike-runtime:1d /opt/tools/afpacket-experiment.sh
   ```

A gotcha for phase 2's `Makefile`, found the hard way: the
official `rust` image sets `CARGO_HOME=/usr/local/cargo`, so
mounting a cache at `$HOME/.cargo/registry` and setting
`HOME=/build` -- which is what `ryll`'s invocation does,
because its own devcontainer image puts `CARGO_HOME` under
`/build` -- silently downloads into the container layer and
throws it away. `-e CARGO_HOME=/build/.cargo` is what makes the
mount take effect.

The spike is three binaries in one Cargo project rather than
three projects, so that part (iv) audits one dependency tree
containing both the DNS and the DHCP halves.

#### The spike's `Cargo.toml`

```toml
[package]
name = "teneral-spike"
version = "0.0.0"
edition = "2021"
license = "Apache-2.0"
publish = false

[dependencies]
hickory-server = { version = "0.26.3", default-features = false, features = ["resolver", "metrics"] }
hickory-proto = { version = "0.26.3", default-features = false }
hickory-resolver = { version = "0.26.3", default-features = false }
dhcproto = "0.15.0"
libc = "0.2"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "time"] }
```

`default-features = false` throughout, and neither
`dnssec-ring` nor `dnssec-aws-lc-rs` enabled, so `__dnssec` is
off at compile time: DNSSEC is absent rather than merely
switched off, which is what part (i) asked for. The only
features that had to be turned on were **`resolver`** (which is
what `store::forwarder` is gated behind -- without it there is
no `ForwardZoneHandler`) and **`metrics`**. `hickory-proto` and
`hickory-resolver` are direct dependencies only because the
spike names `NameServerConfig` and the record types itself;
`hickory-server` re-exports both as `hickory_server::proto` and
`hickory_server::resolver`, so phase 6 can drop them if it
prefers.

Survey finding 10 held: `dhcproto` 0.15.0's own dependency on
`hickory-proto ^0.26.1` resolved to the same `0.26.3` the DNS
side uses, and `cargo deny`'s `multiple-versions` check
reported no duplicate of it.

#### (i) `hickory-server` 0.26.3: authoritative and forwarding together

`src/bin/dns_spike.rs`:

```rust
//! Spike (i): one process, authoritative for a local zone and
//! forwarding everything else, DNSSEC off.
//!
//! argv: <listen addr:port> <upstream resolver ip>

use std::error::Error;
use std::net::IpAddr;
use std::sync::Arc;

use hickory_proto::rr::rdata::{A, SOA};
use hickory_proto::rr::{LowerName, Name, RData, Record};
use hickory_resolver::config::NameServerConfig;
use hickory_server::Server;
use hickory_server::store::forwarder::{ForwardConfig, ForwardZoneHandler};
use hickory_server::store::in_memory::InMemoryZoneHandler;
use hickory_server::zone_handler::{AxfrPolicy, Catalog, ZoneHandler, ZoneType};
use tokio::net::UdpSocket;

const SERIAL: u32 = 1;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let listen = args.next().ok_or("usage: dns_spike <addr:port> <upstream>")?;
    let upstream: IpAddr = args.next().ok_or("missing upstream")?.parse()?;

    let origin: Name = "teneral.lab.".parse()?;

    // Authoritative half: an in-memory zone with an SOA and one A.
    let mut local: InMemoryZoneHandler =
        InMemoryZoneHandler::empty(origin.clone(), ZoneType::Primary, AxfrPolicy::Deny);
    local.upsert_mut(
        Record::from_rdata(
            origin.clone(),
            300,
            RData::SOA(SOA::new(
                "ns.teneral.lab.".parse()?,
                "hostmaster.teneral.lab.".parse()?,
                SERIAL,
                3600,
                600,
                86400,
                300,
            )),
        ),
        SERIAL,
    );
    local.upsert_mut(
        Record::from_rdata(
            "host1.teneral.lab.".parse()?,
            300,
            RData::A(A::from("10.0.0.100".parse::<std::net::Ipv4Addr>()?)),
        ),
        SERIAL,
    );

    // Forwarding half: everything that is not in the zone above.
    let forward = ForwardZoneHandler::builder_tokio(ForwardConfig {
        name_servers: vec![NameServerConfig::udp(upstream)],
        options: None,
    })
    .with_origin(Name::root())
    .build()
    .map_err(|e| format!("forwarder: {e}"))?;

    // The chain. Catalog::find walks labels off the query name, so
    // the root entry is what a name outside the zone falls through
    // to; LookupControlFlow is what each handler returns to answer
    // or continue.
    let mut catalog = Catalog::new();
    catalog.upsert(
        LowerName::new(&origin),
        vec![Arc::new(local) as Arc<dyn ZoneHandler>],
    );
    catalog.upsert(
        LowerName::new(&Name::root()),
        vec![Arc::new(forward) as Arc<dyn ZoneHandler>],
    );

    let mut server = Server::new(catalog);
    server.register_socket(UdpSocket::bind(&listen).await?);
    println!("dns_spike listening on {listen}, forwarding to {upstream}");
    server.block_until_done().await?;
    Ok(())
}
```

Run as invocation 3 above, with `1.1.1.1` upstream. The process
logged `dns_spike listening on 0.0.0.0:15353, forwarding to
1.1.1.1` and then, from the host:

```
$ dig @127.0.0.1 -p 15353 host1.teneral.lab A +noall +comments +answer
;; ->>HEADER<<- opcode: QUERY, status: NOERROR, id: 17927
;; flags: qr aa rd; QUERY: 1, ANSWER: 1, AUTHORITY: 0, ADDITIONAL: 1
;; WARNING: recursion requested but not available
;; ANSWER SECTION:
host1.teneral.lab.	300	IN	A	10.0.0.100

$ dig @127.0.0.1 -p 15353 www.debian.org A +noall +comments +answer
;; ->>HEADER<<- opcode: QUERY, status: NOERROR, id: 14845
;; flags: qr rd ra; QUERY: 1, ANSWER: 1, AUTHORITY: 0, ADDITIONAL: 1
;; ANSWER SECTION:
www.debian.org.		300	IN	A	103.84.224.41

$ dig @127.0.0.1 -p 15353 teneral.lab SOA +noall +comments +answer
;; ->>HEADER<<- opcode: QUERY, status: NOERROR, id: 21168
;; flags: qr aa rd; QUERY: 1, ANSWER: 1, AUTHORITY: 0, ADDITIONAL: 1
;; ANSWER SECTION:
teneral.lab.		300	IN	SOA	ns.teneral.lab. hostmaster.teneral.lab. 1 3600 600 86400 300

$ dig @127.0.0.1 -p 15353 nope.teneral.lab A +noall +comments
;; ->>HEADER<<- opcode: QUERY, status: NXDOMAIN, id: 2479
;; flags: qr aa rd; QUERY: 1, ANSWER: 0, AUTHORITY: 1, ADDITIONAL: 1
```

Three things in that output are worth more than the pass
itself. The `aa` flag appears on the local name and not on the
forwarded one, so the two halves really are answering as
different kinds of handler rather than one proxying for the
other. `ra` appears only on the forwarded answer, which is
hickory reporting honestly that it does not offer recursion
itself. And the fourth query is the one that matters most for
teneral: a name *inside* the local zone that does not exist
comes back as an authoritative `NXDOMAIN` rather than being
leaked to the forwarder. That is the behaviour a per-network
local zone needs, and it is `LookupControlFlow` doing it -- the
in-memory handler returns a definitive answer and the chain
stops, where on a name outside the zone `Catalog::find` strips
labels until it reaches the root entry and the forwarder gets
it.

Three notes for phase 6. The 0.26 API is as survey finding 9
described and there is no `Authority`; the pieces used here are
`zone_handler::{Catalog, ZoneHandler, ZoneType, AxfrPolicy}`,
`store::in_memory::InMemoryZoneHandler`,
`store::forwarder::{ForwardConfig, ForwardZoneHandler}` and
`server::Server`. Fall-through is positional rather than
negotiated: the forwarder is registered at the root and reached
by `Catalog::find` walking labels off the query name, which
makes one `Catalog` per network the natural shape for
per-network local zones. And `Server::register_socket` takes an
already-bound `tokio::net::UdpSocket`, which is exactly the
hand-off 1c's `setns` constraint needs -- a socket created on a
dedicated thread inside a network namespace can be handed to
the runtime rather than created by it.

#### (ii) `dhcproto` 0.15.0: parse a DISCOVER, build an accepted OFFER

**The input was not 1b's capture.** 1b had not reported a pcap
path when this step ran, and per decision 7 this part was not
to wait or to invent a packet. What it used instead is a
*real* DISCOVER from a *real* client -- `busybox udhcpc`
1.37.0, which identifies itself in the capture as
`ClassIdentifier([117, 100, 104, 99, 112, 32, 49, 46, 51, 55,
46, 48])`, that is the ASCII of `udhcpc 1.37.0` -- captured
with `tcpdump` on the veth pair inside the container. So the
input is neither synthetic nor the lab capture: it is a
different real client on a different link. The stronger test,
against the dnsmasq exchange 1b captures on the lab's bridges,
is still outstanding, and what it would add is a second
client's option set and dnsmasq's own OFFER to compare bytes
against.

`src/bin/dhcp_spike.rs`, the offline half:

```rust
//! Spike (ii), offline half: parse a DISCOVER from a file of raw
//! BOOTP/DHCP bytes and build the OFFER that answers it.
//!
//! argv: <discover.bin> [offer.bin]

use std::error::Error;

use dhcproto::v4::{DhcpOption, Flags, Message, MessageType, Opcode, OptionCode};
use dhcproto::{Decodable, Decoder, Encodable};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let in_path = args.next().ok_or("usage: dhcp_spike <discover.bin> [offer.bin]")?;
    let buf = std::fs::read(&in_path)?;

    // The owned parse path.
    let discover = Message::decode(&mut Decoder::new(&buf))?;
    println!("-- parsed DISCOVER ({} bytes) --", buf.len());
    println!("opcode      {:?}", discover.opcode());
    println!("htype       {:?}", discover.htype());
    println!("xid         0x{:08x}", discover.xid());
    println!("flags       broadcast={}", discover.flags().broadcast());
    println!("chaddr      {}", hex(discover.chaddr()));
    println!("msg type    {:?}", discover.opts().msg_type());
    for (code, opt) in discover.opts().iter() {
        println!("option      {code:?} = {opt:?}");
    }

    // The zero-copy parse path, over the same bytes.
    let borrowed = dhcproto::v4::borrowed::Message::new(&buf)?;
    println!(
        "borrowed    xid=0x{:08x} chaddr={} (zero-copy parse agrees: {})",
        borrowed.xid(),
        hex(borrowed.chaddr()),
        borrowed.xid() == discover.xid() && borrowed.chaddr() == discover.chaddr()
    );

    // Build the OFFER.
    let mut offer = Message::new_with_id(
        discover.xid(),
        [0, 0, 0, 0].into(),
        [10, 0, 0, 100].into(),
        [0, 0, 0, 0].into(),
        discover.giaddr(),
        discover.chaddr(),
    );
    offer.set_opcode(Opcode::BootReply);
    offer.set_htype(discover.htype());
    offer.set_flags(if discover.flags().broadcast() {
        Flags::default().set_broadcast()
    } else {
        Flags::default()
    });
    let opts = offer.opts_mut();
    opts.insert(DhcpOption::MessageType(MessageType::Offer));
    opts.insert(DhcpOption::ServerIdentifier([10, 0, 0, 1].into()));
    opts.insert(DhcpOption::SubnetMask([255, 255, 255, 0].into()));
    opts.insert(DhcpOption::AddressLeaseTime(43200));
    opts.insert(DhcpOption::Router(vec![[10, 0, 0, 1].into()]));
    opts.insert(DhcpOption::DomainNameServer(vec![[10, 0, 0, 1].into()]));
    if let Some(DhcpOption::ClientIdentifier(id)) = discover.opts().get(OptionCode::ClientIdentifier)
    {
        println!("client-id   {} (echoed back is optional, not done)", hex(id));
    }

    let bytes = offer.to_vec()?;
    println!("-- built OFFER ({} bytes) --", bytes.len());
    println!("{}", hex(&bytes));
    if let Some(out) = args.next() {
        std::fs::write(&out, &bytes)?;
        println!("wrote {out}");
    }
    Ok(())
}
```

Fed the 300-byte DISCOVER extracted from the capture:

```
-- parsed DISCOVER (300 bytes) --
opcode      BootRequest
htype       Eth
xid         0x17e85f20
flags       broadcast=false
chaddr      c6de6b0f6950
msg type    Some(Discover)
option      MessageType = MessageType(Discover)
option      ParameterRequestList = ParameterRequestList([SubnetMask, Router, DomainNameServer, Hostname, DomainName, BroadcastAddr, NtpServers])
option      MaxMessageSize = MaxMessageSize(576)
option      ClassIdentifier = ClassIdentifier([117, 100, 104, 99, 112, 32, 49, 46, 51, 55, 46, 48])
option      ClientIdentifier = ClientIdentifier([1, 198, 222, 107, 15, 105, 80])
borrowed    xid=0x17e85f20 chaddr=c6de6b0f6950 (zero-copy parse agrees: true)
client-id   01c6de6b0f6950 (echoed back is optional, not done)
-- built OFFER (274 bytes) --
0201060017e85f2000000000000000000a0000640000000000000000c6de6b0f695000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000638253630104ffffff0003040a00000106040a00000133040000a8c035010236040a000001ff
```

Two things in there matter beyond the pass.
`v4::borrowed::Message`, the zero-copy path survey finding 10
noticed, parsed the same bytes and agreed on `xid` and
`chaddr`, so it is real and usable in phase 5 rather than
merely present. And `ClientIdentifier` came through as
`[1, 198, ...]` -- a type byte of 1 followed by the hardware
address -- which is the shape 1e's client-identity union has to
carry. The spike does not echo it back, because RFC 2131 does
not require it; that is a decision teneral has to make
deliberately rather than inherit.

The acceptance test, which is the part that counts, is in (iii)
below: the same `dhcproto` code path built the OFFER and the
ACK that `udhcpc` accepted. As a cross-check, the OFFER this
offline binary built from the captured DISCOVER was compared
byte for byte with the OFFER the live spike actually put on the
wire, and `cmp -l` reported them identical, both 274 bytes.
That is a consistency check on two call sites, not independent
evidence; the independent evidence is the client's
`event=bound`.

#### (iii) `AF_PACKET` to a client with no address, and the privilege it needs

`src/bin/afpacket_spike.rs`:

```rust
//! Spike (iii), and the live half of (ii): answer a DHCP client that
//! has no address yet, over AF_PACKET, with a hand-built
//! Ethernet/IP/UDP frame carrying a dhcproto-built OFFER and ACK.
//!
//! argv: <ifname> <server ip> <offered ip>

use std::error::Error;
use std::ffi::CString;
use std::io;
use std::mem;
use std::net::Ipv4Addr;

use dhcproto::v4::{DhcpOption, Flags, Message, MessageType, Opcode};
use dhcproto::{Decodable, Decoder, Encodable};

const ETH_P_IP: u16 = 0x0800;
const ETH_HLEN: usize = 14;

fn last_err<T>(what: &str) -> Result<T, Box<dyn Error>> {
    Err(format!("{what}: {}", io::Error::last_os_error()).into())
}

/// The interface's index and MAC, by ioctl on an AF_PACKET socket.
fn if_info(fd: i32, name: &str) -> Result<(i32, [u8; 6]), Box<dyn Error>> {
    let cname = CString::new(name)?;
    let idx = unsafe { libc::if_nametoindex(cname.as_ptr()) };
    if idx == 0 {
        return last_err("if_nametoindex");
    }
    let mut req: libc::ifreq = unsafe { mem::zeroed() };
    let bytes = name.as_bytes();
    for (slot, b) in req.ifr_name.iter_mut().zip(bytes) {
        *slot = *b as libc::c_char;
    }
    if unsafe { libc::ioctl(fd, libc::SIOCGIFHWADDR, &mut req) } < 0 {
        return last_err("SIOCGIFHWADDR");
    }
    let sa = unsafe { req.ifr_ifru.ifru_hwaddr };
    let mut mac = [0u8; 6];
    for (slot, b) in mac.iter_mut().zip(sa.sa_data.iter()) {
        *slot = *b as u8;
    }
    Ok((idx as i32, mac))
}

fn checksum(words: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = words.chunks_exact(2);
    for c in chunks.by_ref() {
        sum += u32::from(u16::from_be_bytes([c[0], c[1]]));
    }
    if let [odd] = chunks.remainder() {
        sum += u32::from(u16::from_be_bytes([*odd, 0]));
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// Ethernet + IPv4 + UDP around a DHCP payload, broadcast to a client
/// that cannot answer ARP yet.
fn frame(src_mac: [u8; 6], src_ip: Ipv4Addr, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(ETH_HLEN + 28 + payload.len());
    f.extend_from_slice(&[0xff; 6]);
    f.extend_from_slice(&src_mac);
    f.extend_from_slice(&ETH_P_IP.to_be_bytes());

    let udp_len = (8 + payload.len()) as u16;
    let total_len = 20 + udp_len;
    let mut ip = Vec::with_capacity(20);
    ip.extend_from_slice(&[0x45, 0x00]);
    ip.extend_from_slice(&total_len.to_be_bytes());
    ip.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 64, 17, 0x00, 0x00]);
    ip.extend_from_slice(&src_ip.octets());
    ip.extend_from_slice(&Ipv4Addr::BROADCAST.octets());
    let ip_csum = checksum(&ip);
    ip[10..12].copy_from_slice(&ip_csum.to_be_bytes());
    f.extend_from_slice(&ip);

    let mut udp = Vec::with_capacity(8 + payload.len());
    udp.extend_from_slice(&67u16.to_be_bytes());
    udp.extend_from_slice(&68u16.to_be_bytes());
    udp.extend_from_slice(&udp_len.to_be_bytes());
    udp.extend_from_slice(&[0x00, 0x00]);
    udp.extend_from_slice(payload);
    let mut pseudo = Vec::with_capacity(12 + udp.len());
    pseudo.extend_from_slice(&src_ip.octets());
    pseudo.extend_from_slice(&Ipv4Addr::BROADCAST.octets());
    pseudo.extend_from_slice(&[0x00, 17]);
    pseudo.extend_from_slice(&udp_len.to_be_bytes());
    pseudo.extend_from_slice(&udp);
    let udp_csum = match checksum(&pseudo) {
        0 => 0xffff,
        c => c,
    };
    udp[6..8].copy_from_slice(&udp_csum.to_be_bytes());
    f.extend_from_slice(&udp);
    f
}

fn reply(req: &Message, kind: MessageType, server: Ipv4Addr, offered: Ipv4Addr) -> Message {
    let mut m = Message::new_with_id(
        req.xid(),
        Ipv4Addr::UNSPECIFIED,
        offered,
        Ipv4Addr::UNSPECIFIED,
        req.giaddr(),
        req.chaddr(),
    );
    m.set_opcode(Opcode::BootReply);
    m.set_htype(req.htype());
    m.set_flags(if req.flags().broadcast() {
        Flags::default().set_broadcast()
    } else {
        Flags::default()
    });
    let opts = m.opts_mut();
    opts.insert(DhcpOption::MessageType(kind));
    opts.insert(DhcpOption::ServerIdentifier(server));
    opts.insert(DhcpOption::SubnetMask([255, 255, 255, 0].into()));
    opts.insert(DhcpOption::AddressLeaseTime(43200));
    opts.insert(DhcpOption::Router(vec![server]));
    opts.insert(DhcpOption::DomainNameServer(vec![server]));
    m
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let ifname = args.next().ok_or("usage: afpacket_spike <if> <server ip> <offered ip>")?;
    let server: Ipv4Addr = args.next().ok_or("missing server ip")?.parse()?;
    let offered: Ipv4Addr = args.next().ok_or("missing offered ip")?.parse()?;

    let fd = unsafe {
        libc::socket(
            libc::AF_PACKET,
            libc::SOCK_RAW,
            i32::from(ETH_P_IP.to_be()),
        )
    };
    if fd < 0 {
        return last_err("socket(AF_PACKET, SOCK_RAW)");
    }
    println!("AF_PACKET socket opened, euid={}", unsafe { libc::geteuid() });

    let (ifindex, mac) = if_info(fd, &ifname)?;
    println!("{ifname}: ifindex={ifindex} mac={mac:02x?}");

    let mut addr: libc::sockaddr_ll = unsafe { mem::zeroed() };
    addr.sll_family = libc::AF_PACKET as u16;
    addr.sll_protocol = ETH_P_IP.to_be();
    addr.sll_ifindex = ifindex;
    let alen = mem::size_of::<libc::sockaddr_ll>() as u32;
    if unsafe { libc::bind(fd, (&addr as *const libc::sockaddr_ll).cast(), alen) } < 0 {
        return last_err("bind");
    }

    let mut buf = [0u8; 2048];
    let mut served = 0;
    while served < 2 {
        let n = unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n < 0 {
            return last_err("recv");
        }
        let f = &buf[..n as usize];
        if f.len() < ETH_HLEN + 28 {
            continue;
        }
        let ihl = usize::from(f[ETH_HLEN] & 0x0f) * 4;
        if f[ETH_HLEN + 9] != 17 {
            continue;
        }
        let udp = ETH_HLEN + ihl;
        if u16::from_be_bytes([f[udp + 2], f[udp + 3]]) != 67 {
            continue;
        }
        let dhcp = &f[udp + 8..];
        let req = match Message::decode(&mut Decoder::new(dhcp)) {
            Ok(m) => m,
            Err(e) => {
                println!("undecodable payload: {e}");
                continue;
            }
        };
        let kind = match req.opts().msg_type() {
            Some(MessageType::Discover) => MessageType::Offer,
            Some(MessageType::Request) => MessageType::Ack,
            other => {
                println!("ignoring {other:?}");
                continue;
            }
        };
        println!(
            "in:  {:?} xid=0x{:08x} chaddr={:02x?}",
            req.opts().msg_type(),
            req.xid(),
            &req.chaddr()[..6]
        );
        let payload = reply(&req, kind, server, offered).to_vec()?;
        let out = frame(mac, server, &payload);
        let sent = unsafe {
            let mut dst: libc::sockaddr_ll = mem::zeroed();
            dst.sll_family = libc::AF_PACKET as u16;
            dst.sll_protocol = ETH_P_IP.to_be();
            dst.sll_ifindex = ifindex;
            dst.sll_halen = 6;
            dst.sll_addr[..6].copy_from_slice(&[0xff; 6]);
            libc::sendto(
                fd,
                out.as_ptr().cast(),
                out.len(),
                0,
                (&dst as *const libc::sockaddr_ll).cast(),
                alen,
            )
        };
        if sent < 0 {
            return last_err("sendto");
        }
        println!("out: {kind:?} {sent} bytes of frame, {} of DHCP", payload.len());
        served += 1;
    }
    unsafe { libc::close(fd) };
    Ok(())
}
```

The experiment runs as invocation 4: one network namespace, a
veth pair, **no IP address on either end**, `busybox udhcpc` on
`veth-cli` and this binary on `veth-srv`. The client therefore
has no address and cannot answer ARP, which is the case under
test, and the reply goes out as a broadcast frame built by
hand.

```
== topology ==
veth-srv@veth-cli UP             fe80::c042:baff:fe8c:ba93/64
veth-cli@veth-srv UP             fe80::c4de:6bff:fe0f:6950/64

== privilege: uid 1000, no capabilities ==
Error: "socket(AF_PACKET, SOCK_RAW): Operation not permitted (os error 1)"
exit=1 (expected: socket(AF_PACKET, SOCK_RAW) refused)

== privilege: uid 1000, file capability cap_net_raw=ep ==
/usr/local/bin/afpacket_spike cap_net_raw=ep
AF_PACKET socket opened, euid=1000
veth-srv: ifindex=3 mac=[c2, 42, ba, 8c, ba, 93]

== real client ==
udhcpc: started, v1.37.0
udhcpc event=deconfig interface=veth-cli ip= subnet= router= dns= lease= serverid=
udhcpc: broadcasting discover
in:  Some(Discover) xid=0x17e85f20 chaddr=[c6, de, 6b, 0f, 69, 50]
out: Offer 316 bytes of frame, 274 of DHCP
udhcpc: broadcasting select for 10.0.0.100, server 10.0.0.1
in:  Some(Request) xid=0x17e85f20 chaddr=[c6, de, 6b, 0f, 69, 50]
out: Ack 316 bytes of frame, 274 of DHCP
udhcpc: lease of 10.0.0.100 obtained from 10.0.0.1, lease time 43200
udhcpc event=bound interface=veth-cli ip=10.0.0.100 subnet=255.255.255.0 router=10.0.0.1 dns=10.0.0.1 serverid=10.0.0.1
udhcpc exit=0
tcpdump: listening on veth-srv, link-type EN10MB (Ethernet), snapshot length 262144 bytes
4 packets captured
```

So Rust can do it, and the privilege answer is clean:
**`CAP_NET_RAW`, and it does not need root.** The same binary,
at the same uid 1000, refused with `EPERM` without the
capability and worked with `cap_net_raw=ep` set on the file --
which is the evidence phase 7 needs to write
`AmbientCapabilities=CAP_NET_RAW` in a systemd unit running as
an unprivileged user rather than `User=root`. `CAP_NET_ADMIN`
was needed in this experiment only to create the veth pair,
which is the lab's job and not teneral's. Note that the
capability has to be in the process's bounding set for a file
capability to take effect, which is the same thing systemd's
`CapabilityBoundingSet=` controls.

**On comparing notes with 1c: I could not.** 1c had not
reported when this ran, so the same-question comparison the
brief asks for is still owed, and the management session should
make it. What this step claims, for 1c to agree or disagree
with: that `AF_PACKET`/`SOCK_RAW` needs `CAP_NET_RAW` and not
root; that a client with the BROADCAST flag *clear* still
accepted a 255.255.255.255 broadcast reply (this `udhcpc` sent
`flags broadcast=false` and took the broadcast answer anyway,
so a correct server should not read that flag as permission to
unicast); and that `SO_BINDTODEVICE` was not needed, because
`bind()`ing the `AF_PACKET` socket to an `sll_ifindex` is
already per-interface and `sendto` with `sll_ifindex` chooses
the egress interface directly. I did not test whether an
`AF_PACKET` socket stays bound to the network namespace it was
created in after the creating thread moves away -- that is
1c's measurement, and it is the one that makes this trick work
for teneral at all.

Two smaller observations for phase 5. The frame is 316 bytes
for 274 of DHCP, so the padding `dhcproto` emits to reach its
`MIN_PACKET_SIZE` already clears the Ethernet minimum and
nothing has to pad further. And the UDP checksum was computed
rather than zeroed; `udhcpc` accepted it, but a zero checksum
is legal for IPv4 UDP and would be one fewer thing to get
wrong, so phase 5 should choose deliberately.

#### (iv) `cargo deny`, and the `deny.toml` phase 2 should adopt

`cargo-deny` 0.22.0, the published release binary
(`cargo-deny-0.22.0-x86_64-unknown-linux-musl.tar.gz`, sha256
verified against the release's own `.sha256` file), mounted
into the container from outside the workspace and run there. It
reads `Cargo.lock` and clones the advisory database; it
compiles nothing, so it ran in the networked invocation.

```
$ cargo-deny check
advisories ok, bans ok, licenses ok, sources ok
```

No GPL and no AGPL crate, and nothing weakly copyleft either.
The built tree is `Apache-2.0` (70 crates), `MIT` (82),
`Unicode-3.0` (19 -- the ICU crates, reached through `idna`)
and `Zlib` (1 -- `tinyvec`); most crates are dual
`MIT OR Apache-2.0`, which is why the first two counts overlap.
`ring` appears in `Cargo.lock` as an optional dependency of the
DNSSEC features but is not in the built graph; its licence is
`Apache-2.0 AND ISC`, both allowed, so turning DNSSEC on later
would not trip this gate.

One correction to the brief, and it is why the file below does
not look as expected: **`cargo-deny` no longer accepts a
`licenses.deny` list.** Under `[licenses] version = 2` the key
was removed as redundant (cargo-deny PR 611), and it is a hard
configuration error rather than a warning:

```
error[deprecated]: this key has been removed, see https://github.com/EmbarkStudios/cargo-deny/pull/611 for migration information
   ┌─ /workspace/deny.toml:50:1
   │
50 │ deny = [
   │ ━━━━
[ERROR] failed to validate configuration file /workspace/deny.toml
```

So the GPL/AGPL denial cannot be written as a `deny` list at
all. It is expressed instead by an allow-list that contains no
copyleft licence, which under `version = 2` makes every
unlisted licence an error -- strictly stronger, since it also
denies the copyleft licences nobody thought to name. What the
file carries explicitly is the *statement* of the rule, as a
comment naming the denied licences and the two places a
copyleft crate could still get in (`allow` and `exceptions`),
both of which are present and empty so that a change to either
appears as a diff on this file rather than as a silent pass.

```toml
# cargo-deny configuration for teneral.
#
# Teneral is Apache-2.0 and may not acquire a GPL, LGPL or AGPL
# dependency. That denial is expressed by the `allow` list below
# and nothing else: under `[licenses] version = 2` cargo-deny
# evaluates an allow-list and treats every unlisted licence as an
# error, and it removed the separate `deny` key as redundant (see
# cargo-deny PR 611, which rejects the key outright rather than
# warning). So the GPL family is denied here by not appearing in
# `allow`, and the two places a copyleft crate could still get in
# are `allow` and `exceptions`. Both are listed explicitly, both
# carry the rule in a comment, and a change to either is a licence
# decision for a human rather than a lint fix.

[graph]
# The daemon is Linux-only. Evaluating other targets drags in
# Windows and macOS crates that are never built, so their licences
# are not ours to carry.
targets = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
]
# False, so the audit describes the tree that is actually shipped.
# An optional feature that would pull in a copyleft crate is
# caught when someone enables it, which is when it becomes true.
all-features = false

[advisories]
version = 2
yanked = "deny"
unmaintained = "workspace"
# Every entry is a known, accepted advisory and carries a comment
# saying why it is accepted and what would remove it.
ignore = []

[licenses]
version = 2
# Permissive licences only. No copyleft licence appears here,
# including the weak ones: adding one is a deliberate decision,
# not a way to make the check pass.
#
# DENIED BY OMISSION, and this is the point of the file:
#   GPL-1.0, GPL-2.0, GPL-3.0 (dnsmasq is GPL-2/3),
#   LGPL-2.0, LGPL-2.1, LGPL-3.0,
#   AGPL-1.0, AGPL-3.0 (Technitium DNS Server is GPL-3.0),
#   SSPL-1.0, and everything else not written below.
allow = [
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "MIT",
    "MIT-0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "Unicode-3.0",
    "CC0-1.0",
    "Unlicense",
]
confidence-threshold = 0.93
# A per-crate exception is a licence decision. Each one needs a
# comment naming who accepted it and on what grounds. A GPL,
# LGPL, AGPL or SSPL crate is never an acceptable exception.
exceptions = []
unused-allowed-license = "allow"

[bans]
multiple-versions = "warn"
wildcards = "deny"
allow-wildcard-paths = true
# One process serves every network, so a crate that panics on
# packet-derived input is a cluster-wide outage. A crate banned
# for that reason is listed here with the reason recorded.
deny = []
skip = []
skip-tree = []

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
allow-git = []
```

Three choices in it are arguable and are therefore stated
rather than left to be discovered. `[graph] all-features =
false` audits the tree that actually ships, which is the
accurate licence question; the cost is that an optional feature
pulling in a copyleft crate is caught when someone enables it
rather than before. `targets` is Linux-only, because
evaluating Windows and macOS targets drags in crates that are
never built and whose licences teneral does not carry.
`multiple-versions = "warn"` rather than `"deny"`, because a
duplicate minor version in a transitive tree is routine and a
hard failure on it would be a standing nuisance -- but
`wildcards = "deny"`, because a wildcard version requirement
makes the audit meaningless.

#### What this step did not establish

- **1b's captured DISCOVER.** Part (ii) used a real
  `busybox udhcpc` 1.37.0 DISCOVER captured in the container,
  not the lab capture, and the dnsmasq-OFFER byte comparison
  the brief offers as the weaker fallback was not possible
  either, because there is no dnsmasq OFFER yet. Both are
  still worth doing once 1b's pcap exists.
- **The 1c comparison on `AF_PACKET`.** Owed, as set out
  above.
- **Namespaces.** Nothing here touched `setns`. The spike ran
  inside a container's own network namespace, which is not the
  same question as one process holding sockets in several.
- **`InMemoryZoneHandler` under mutation.** The spike builds
  the zone once with `upsert_mut` before wrapping it in an
  `Arc`. Teneral's zones change while it is running, and the
  async `upsert` on `&self` is what phase 6 will use; that it
  exists is recorded, that it behaves is not tested.
- **Panic discipline, and a reachable remote panic in
  `dhcproto`.** The spike returns `Result<(), Box<dyn Error>>`
  from `main` and avoids `unwrap`. `dhcproto` does not. 1d
  found that `Message::new_with_id` asserts on a `chaddr`
  longer than 16 bytes; the management session checked that
  and found the reachable defect is worse and sits elsewhere,
  so the sharpened version is recorded here.

  `hlen` is read from the packet unvalidated
  (`v4/mod.rs:477`), `chaddr` is the fixed 16-byte field
  (`:486`), and the accessor is
  `&self.chaddr[..(self.hlen as usize)]` (`:288`). So
  `Message::chaddr()` -- not a constructor, but the accessor
  any DHCP server calls on every packet it parses -- panics on
  any otherwise well-formed DISCOVER whose `hlen` byte exceeds
  16. Measured: a 244-byte DISCOVER with `hlen = 255` decodes
  without error, and then `chaddr()` panics with `range end
  index 255 out of range for slice of length 16`. One
  unauthenticated UDP packet, every network down at once.

  The zero-copy path is worse rather than better.
  `v4::borrowed::Message::chaddr()` is `&self.buffer[28..28 +
  self.hlen() as usize]` (`borrowed.rs:92`) over a buffer
  checked only for a 240-byte minimum, so the outcome depends
  on the packet's length rather than on anything about the
  client: a 240-byte packet panics, and an ordinary 300-byte
  one returns 255 bytes reaching through `sname`, `file` and
  into the options -- bytes the client also chose. Measured:
  bytes 16..24 of the returned "hardware address" were filler
  written into the options region. A store keyed by client
  identity would be reading an identity the client can extend
  at will, which is an identity-forgery primitive rather than
  a crash.

  What phase 5 must do: reject `hlen > 16` at ingress, before
  any accessor runs, and never size a slice from a packet
  byte. The setter is already safe -- `set_chaddr` truncates
  at 16 -- so it is the read path alone. This is the class of
  bug `AGENTS.md` names as a cluster-wide outage, in the crate
  the project had already chosen, found before a line of
  teneral was written. It is the strongest argument this phase
  produced for the panic lints phase 2 installs.
- **`cargo clippy` with `-D warnings`.** Not run; the spike
  was built with `cargo build` only.

### 1e: the control API, and the transport

The note is `docs/api-design.md`. Its recommendation, made
without hedging as the brief asked: **HTTP with JSON bodies,
served on a Unix domain socket, with watch delivered as a
long-lived event-stream response, and no TCP listener in the
daemon at all.**

The reasoning in one sentence: a browser does not need TCP, it
needs HTTP -- an ordinary reverse proxy bridges a Unix socket to
a browser for nothing *if* what is on the socket is HTTP,
whereas a proxy bridging a browser to protobuf has to translate,
and a translating proxy holds a second implementation of
cross-network scoping in the one component nobody would think to
test with two networks.

So the master plan's default half survives: the socket is kept
and only the encoding is discarded, with file permissions still
the gate and no listen address or certificate for the daemon to
own. The operator has not yet ruled on it.

The rest of the note, in brief, since phases 3, 4 and 7 are
written against it:

- **The resource model** is networks, interfaces, client
  identities, bindings, leases and content blobs, every resource
  scoped by network from the first version, with a client
  identity as the union of MAC, client-id, DUID/IAID, user-class
  and vendor-class.
- **The watch primitive** sends a snapshot to a new client, a
  resume to a reconnecting client whose cursor is still held,
  and an explicit in-band `resync` to one whose cursor is not.
  It never silently starts from the present and never silently
  drops events: a reader that cannot keep up is disconnected, so
  that a cursor always means something. A browser's native
  event-source client already implements the client half, cursor
  and all.
- **Authentication** is the peer credential on the socket -- the
  connecting process's user and group, read from the kernel
  rather than claimed -- because there is no configuration file
  in this project and so nowhere to keep a secret. Beyond the
  socket it is bearer tokens, each an id, a write-once secret
  and a scope of networks plus a capability set, with blob
  reading as its own capability because blobs hold credentials.
  Human identity is separate again, and arrives as request
  headers from an authenticating reverse proxy running the OIDC
  flow. The note treats those headers as evidence only because
  the socket's file mode means nothing else can reach the
  listener, from which it follows that the header-trusting
  listener and the token-trusting one cannot be the same
  listener. That half was written after the operator resolved
  open question 3; authorisation stays in the daemon either
  way.
- **What it leaves open**, deliberately: the wire details, IPv6
  resources, multi-host deployment, control-plane rate limits,
  and how an authenticated identity maps to a scope -- the last
  wanting a real authentik configuration rather than paper.

It also found the premise behind the master plan's open question
1 to be wrong, and that correction has been made at source: see
the execution notes below.

## Risks and mitigations

| Risk | Mitigation |
|------|------------|
| dnsmasq turns out to be able to serve overlapping networks by interface tag, weakening the project's justification. | Decision 2 makes it a configuration 1b must try, and a reporting gate rather than a footnote. The management session, not the sub-agent, decides what it means; the operator decides whether the project changes. |
| 1c fails and the project ends. | The step's brief says a negative result is reported as loudly as a positive one and that work stops. The management session reads the script and the output itself rather than the sub-agent's summary, because "it worked" is the answer a sub-agent is most likely to reach for. |
| The lab is torn down with its measurements unrecorded, as the OVS lab's experiments 3, 5 and 6 were. | Decision 9: 1b and 1c commit output as they take it, and the definition of done asks for recorded output rather than a running lab. The management session checks the commits, not the lab. |
| Two interfaces holding `10.0.0.1/24` in the root namespace breaks the lab host's own networking. | Both bridges are portless and the management network is `10.0.4.0/24`, so nothing the lab needs routes through either. 1a's Gotchas section states it so the next reader does not read it as a bug. The failure mode if wrong is losing the Ansible connection, which teardown-and-redeploy recovers. |
| 1d's `hickory` spike flounders on the 0.26 API rename and reports a false negative on Rust. | Survey finding 9 names the real types in the brief. If the step still cannot compose the two zone handlers, the management session reads the compiler errors before accepting a no-go, because "the crate cannot do it" and "I could not find the trait" look identical in a summary. |
| 1e recommends a transport the operator rejects, wasting phases 4 and 7's premise. | 1e recommends rather than decides, and decision 6 makes it specify the watch/subscribe primitive, which both transports need -- so the half of the note that is expensive to write survives either answer. |
| Running experiments on `sfcbr`, which other work depends on. | Everything runs inside a Shaken Fist instance created and destroyed by playbook, in its own namespace. Nothing creates a namespace, bridge or raw socket on the workstation or on `sfcbr` itself. |
| The teneral half cannot be pushed or reviewed, because the repository does not exist. | Decision 8, and it is raised with the operator rather than worked around. It costs this phase review, not correctness; the commits are ordinary and land whenever a remote appears. |

## Definition of done

Each item is checkable by running something. "Documented" is not
on the list.

1. `playbooks/linux-bridge-lab-debian-13.yml` and
   `roles/linux_bridge_lab` exist in
   `~/src/private/homelab-deployments-lfs`, on a branch that is
   not `main`, and `ansible-playbook --syntax-check` passes on
   the playbook.
2. The playbook has been deployed on `sfcbr`, and a second run
   of it reported zero changed tasks.
3. On the deployed host, `lbr-lab-endpoint list` printed four
   client rows, with `10.0.0.11` appearing twice -- once on
   `br-n1` and once on `br-n2`.
4. `lbr-lab-gateway show` printed both gateway inners in the
   root namespace before `lbr-lab-gateway netns`, and in
   `ns-gw1` and `ns-gw2` after it, each still holding
   `10.0.0.1/24`.
5. `docs/linux-bridge-lab.md` in that repository records, for
   each of 1b(i), 1b(ii)'s four configurations, 1b(iii) and 1c,
   the exact command and its verbatim output. No section is a
   specification with an empty result.
6. That write-up cites no dnsmasq source file or line number
   anywhere, and says in the text that its method was
   observation of a running program.
7. 1b(iii) recorded a dnsmasq lease file verbatim, field by
   field, and named which files SIGHUP re-read and which it did
   not.
8. 1c's write-up states, as a sentence a phase 8 reader can act
   on, that `setns(CLONE_NEWNET)` moves the calling thread
   rather than the process, and that a socket outlives its
   creator thread's move -- demonstrated by a socket created on
   a thread that has since exited still answering.
9. 1c's write-up answers, each as its own finding, whether
   `SO_BINDTODEVICE` and `IP_PKTINFO` were needed, what the
   reply path to an address-less client required, and the
   capability set.
10. `docs/provenance.md` in teneral has at least two rows, and
    every row cites either an RFC section or a named measurement
    in `docs/linux-bridge-lab.md`. No row says "as dnsmasq
    does".
11. `docs/api-design.md` exists in teneral, recommends exactly
    one transport, and specifies the watch/subscribe
    primitive's behaviour on reconnect.
12. This plan file has a Findings section stating a go or no-go
    on Rust with 1d's four answers, and 1b's and 1c's results.
13. The master plan's Execution table has phase 1 `Complete`
    with a populated `Merged` cell, and
    `docs/plans/index.md` carries the plan as `In progress`.
14. `pre-commit run --all-files` passes in teneral.
15. The lab has been torn down (`ansible-playbook
    playbooks/teardown.yml -e identifier=<the identifier>`) and
    `sf-client namespace list` no longer shows its namespace.
16. No process this phase started is still running. The lab is
    remote and goes with the instance, but 1d's Docker
    containers and any backgrounded local script are not.

## Back brief

Before executing any step, back brief the operator on your
understanding of this plan and how the work you intend to do
aligns with it.

Two gates, both cheap to ask about and expensive to get wrong:

- **After 1b(ii), if the interface-tag configuration serves both
  overlapping networks.** Stop before 1c and report. It does not
  end the project -- API-driven configuration and one process
  instead of N remain worth having -- but the master plan's
  stated justification is that one dnsmasq *cannot* do this, and
  restating a plan's justification is the operator's decision.
- **After 1c, if sockets do not survive the namespace move.**
  Stop. A no here is a project-ending finding and nothing after
  it is worth doing until the operator has read it.

1d's and 1e's own briefs carry their reporting obligations, and
neither needs a gate: a no-go on Rust and a transport
recommendation are both read at the end of the phase, together,
which is what the master plan's exit condition asks for.

## Execution notes

Kept as the phase runs, for departures from the plan and for the
things a later reader would otherwise have to reconstruct.

**2026-11-21, AEDT.** Back brief answered: the operator approved
creating the namespace, network and instance on `sfcbr`, and
approved pushing the homelab branch and opening its merge request
at the end of the phase. Steps 1a, 1d and 1e ran concurrently per
decision 7; 1b and 1c had not started when these notes were
written, because they share 1a's lab and are sequential behind it.

**A plan premise was wrong, and it was stated in four places.**
1e read the Shaken Fist source and found that it does not learn an
instance's address by reading a lease file: `_enumerate_leases`
builds `allowed_leases[ni.macaddr] = ni.ipv4` from its own
`networkinterfaces` and writes the address into `dhcphosts.tmpl`
with the range in `static` mode, so it *chooses* the address.
`_remove_invalid_leases` reads the lease file to find an unexpired
lease held by an unauthorised MAC and repairs it by killing dnsmasq
and deleting the file, which is reconciliation rather than
discovery. The management session verified each claim against
`shakenfist/managed_executables/dnsmasq.py:288-344,403-409`,
`dhcphosts.tmpl`, `util/concurrency.py:134-149` and the `.proto`
files before accepting it. Corrected at source in the master plan's
open question 1, its Mission paragraph and its 1e brief row, in
`ARCHITECTURE.md`, and in this file's decision 6 and 1e brief row.
The consequence is that a watch primitive alone would not have
replaced what Shaken Fist does; `docs/api-design.md` adds
conflict-reporting writes and a per-network digest for that reason.

**1e's transport recommendation half-overturns the master plan's
default** and is accepted: the Unix socket survives, protobuf on it
does not. The reasoning is not that a browser cannot reach a Unix
socket, which a proxy solves, but that a proxy bridging a browser
to protobuf must translate, and so holds a second implementation of
cross-network scoping in the component least likely to be tested
with two networks. The operator has not yet ruled on it.

**1d's `deny.toml` corrects the brief it was given.** Under
`[licenses] version = 2`, cargo-deny has removed the `deny` key and
rejects a configuration containing it. Verified in the management
session: a config with `deny = ["GPL-3.0"]` fails with "this key
has been removed, see PR 611" and the file does not validate. The
GPL and AGPL denial is therefore an allow-list, which is strictly
stronger. The licence verdict itself was reproduced in the
container rather than taken from the report: `advisories ok, bans
ok, licenses ok, sources ok`.

**The `dhcproto` panic was sharpened rather than accepted as
reported**, and the probes that establish it are
`chaddr_panic.rs` and `chaddr_borrowed.rs`, written by the
management session in the step 1d scratchpad and run in the same
offline container shape as the spike. They are not committed, for
the reason decision 5 gives.

**Not reported upstream.** The `dhcproto` defect is a
security-relevant bug in a third-party crate, and nothing about it
has been sent anywhere. It is recorded here for the operator to
decide, because sending anything to an upstream project is his call
and not this session's.

**1a's convergence was re-run by the management session.** 1a
reported that its second run changed one task, carrying an edit it
had made after the first, and that a third run was clean. Rather
than take the third run's word for it, the playbook was run again
here against `lbrlab-1795244131`: `ok=24 changed=0` on `lbr1` and
`ok=9 changed=0` on `localhost`, zero changed tasks. The gateway
toggle was also exercised in both directions from this session, and
reports `moved` then `unchanged` on a repeat, with the address
re-asserted after each move.

**`shellcheck` was run by the management session.** 1a could not
run it -- the tool is not installed on the development host -- and
reported that honestly, having fallen back to `bash -n` and live
exercise. Both helpers were then checked from this session in a
container (`koalaman/shellcheck:stable`, `--shell=bash`) and are
clean.

**1a's unrequested choices are kept**, all five: fixed gateway MACs
rather than kernel-random ones, the helper's third `configured`
state so that the first run after the veths are created is not
reported as converged, `changed_when: false` on `add_host` so that
a converged run is literally zero changed, the split of veth
creation (role) from the inner end's location, MAC and address
(helper), and `lbr-lab-endpoint list` omitting namespaces with no
`l-<name>` port so the gateway namespaces do not appear in it. Each
makes the lab's own idempotence claim checkable, which is what
decision 9 relies on.

**The decision-2 gate was reached, and the operator restated the
justification.** 1b found that the interface-tag configuration does
work -- but only by partitioning one `/24` between the two networks,
which removes the overlap rather than serving it. Asked to serve the
same address to both, dnsmasq refuses the second network with `no
address available` on a network whose addresses are entirely free.
The management session reproduced both halves directly before
bringing it up: a client on `gw-n2` was offered and ACKed
`10.0.0.101` out of n1's range under default binding, with both
ranges logged as available for a request that arrived on `gw-n2`;
and with tagged single-address ranges, n1 got `10.0.0.50` and n2 was
refused three times. Put to the operator on 2026-11-21, he chose to
restate rather than to pause the phase or leave the wording alone.

The restated claim, now written into the master plan's Situation and
Mission, `ARCHITECTURE.md` and `README.md`: what one dnsmasq cannot
do is hold two leases of one address for two networks at the same
time, because its lease table and its lease file have no network
dimension. Two things the documents previously implied are false and
are now denied explicitly -- dnsmasq can tell two same-subnet
networks apart when told to, and two networks using `10.0.0.0/24`
can share a namespace with one socket serving both. The claim is
narrower than it was and better evidenced, and it is exactly what a
store keyed by `(network, identity)` fixes.

**A measurement 1a took that was not asked for, and that refines
decision 1.** With both gateways in the root namespace, the
overlap costs nothing on the receive path: each network's client
still resolves `10.0.0.1` to its own gateway's MAC, because the
kernel answers ARP on the interface the request arrived on. The
incoherence is in the routing table, as two equally specific
`10.0.0.0/24` routes of which locally-originated traffic takes the
first. So the root configuration is a reply-path problem rather
than a receive-path one, and 1b should expect dnsmasq's failure to
appear there rather than in binding.

**The `Merged` cell is deliberately half-filled.** The master plan
records this phase as merged at `homelab-deployments-lfs 658caab
(!12)` and nothing else, because the teneral half has not landed on
`develop` yet: decision 8 lands it locally, and 1f left its work
uncommitted for the management session to review. The teneral
`first..last` range belongs in that cell and is the management
session's to add once the commits exist. A reader finding only the
GitLab merge request there is looking at a gap that is recorded,
not at an oversight.
