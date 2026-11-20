# Provenance

Teneral is Apache-2.0. dnsmasq is GPL-2/3 and Technitium DNS
Server is GPL-3.0, so no code may be copied from either. Reading
them to work out *what* to build is fine; this file is how the
project stays able to say where each behaviour came from, years
later, without relying on anyone's memory.

Every behaviour visible on the wire gets a row. The rule is one of
two sources, and the row says which:

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

Rows are added by the phase that adds the behaviour. The table is
empty because nothing is built yet; its first rows come from phase
1b, which measures dnsmasq's lease file format and its SIGHUP
re-read behaviour.

| Behaviour | Source | Citation |
|-----------|--------|----------|
