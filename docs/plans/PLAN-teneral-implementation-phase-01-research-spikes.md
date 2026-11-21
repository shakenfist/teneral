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
   required either way -- it is what replaces Shaken Fist
   reading a lease file -- so 1e is not finished without its
   delivery semantics on reconnect: whether a reconnecting
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
| 1e | xhigh | opus | none | Write the API design note as `docs/api-design.md` in the teneral worktree at `/srv/kasm_profiles/mikal/vscode/src/shakenfist/teneral-wt-p01-spikes`, and resolve the master plan's open question 1. Read `docs/plans/PLAN-teneral-implementation.md` (the Mission, the Open questions and the phase 3/4/9 sketches), `ARCHITECTURE.md` and `AGENTS.md` first. Study Technitium DNS Server's `APIDOCS.md` (<https://github.com/TechnitiumSoftware/DnsServer/blob/master/APIDOCS.md>) for API SHAPE ONLY -- it is GPL-3.0, so copy no text and no code, and say in the note that it was studied as an interface rather than a source; it is a flat HTTP API with bearer-token auth and `status: ok|error|invalid-token` responses, and the interesting part is how it models zones, records and DHCP scopes as one surface. Also read Shaken Fist's `shakenfist/managed_executables/dnsmasq.py` and `shakenfist/network/bridged_vxlan_network.py` in `~/src/shakenfist/shakenfist` to see what the existing consumer actually needs: `DnsMasq.new()` keys on `owner_network._vx_veth_inner`, `provide_dns` defaults to `False`, and the thing to replace is a lease file being read to learn an instance's address. SPECIFY: the resource model over the binding store -- networks, interfaces, client identities, bindings, leases, content blobs -- where a client identity is the union of MAC, client-id, DUID/IAID, user-class and vendor-class, and every resource is scoped by network from the first version because that is the property the whole project exists for; the watch/subscribe primitive, in full, including its delivery semantics on reconnect (snapshot, resume point, or nothing, and what happens to events missed while away -- decision 6 makes this non-optional, because it is what replaces the lease file); how a client learns the address a given instance was assigned; the authentication model; and the transport decision with its reasoning. State explicitly what a browser-based UI needs, because that is the argument against a Unix domain socket: the master plan's default is protobuf over UDS matching `ryll` and Shaken Fist, and the thing that would overturn it is that a browser cannot speak to a UDS, so an HTTP layer appears somewhere regardless. Recommend ONE transport, say what would change the recommendation, and do not hedge -- the operator decides, but an unmade recommendation gives him nothing to decide against. Note also where the API's shape constrains phase 3's storage trait, since open question 5 accepts that a trait designed against one backend usually fits only that backend. |
| 1f | medium | opus | none | Fold the findings back into teneral. Work in `/srv/kasm_profiles/mikal/vscode/src/shakenfist/teneral-wt-p01-spikes`, after 1b through 1e have all reported. Four things, and nothing else -- this step writes no new findings, it only transcribes and corrects. (1) Add a "Findings" section to `docs/plans/PLAN-teneral-implementation-phase-01-research-spikes.md` holding: 1b's result in a few sentences with the GitLab commit or merge request that carries the full write-up, 1c's go/no-go with the per-thread `setns` constraint stated explicitly, 1d's four answers and its go/no-go on Rust, and the transport 1e recommends with one sentence of its reasoning. 1d and 1e will have written their own parts already; your job is the 1b and 1c summaries and making the whole section read as one piece. (2) Add the first rows to `docs/provenance.md`, transcribed from the "What a provenance row would cite" list at the end of 1b's write-up. Every row cites either an RFC section or a named observed-behaviour measurement in `docs/linux-bridge-lab.md`; a row saying "as dnsmasq does" is not acceptable, which is the file's own rule. Expect at least two: the lease file format, and what SIGHUP re-reads. (3) Correct the master plan, `docs/plans/PLAN-teneral-implementation.md`, where phase 1 proved it wrong: update its Situation section if 1b or 1c contradicted a measurement there (the shared blocks are off limits -- they re-sync from shakenfist/development), and set phase 1's `Status` to `Complete` with its `Merged` cell per decision 8, which is the GitLab merge request as `homelab-deployments-lfs <sha> (!NN)` plus the teneral `first..last` range or pull-request merge commit. Also set the plan's row in `docs/plans/index.md` -- it stays `In progress`, because ten phases remain. (4) If 1b or 1c produced work worth doing that this phase declined, add it to the master plan's *Future work* section; silent disappearance is the failure mode the master plan names. Run `pre-commit run --all-files` and fix what it reports. |

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
