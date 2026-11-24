-- Bindings: the answer an operator configured for a client identity,
-- inside one network.
--
-- Applied to a database already carrying the identity, the event log,
-- and the networks, attachments and ranges. As with the migrations
-- before it, this file is never edited once it has been released: a
-- change to the shape of this table is a new file beside it, because
-- an edit here describes a database that only a fresh install has
-- ever had.

-- A binding: a selector, under a network, and the address to answer
-- with.
--
-- The primary key is the whole of the resource's identity, and it has
-- three parts because a selector is not a resource. A selector is a
-- (kind, value) pair that means something only inside a network --
-- the same MAC in two networks is two unrelated clients, legitimately
-- -- so there is no table in which a selector exists on its own, no
-- row to reach without naming a network, and no identifier the store
-- could hand out that would let somebody try. A binding is the only
-- place a selector appears, and it appears there under its network.
--
-- `selector_kind` is constrained to the kinds the lookup knows how to
-- match. A CHECK rather than a comment, and the duplication with the
-- code is deliberate: a misspelled kind would otherwise be a binding
-- an operator configured, that the store stored, and that no lookup
-- would ever consult -- a configured answer that is silently never
-- given. The two lists are held together by a test that writes every
-- kind the code knows through the engine directly and asserts the
-- table takes them all.
--
-- The address is one INTEGER holding its 32-bit value, as in
-- `address_range`, with the same two CHECK constraints bounding it to
-- the space an IPv4 address occupies. Note what is deliberately *not*
-- constrained here: the address is not required to fall inside one of
-- its network's ranges, because a network with no range at all is a
-- supported and common configuration -- the consumer that matches
-- static bindings and allocates nothing has exactly that -- and a
-- reservation outside a pool is how an operator keeps an address the
-- pool can never hand out. Nor is the address unique within a
-- network: two selectors for the same machine, its MAC and its
-- client identifier, legitimately point at one address, and the store
-- cannot tell that case from two machines sharing one. The conflict
-- that *is* decidable is a binding against an address a lease holds,
-- and that arrives with leases.
--
-- `hostname` is the name the binding contributes to its network's
-- forward and reverse zones, and is optional because an address with
-- no name is a complete answer to the question a DHCP client asks.
--
-- The reference to `network` is RESTRICT, like every other child
-- here: a network with bindings still configured refuses to be
-- deleted rather than quietly discarding them, and the refusal names
-- what is in the way.
CREATE TABLE binding (
    network TEXT NOT NULL REFERENCES network(name) ON DELETE RESTRICT,
    selector_kind TEXT NOT NULL,
    selector_value TEXT NOT NULL,
    address INTEGER NOT NULL,
    hostname TEXT,
    cursor_seq INTEGER NOT NULL,
    PRIMARY KEY (network, selector_kind, selector_value),
    CHECK (selector_kind IN (
        'client-id', 'duid-iaid', 'mac', 'user-class', 'vendor-class'
    )),
    CHECK (address >= 0),
    CHECK (address <= 4294967295)
) STRICT;

-- No index beside the primary key. The two questions asked of this
-- table are "the binding for this selector in this network", which is
-- the key itself, and "every binding in this network", which is the
-- key's leading column. An index on the selector alone would answer a
-- third question -- which networks hold this MAC -- that nothing
-- should be asking: a lookup that can be answered without naming a
-- network is the shape that serves one network's answer to another.
