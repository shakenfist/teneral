# Provenance

Teneral is Apache-2.0. dnsmasq is GPL-2/3 and Technitium DNS
Server is GPL-3.0, so no code may be copied from either. Reading
them to work out *what* to build is fine; this file is how the
project stays able to say where each behaviour came from, years
later, without relying on anyone's memory.

Every behaviour visible on the wire of a protocol teneral did not
invent gets a row. The control API's own wire is teneral's invention
end to end, with no RFC to cite and nothing to measure, so it has no
rows: a row citing nothing would make this rule meaningless. The rule
is one of two sources, and the row says which:

- **An RFC**, cited to the section. The protocols are specified in
  public documents -- dnsmasq's own source references roughly 120
  RFCs -- so nearly every algorithm has a legitimate independent
  source, and that source is the one to implement from.
- **An observed-behaviour test**, named, in the differential rig.
  Where behaviour is not specified anywhere -- dnsmasq's
  tag-matching precedence, its `dhcp-range` mode keywords, its
  hosts-file quirks, its lease-file format -- the lawful way to
  learn it is to run dnsmasq and record what it does. The test
  that records it is the citation.

A row that says "as dnsmasq does" with neither an RFC nor a test
is not acceptable, and `PUSH-AUDIT.md` greps for protocol changes
that arrive without a row here.

Rows are added by the phase that adds the behaviour, and a row may
be written before the behaviour it describes is built. The rows
below were all taken before any code existed, in a lab that was
deliberately not kept: a measurement recorded only in a running
lab is a measurement that disappears with it.

All but one of the rows below are observed behaviour rather than
an RFC citation, measured on 2026-11-21 in the Linux-bridge lab whose
write-up is `docs/linux-bridge-lab.md` in the private
`homelab-deployments-lfs` repository -- dnsmasq 2.91 on Debian 13,
kernel `6.12.117+deb13-amd64`, CPython 3.13.5. The `Citation`
column names that document's experiment and the heading within it
that holds the command and its verbatim output. None of these rows
is a reading of anybody's source, and none of them should be
restated as "what dnsmasq does" without the measurement behind it.

The exceptions are `selector-client-id-over-chaddr` and
`lease-address-selection-order`, which are this file's RFC-cited
rows and both arrived with the binding store rather than with a
front-end. That order was expected: the sentence here used to say
RFC rows would arrive with the first front-end implementing a
specified algorithm, and it was wrong because the store's lookup is
where client identification is decided, and the store's allocation
is where an address is chosen, several phases before anything
parses a packet. An RFC row is owed by whichever layer implements
the specified rule, which is not always the layer that puts bytes
on a wire.

Both of those rows cite a rule the RFC specifies and stop there.
Where a specification settles part of a question and leaves the
rest open, the row says what it covers and the open part is
documented as this project's own in `api-design.md` rather than
being given a row that stretches the citation over it. A row that
claimed more than its section says would be worse than no row,
because the whole value of this file is that a citation can be
checked.

The `Row id` column is a stable name for the row, and it exists so
that something other than a human can check the citation. A case in
the differential rig names the rows it supports by these
identifiers, and the rig's harness fails a case whose identifier is
not in the column below -- so a row cannot be deleted, nor an
identifier mistyped, without a test going red. See
[the differential rig](differential-rig.md).

The identifiers are deliberately not numbers, because a number
shifts when a row is inserted above it. Rewording a behaviour costs
nothing, and should not: the identifier is what the cases cite.
Renaming one means updating every case that cites it, so rename
only when an identifier has become actively misleading, and in the
same commit as the cases.

| Row id | Behaviour | Source | Citation |
|--------|-----------|--------|-----------|
| `lease-file-format` | **The lease file format**: one line per lease, five space-separated fields -- expiry as Unix seconds, lower-case colon-separated MAC, dotted-quad address, client hostname or `*`, client identifier as colon-separated hex or `*` -- newline-terminated, no header, no quoting, and no field naming a network. | Observed, dnsmasq 2.91 | `linux-bridge-lab.md`, experiment 4 -> *The lease file, field by field* |
| `lease-expiry-absolute` | **A lease file expiry is an absolute Unix time, not a duration**, and sits one lease time ahead of the clock: `1795289193` against a `now` of `1795245998` for a `12h` lease. | Observed, dnsmasq 2.91 | `linux-bridge-lab.md`, experiment 4 -> *The lease file, field by field* |
| `lease-absent-as-star` | **Both absent-value cases in a lease line are spelled `*`**, so a line always has five fields and a parser must read `*` as absent rather than as a hostname. | Observed, dnsmasq 2.91 | `linux-bridge-lab.md`, experiment 4 -> *The lease file, field by field* |
| `sighup-rereads-hostsfile` | **SIGHUP re-reads the `--dhcp-hostsfile` and nothing else**: the one `openat` in an `strace` of the signal, corroborated by an `,ignore` entry taking effect on the next request. | Observed, dnsmasq 2.91 | `linux-bridge-lab.md`, experiment 4 -> *What one SIGHUP does, in syscalls* and *The hosts file: re-read, confirmed twice over* |
| `sighup-ignores-config` | **SIGHUP does not re-read the configuration file**, so a `dhcp-range` change needs a restart: the file was absent from the trace and a changed range was still not in effect afterwards. | Observed, dnsmasq 2.91 | `linux-bridge-lab.md`, experiment 4 -> *The configuration file: not re-read* |
| `lease-file-write-only` | **The lease file is written, never read back, while the process runs**: a hand-added lease was neither adopted nor rejected across a SIGHUP and was erased by the next write, which rewrites the whole file. At startup it is opened `O_RDWR\|O_CREAT\|O_APPEND`, once. | Observed, dnsmasq 2.91 | `linux-bridge-lab.md`, experiment 4 -> *The lease file: written, never read back* |
| `dhcp-address-by-identity` | **An address is allocated from within the range by client identity, not by a counter**: a client was offered `10.0.0.128` from a range beginning at `10.0.0.100`, by two different clients and repeatably. | Observed, dnsmasq 2.91 | `linux-bridge-lab.md`, experiment 2 |
| `dhcp-unasked-option-set` | **The unasked-for option set in an OFFER and an ACK**: 53, 54, 51, 58 (T1, half the lease), 59 (T2, seven-eighths), 1, 28, 3 and 6, with `router` and `dns-server` defaulting to the server's own address. | Observed, dnsmasq 2.91 | `linux-bridge-lab.md`, experiment 2 |
| `dhcp-reply-unicast-to-offered` | **The reply to a client with no address is unicast to the address being offered** -- `10.0.0.1.67 > 10.0.0.128.68` -- which ordinary IP routing cannot deliver, and so requires either a hand-written ARP entry or a frame placed below the IP layer. | Observed, dnsmasq 2.91 | `linux-bridge-lab.md`, experiment 2 |
| `dhcp-implicit-interface-tag` | **An implicit tag naming the arrival interface is set on every DHCP transaction**, logged as `DHCPDISCOVER(gw-n1)` with `tags: gw-n1`, whether or not the configuration asks for it. | Observed, dnsmasq 2.91 | `linux-bridge-lab.md`, experiments 2 and 3 -> *Configuration 1* |
| `dhcp-ranges-merge-in-subnet` | **Two `--dhcp-range` in one subnet are accepted and merged into one pool**: a client on `gw-n2` was offered `10.0.0.141` out of `gw-n1`'s range, with no warning anywhere. | Observed, dnsmasq 2.91 | `linux-bridge-lab.md`, experiment 3 -> *Configuration 1* |
| `dhcp-tag-selects-range` | **A `tag:` constraint on `--dhcp-range` does disambiguate two same-subnet networks by arrival interface**, serving `10.0.0.128` on one and `10.0.0.241` on the other, end to end. | Observed, dnsmasq 2.91 | `linux-bridge-lab.md`, experiment 3 -> *Configuration 3* |
| `dhcp-tag-shares-address-space` | **A `tag:` constraint does not give each network its own address space**: with both tagged ranges set to the same single address, the second network's client was refused with `no address available`. | Observed, dnsmasq 2.91 | `linux-bridge-lab.md`, experiment 3 -> *Configuration 3* |
| `netns-socket-bound-at-creation` | **A socket belongs to the network namespace it was created in, for as long as it is open**, whatever its creator thread does afterwards: two sockets held `10.0.0.1:67`, one per namespace, with no `SO_REUSEADDR` anywhere and the owning process in the root namespace, and each received only its own network's datagrams. | Observed, Linux 6.12 / CPython 3.13 | `linux-bridge-lab.md`, experiment 5 -> *1. The sockets outlive the move* |
| `netns-setns-moves-thread` | **`setns(CLONE_NEWNET)` moves the calling thread, not the process**: two threads of one process were in two network namespaces at the same instant, and `/proc/self/ns/net` reports the main thread's namespace rather than a process-wide one. | Observed, Linux 6.12 / CPython 3.13 | `linux-bridge-lab.md`, experiment 5 -> *5. The one that constrains everything built on this* |
| `sock-bound-misses-broadcast` | **A UDP socket bound to a specific local address does not receive datagrams addressed to `255.255.255.255`**, so a DHCP socket must bind the wildcard -- and the wildcard is then only as wide as the namespace the socket was created in. | Observed, Linux 6.12 / CPython 3.13 | `linux-bridge-lab.md`, experiment 5 -> *2. What a `10.0.0.1:67` bind does not receive* |
| `sock-broadcast-needs-egress` | **A limited-broadcast reply cannot be sent from a namespace with one interface and no default route** without naming the egress interface: `sendto('255.255.255.255', 68)` with `SO_BROADCAST` returns `ENETUNREACH`. Either `SO_BINDTODEVICE` or a per-message `IP_PKTINFO` `ipi_ifindex` fixes it, both measured end to end. | Observed, Linux 6.12 / CPython 3.13 | `linux-bridge-lab.md`, experiment 5 -> *4. `SO_BINDTODEVICE` was not needed to receive* |
| `sock-unicast-fails-at-ip` | **A reply unicast to the address being offered fails silently at the IP layer**: `sendto` succeeds, the client never receives it, four OFFERs went out and no lease resulted. | Observed, Linux 6.12 / CPython 3.13 | `linux-bridge-lab.md`, experiment 5 -> *4.*, and experiment 2 from the other side |
| `sock-pktinfo-reports-arrival` | **`IP_PKTINFO` reports the arrival interface index and the datagram's destination address** on receipt -- `ifindex=13 spec_dst=10.0.0.1 daddr=10.0.0.1` for a unicast, `daddr=255.255.255.255` for a DISCOVER -- so a broadcast is distinguishable from a unicast without parsing the packet. | Observed, Linux 6.12 / CPython 3.13 | `linux-bridge-lab.md`, experiment 5 -> *1.* and *3. Two overlapping networks, one process, one `epoll` loop* |
| `netns-ifindex-is-local` | **An interface index is only meaningful inside its own namespace**: `gw-n1` is 13 in `ns-gw1` and `gw-n2` is 15 in `ns-gw2`, and `if_nametoindex` cannot resolve either name from the root namespace. | Observed, Linux 6.12 / CPython 3.13 | `linux-bridge-lab.md`, experiment 5 -> *1.* and *6. `AF_PACKET` is namespace-bound at creation too* |
| `netns-sysfs-ignores-setns` | **`/sys/class/net` does not follow `setns`** -- it shows the namespace its mount was made in -- while an `ioctl` on a socket does, so interface enumeration must not go through sysfs. | Observed, Linux 6.12 / CPython 3.13 | `linux-bridge-lab.md`, experiment 5 -> *6.* |
| `netns-afpacket-bound-at-creation` | **An `AF_PACKET` socket is bound to the namespace it was created in**, and a hand-built Ethernet/IPv4/UDP frame sent on one from the root namespace reached a client with no address. | Observed, Linux 6.12 / CPython 3.13 | `linux-bridge-lab.md`, experiment 5 -> *6.* |
| `sock-unicast-works-below-ip` | **A reply unicast to the address being offered is deliverable below the IP layer**: an IP packet addressed to an address the client does not hold, in a frame addressed to the MAC it does, was accepted -- the server supplying the address resolution ARP cannot do. | Observed, Linux 6.12 / CPython 3.13 | `linux-bridge-lab.md`, experiment 5 -> *6.* |
| `dhcp-broadcast-flag-not-permission` | **A client that sent `broadcast=false` in its BOOTP flags accepted a reply broadcast to `255.255.255.255`** in every one of seven successful transactions, so a clear BROADCAST flag is not permission to unicast. | Observed, Linux 6.12 / CPython 3.13 | `linux-bridge-lab.md`, experiment 5 -> *3.*, *4.* and *6.* |
| `netns-capability-set` | **The capability set for serving many namespaces from one process**: `CAP_SYS_ADMIN` for `setns(CLONE_NEWNET)`, `CAP_NET_BIND_SERVICE` for port 67, `CAP_NET_RAW` for `AF_PACKET` and nothing else -- measured as a six-row matrix at uid 1000, with the two-network exchange then running unprivileged holding the first two. Opening `/var/run/netns/<ns>` needs no privilege at all. | Observed, Linux 6.12 / CPython 3.13 | `linux-bridge-lab.md`, experiment 5 -> *7. The capability set, and it is not root* |
| `netns-held-fd-keeps-namespace` | **A held file descriptor keeps a deleted network namespace alive, with its interfaces**, after `ip netns del` has removed the name and the mount and the descriptor no longer resolves to a path. | Observed, Linux 6.12 / CPython 3.13 | `linux-bridge-lab.md`, experiment 5 -> *8. A held descriptor keeps a deleted namespace alive* |
| `selector-client-id-over-chaddr` | **A client identifier outranks the hardware address when identifying a client.** Where a client supplies a client identifier, a server MUST use that identifier to identify the client; the contents of `chaddr` are the MUST-use fallback only where no client identifier was supplied. This fixes the top two places of teneral's selector precedence, whose remaining steps are unspecified and are documented as this project's own in `api-design.md` rather than given a row here. | RFC 2131 section 4.2 | RFC 2131 s4.2; the order is asserted by `the_precedence_is_the_published_one` and `the_highest_precedence_selector_a_client_presents_is_the_one_that_matches` |
| `lease-address-selection-order` | **The order a server considers addresses in when a client has none**: the address the client already holds under its current lease, ELSE the address it held under a now-expired or released lease if that address is in the pool and not already allocated, ELSE the address it asked for in the Requested IP Address option if that address is valid and not already allocated, ELSE a new address from the pool. The steps are tried in turn and the keyword is SHOULD, not MUST. Two things are *not* from this source and are documented as teneral's own in `api-design.md`: which address the last step picks, the section saying nothing about it and teneral choosing the lowest free one; and the reading of "valid" as falling inside one of the network's configured ranges. | RFC 2131 section 4.3.1 | RFC 2131 s4.3.1; the four steps are asserted by `the_address_a_client_already_holds_is_the_first_thing_considered`, `the_address_a_client_held_until_it_lapsed_comes_back_before_the_pool_does`, `the_address_a_client_asks_for_is_given_only_if_nothing_holds_it` and `the_pool_offers_the_lowest_free_address_and_each_network_has_its_own` |
