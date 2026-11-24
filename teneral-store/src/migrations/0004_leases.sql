-- Leases: who is actually using an address in a network right now,
-- as opposed to who an operator configured it for.
--
-- Applied to a database already carrying the identity, the event log,
-- the networks, attachments and ranges, and the bindings. As with the
-- migrations before it, this file is never edited once it has been
-- released: a change to the shape of this table is a new file beside
-- it, because an edit here describes a database that only a fresh
-- install has ever had.

-- A lease: one address in one network, and the client holding it.
--
-- The primary key is `(network, address)` and not the client. Within
-- one network an address has at most one holder at a time, which is
-- what makes it a true key; a client may legitimately hold two
-- addresses at once while it moves from one to another, which makes
-- the client not one. Keyed by client, the second address would
-- either overwrite the first or need a key that is not the client
-- after all.
--
-- The consequence is that "what does this client hold" is a query
-- rather than a key lookup, and the index at the foot of this file is
-- what makes it a cheap one. That query is the thing the consumer
-- this store replaces cannot do at all today, so it is a first class
-- operation rather than a filter a caller applies to a list of
-- everything.
--
-- `holder_kind` and `holder_value` are the selector the client was
-- identified by when it acquired the address, constrained to the same
-- five kinds a binding may be configured against and for the same
-- reason: a kind no lookup knows how to match would be a holder no
-- query would ever find. A lease records one selector rather than
-- everything the client presented, because the holder has to be a
-- single comparable value, and the one recorded is the
-- highest-precedence attribute the client presented -- the same
-- published order a binding lookup walks, so that a client is
-- identified the same way whether it is being matched against
-- configuration or being given an address.
--
-- `lease_id` is the sequence number at which the current holder
-- acquired this address, so it changes when and only when the address
-- changes holder. A renewal keeps it because the holder has not
-- changed; an acquisition issues a new one because acquiring is what
-- issues it. It is deliberately not a random value: derived from the
-- store's own order it is deterministic in a test, needs no generator,
-- and is already a position a consumer can compare against its stream.
-- It is paired with the epoch outside this table, because a sequence
-- number from a restored backup means nothing on its own.
--
-- `expires_at` is an absolute instant, as seconds since the Unix
-- epoch, and not a duration with a start. Expiry is therefore a
-- comparison against the clock and not an arithmetic the reader has to
-- repeat, and the value is the same one the protocol's own lease files
-- record. `acquired_at` is kept beside it because an operator looking
-- at a lease wants to know how long it has been held, which neither
-- the expiry nor the lease id answers on its own.
--
-- Nothing in this table says "expired". Expiry is evaluated when the
-- row is read, against the clock, by every read and every allocation,
-- so a lease whose instant has passed is expired whether or not
-- anything has swept it. The sweep exists to turn those expiries into
-- events for consumers watching the history; it is not what makes
-- them true, and no correctness here waits for it.
--
-- The address is one INTEGER holding its 32-bit value, as in
-- `address_range` and `binding`, with the same two CHECK constraints
-- bounding it to the space an IPv4 address occupies. The third CHECK
-- refuses a lease that expires before it was acquired, which is a
-- lease that was never held and would read as expired from the
-- instant it was written.
--
-- The reference to `network` is RESTRICT, like every other child
-- here: a network with leases on it refuses to be deleted rather than
-- quietly discarding them, and the refusal names what is in the way.
-- Note that a lease is not a child of a range: an address may be
-- leased in a network whose ranges no longer contain it, because a
-- pool can be resized under a lease that is already held, and
-- cascading that resize into a revocation would take an address away
-- from a client that is still using it without telling anybody.
CREATE TABLE lease (
    network TEXT NOT NULL REFERENCES network(name) ON DELETE RESTRICT,
    address INTEGER NOT NULL,
    holder_kind TEXT NOT NULL,
    holder_value TEXT NOT NULL,
    lease_id INTEGER NOT NULL,
    acquired_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    cursor_seq INTEGER NOT NULL,
    PRIMARY KEY (network, address),
    CHECK (holder_kind IN (
        'client-id', 'duid-iaid', 'mac', 'user-class', 'vendor-class'
    )),
    CHECK (address >= 0),
    CHECK (address <= 4294967295),
    CHECK (expires_at >= acquired_at)
) STRICT;

-- Answering "what does this client hold in this network", which the
-- primary key cannot: the holder is not its leading column.
--
-- Deliberately not unique. A client holding two addresses at once is
-- the case the key above exists to allow, so a unique index here
-- would forbid exactly what keying by address was chosen to permit.
--
-- The network leads it, as every index in this schema does. An index
-- on the holder alone would answer a question nothing should be
-- asking -- which networks hold this MAC -- and a lookup that can be
-- answered without naming a network is the shape that serves one
-- network's answer to another.
CREATE INDEX lease_by_holder ON lease (network, holder_kind, holder_value);

-- Finding the lowest address in a network that nothing holds, which
-- is what an allocation from a pool walks. Ordered by address within
-- the network, which the primary key already gives; the expiry is
-- carried in the index so that the walk can skip an expired lease
-- without reading the row it belongs to.
CREATE INDEX lease_by_address ON lease (network, address, expires_at);
