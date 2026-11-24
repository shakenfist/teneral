-- The foundations: who this store is, and the log that gives every
-- mutation a place in one total order.
--
-- Applied once, inside a transaction, to a database whose
-- user_version is 0. Migrations are forward-only, so a change to the
-- shape of these tables is a new file beside this one rather than an
-- edit here: an edit here describes a database that only a fresh
-- install has ever had.
--
-- Both tables are STRICT, which makes SQLite reject a value whose
-- type does not match the column rather than storing it as given.
-- Without it a column declared INTEGER holds the text "10.0.0.5"
-- quite happily, and every comparison against it then answers
-- something -- just not the thing the query meant.

-- The store's identity, as exactly one row.
--
-- The epoch is the half of a cursor that says whose sequence numbers
-- these are. A consumer that has been away presents a cursor, and the
-- epoch is what separates "carry on from here" from "that number
-- belongs to another store's history". Without it, a consumer pointed
-- at a different store -- a fresh file at the same path, a second
-- daemon, a database recreated after being deleted -- is handed
-- sequence numbers it recognises attached to somebody else's events,
-- and nothing about the stream looks wrong.
--
-- Be precise about what it does NOT cover, because this comment used
-- to claim more. It does not detect this same store restored from a
-- backup. The epoch lives in this table, so a file-level restore
-- brings it back unchanged, together with a sequence counter wound
-- back to the backup. A consumer whose position is ahead of the
-- restore point is caught, by presenting a position the store never
-- issued; a consumer whose position is at or below the restore point
-- is not caught at all, and is served different events under numbers
-- it has already seen. That is the failure this value is named for,
-- and against a restore it does not prevent it.
--
-- The remedy is an operator action rather than a mechanism here:
-- after restoring a store from a backup, mint a new epoch before any
-- consumer reconnects, which makes the restored store honestly a
-- different store. Nothing in this crate offers that yet, and the
-- gap is recorded in the plan's future work rather than left to be
-- discovered by a consumer holding a stale cursor.
--
-- The value is generated here, by the engine, at the instant the
-- store is created: sixteen random bytes as lower-case hex. Doing it
-- in the migration rather than on each open is what makes it stable
-- across a reopen structurally, rather than through an upsert that
-- somebody has to keep idempotent, and it costs neither a dependency
-- nor a line of Rust.
--
-- "Exactly one row" is held by the two constraints together: the
-- primary key stops a second row from taking id 1, and the CHECK
-- stops it from taking any other id.
CREATE TABLE store_identity (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    epoch TEXT NOT NULL
) STRICT;

INSERT INTO store_identity (id, epoch)
VALUES (1, lower(hex(randomblob(16))));

-- The event log: one row per mutation, in the order the store applied
-- them, and the source of the sequence half of every cursor.
--
-- AUTOINCREMENT is the load-bearing word in the first column. A bare
-- INTEGER PRIMARY KEY is the largest rowid in use plus one, so SQLite
-- reuses the numbers of deleted rows -- and this table is designed to
-- be deleted from, because the retained history is bounded and old
-- events are evicted. A reused sequence number hands a resuming
-- consumer somebody else's history while looking perfectly
-- well-formed, which is the failure the epoch above exists to
-- prevent, reintroduced one layer further down. AUTOINCREMENT keeps a
-- high-water mark in sqlite_sequence instead, so no number is ever
-- issued twice in the life of the database.
--
-- Every event carries its network, including when it is read back on
-- a stream scoped to a single network. Repeating it costs a few bytes
-- and removes a class of bug: a consumer that learned to infer the
-- network from its subscription is wrong the day somebody widens the
-- subscription. Note that the order here is total across all
-- networks, and that this is not a claim about causality between
-- them -- two networks' events are interleaved by nothing more than
-- when they arrived.
--
-- `kind` and `state` are deliberately generic. A kind is a string and
-- a state is a serialised post-state, rather than a column per
-- resource, which is what makes the resources that are not built yet
-- a migration and a variant each rather than a reshaping of the one
-- table every consumer reads.
--
-- `state` holds the resource's whole post-state rather than a delta,
-- so applying an event is idempotent and at-least-once delivery is
-- enough; NULL is how a deletion is spelled, being the absence the
-- event is reporting. `resource_id` is the resource's identity within
-- its network, rendered as text by whichever kind owns it, and is
-- opaque here: it is in its own column so that the log can be read
-- for one resource's history without parsing `state`.
--
-- `recorded_at` has no default, deliberately. The store's notion of
-- time is injected so that a test can advance it on command, and a
-- DEFAULT here would be a second clock that no test can move.
CREATE TABLE event_log (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    network TEXT NOT NULL,
    kind TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    state TEXT,
    recorded_at INTEGER NOT NULL
) STRICT;

-- Reading one network's history, which is the common case, and the
-- scoping the cursor's total order does not give on its own.
CREATE INDEX event_log_by_network ON event_log (network, seq);
