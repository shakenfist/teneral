-- The first three configured resources: networks, the interfaces
-- attached to them, and the address ranges they own.
--
-- Applied to a database already carrying the identity and the event
-- log. As with the migration before it, this file is never edited
-- once it has been released: a change to the shape of these tables is
-- a new file beside it, because an edit here describes a database
-- that only a fresh install has ever had.
--
-- Every table here is STRICT, so a value whose type does not match
-- its column is refused rather than stored as given. That matters
-- most for the address columns: without it an INTEGER column holds
-- the text "10.0.0.5" quite happily, and every comparison against it
-- then answers something other than what the query meant.
--
-- Every table also carries a `cursor_seq`, which is the sequence
-- number of the event the write that produced this state emitted.
-- Two things rest on it. A reader gets the position in the store's
-- order at which each row was last written, without a second query;
-- and a caller that wants to change a row can present that position
-- back, so that "change this only if it is still what I read" is one
-- operation rather than a read followed by a write that was correct
-- only until somebody else got in between.

-- A network: a name and nothing else.
--
-- The name is the caller's, unique in the store, and opaque here -- a
-- UUID and the word `guest` are equally valid, because the consumer
-- that already has a stable identifier for the thing should not have
-- to keep a mapping to a second one this store invented. Deliberately
-- not keyed on the subnet, which two networks may legitimately share,
-- nor on an interface, which can be renamed.
--
-- A network is not a subnet and holds no addresses of its own. It
-- owns zero or more ranges as child rows, which is what makes a
-- static-only network with no range and a home network with a pool
-- the same resource with different children rather than two modes.
CREATE TABLE network (
    name TEXT PRIMARY KEY,
    cursor_seq INTEGER NOT NULL
) STRICT;

-- An attachment: an interface, in a namespace, belonging to a
-- network.
--
-- The primary key is `(namespace, ifname)` and the network is an
-- ordinary column. That is the single most important line in this
-- file. An interface belongs to exactly one network, and a second
-- network's claim on an already-attached interface is the one place
-- where a cross-network leak would be configured rather than coded --
-- so the database refuses it, whether or not any code above
-- remembered to look. Keyed the other way round, as
-- `(network, namespace, ifname)`, the same two rows would both be
-- accepted and every single-network test would still pass.
--
-- The namespace is in the key because an interface name is unique
-- only within a network namespace, and both of the namespace-using
-- consumers put each network in one of its own. Identity without it
-- would collide the first time a consumer named its interfaces by
-- role rather than by network.
--
-- The reference to `network` is RESTRICT rather than CASCADE: a
-- network with interfaces still attached refuses to be deleted
-- instead of quietly detaching them. An operator who meant it
-- detaches first, and the refusal names what is in the way. Note that
-- this is enforced only on a connection that asked for foreign key
-- enforcement, which every connection this store opens does.
CREATE TABLE attachment (
    namespace TEXT NOT NULL,
    ifname TEXT NOT NULL,
    network TEXT NOT NULL REFERENCES network(name) ON DELETE RESTRICT,
    cursor_seq INTEGER NOT NULL,
    PRIMARY KEY (namespace, ifname)
) STRICT;

-- Listing one network's attachments, which the primary key above
-- cannot serve because the network is not its leading column. The
-- trailing columns make the listing ordered without a sort.
CREATE INDEX attachment_by_network ON attachment (network, namespace, ifname);

-- An address range: a contiguous span of addresses a network may
-- allocate from, named within that network.
--
-- The addresses are each one INTEGER holding the address's 32-bit
-- value, not a dotted quad in TEXT. "The lowest free address in this
-- range" is then ordinary SQL and containment is a comparison; as
-- text both are string operations, which give the right answer for a
-- /24 and the wrong one the first time a range crosses an octet.
-- IPv6 is not in this schema at all, so the column is not widened in
-- advance: the migration that adds v6 is where the choice between a
-- 16-byte BLOB and a table of its own gets made, with something
-- actually using it.
--
-- The CHECK constraints are the two things no caller can be trusted
-- to have got right and that are cheap to state here: the span runs
-- forwards, and both ends are inside the unsigned 32-bit space that
-- an IPv4 address occupies. SQLite's INTEGER is signed and 64-bit, so
-- without the upper bound a column declared for an address would
-- accept a number no address can have.
--
-- Ranges in one network may not overlap each other; ranges in
-- different networks may be identical, and routinely are. The first
-- half of that is not expressible as a constraint on a single row, so
-- it is checked in the transaction that does the write. The second
-- half is what this key gives for nothing by naming the network.
CREATE TABLE address_range (
    network TEXT NOT NULL REFERENCES network(name) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    first_address INTEGER NOT NULL,
    last_address INTEGER NOT NULL,
    cursor_seq INTEGER NOT NULL,
    PRIMARY KEY (network, name),
    CHECK (first_address <= last_address),
    CHECK (first_address >= 0),
    CHECK (last_address <= 4294967295)
) STRICT;

-- Finding the ranges a network's addresses fall in, which is the
-- overlap check above and will be the containment check an allocation
-- needs. The primary key leads with the network but then orders by
-- name, which answers neither.
CREATE INDEX address_range_by_span ON address_range (network, first_address, last_address);
