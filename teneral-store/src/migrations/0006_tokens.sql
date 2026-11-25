-- Tokens: who may reach this store and what they may do with it.
-- And, on every table that was already here, who made each write.
--
-- Applied to a database already carrying the identity, the event log,
-- the networks, attachments, ranges, bindings and leases, and the
-- history's bound. As with the migrations before it, this file is
-- never edited once it has been released: a change to the shape of
-- these tables is a new file beside it, because an edit here
-- describes a database that only a fresh install has ever had.
--
-- Every table here is STRICT, so a value whose type does not match
-- its column is refused rather than stored as given.

-- A token: a secret somebody presents, the networks it may reach and
-- the things it may do in them.
--
-- A token is store-global rather than per-network, which is the one
-- exception to this schema's rule and is not an oversight. Its answer
-- *is* which networks may be touched, so there is no network to ask
-- it within -- the same shape as the attachment lookup whose answer
-- is a network. Nothing here is keyed by network, and the scope is
-- the child table below.
--
-- `secret_sha256` is the whole of what is kept of the secret: the
-- lower-case hex of its SHA-256. The plaintext exists in this process
-- for the length of one mint call and is never written anywhere, so a
-- reader of this file -- a backup, a copied database, an operator with
-- the file open -- learns nothing they could present. Verification
-- hashes what was presented and looks the hash up, so the plaintext
-- never reaches a comparison either.
--
-- It is not a password hash and deliberately not slow. The secret is
-- 32 bytes the engine generated, so there is no guessable input for a
-- key-derivation function to defend against; a slow one would only
-- make every connection's handshake slower while defending against an
-- attack nobody can mount.
--
-- UNIQUE because a lookup by hash has to answer with at most one
-- token. Two tokens sharing a secret is already impossible by the
-- width of the random value, and this is what makes the lookup's
-- single answer structural rather than a property of the generator.
--
-- `all_networks` is the explicit every-network form, and it is a
-- column rather than a sentinel in the child table because a
-- sentinel would be a network name -- and names here are opaque, so
-- `*` is a legal name for a real network. A wildcard is typed, never
-- spelled.
--
-- `minted_by` is the actor that minted this token, in the same
-- rendering the columns below use, and `last_used_at` is when it was
-- last presented successfully. Those two and `minted_at` are a
-- token's whole audit trail: a token mutation emits no event, because
-- the event log's network column is not nullable and a token has no
-- network to put in it, so the row has to carry its own history. NULL
-- in `last_used_at` means it has never been used, which is a
-- different fact from having been used at the epoch.
--
-- `name` is what an operator calls this token in a listing. It is not
-- unique: two tokens minted for the same consumer on two hosts
-- legitimately share one, and the identity is the id.
CREATE TABLE token (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    secret_sha256 TEXT NOT NULL UNIQUE,
    all_networks INTEGER NOT NULL CHECK (all_networks IN (0, 1)),
    minted_by TEXT NOT NULL,
    minted_at INTEGER NOT NULL,
    last_used_at INTEGER
) STRICT;

-- The networks a token may reach, one row each, and none at all when
-- `all_networks` is 1.
--
-- Deliberately no foreign key to `network`. A scope is a statement
-- about names rather than a reference to resources that exist: the
-- first token is minted before any network is created, so a token
-- naming a network that is not there yet is the ordinary bootstrap
-- order rather than an error. A reference would also make deleting a
-- network refuse while any token mentions it, which is a coupling
-- between an operator's configuration and their credentials that
-- nothing here wants.
--
-- The cascade is on the token, which is the real parent: deleting a
-- token takes its scope and its capabilities with it, because a
-- half-deleted token is a credential whose reach nobody can read.
CREATE TABLE token_network (
    token TEXT NOT NULL REFERENCES token(id) ON DELETE CASCADE,
    network TEXT NOT NULL,
    PRIMARY KEY (token, network)
) STRICT;

-- What a token may do, one row per capability.
--
-- Rows rather than columns or a bitmask, which is what makes the
-- capability a consumer of some resource this store does not hold yet
-- an insert rather than a migration.
--
-- The CHECK constrains it to the four the daemon decides on, and the
-- duplication with the code is deliberate for the same reason the
-- selector kinds' is: a misspelled capability would otherwise be a
-- permission an operator granted, that the store stored, and that no
-- authorisation check would ever consult -- a grant that is silently
-- never honoured. `admin` is in the list because the operator's own
-- operations -- minting and deleting tokens, listing them, minting an
-- epoch -- are gated by a capability on a token rather than by a
-- second socket whose mode the kernel enforces.
CREATE TABLE token_capability (
    token TEXT NOT NULL REFERENCES token(id) ON DELETE CASCADE,
    capability TEXT NOT NULL CHECK (capability IN (
        'read', 'write', 'watch', 'admin'
    )),
    PRIMARY KEY (token, capability)
) STRICT;

-- No index beside the three primary keys and the unique constraint on
-- the hash. Two questions are asked of these tables: "which token has
-- this secret", which the unique index answers, and "what may this
-- token reach and do", which is the leading column of both child
-- keys. There is deliberately no index that would answer "which
-- tokens can reach this network" cheaply; nothing authenticates in
-- that direction, and an index is a reasonable place to notice that
-- something has started to.

-- Who made each write, in the log and on the row.
--
-- The log answers "what happened and in what order"; it does not
-- answer "who did this" until it carries the actor, and
-- `docs/api-design.md` asks for a write to record the token that made
-- it. The actor is one TEXT value in the store's own rendering -- a
-- token id, a local peer's uid, or a named part of the daemon --
-- rather than a reference into `token`, for two reasons. A peer and
-- a daemon are not tokens and have no row to point at; and a deleted
-- token must not take the attribution of its writes with it, which a
-- foreign key would either forbid or cascade.
--
-- It is kept on the resource row *as well as* in the log, and the
-- duplication is the point. The retained history is bounded, so the
-- event that says who last moved a binding is evicted on a schedule,
-- while the binding itself stays until somebody changes it. Only the
-- log can say who made a write that has since been superseded, and
-- only the row can still say who made the write that produced the
-- state something is answering with today. Both questions are asked
-- and neither column answers the other.
--
-- SQLite needs a default for a NOT NULL column added to a table that
-- already has rows, and the default is a word rather than an empty
-- string. An empty actor would read as a recorded fact -- a write
-- made by nobody -- where what is true is that this row was written
-- before the store recorded who wrote it. It cannot be confused with
-- a real actor either: every actor this code renders carries its kind
-- and a colon, so a bare word is not one.
ALTER TABLE event_log ADD COLUMN actor TEXT NOT NULL DEFAULT 'unrecorded';
ALTER TABLE network ADD COLUMN written_by TEXT NOT NULL DEFAULT 'unrecorded';
ALTER TABLE attachment ADD COLUMN written_by TEXT NOT NULL DEFAULT 'unrecorded';
ALTER TABLE address_range ADD COLUMN written_by TEXT NOT NULL DEFAULT 'unrecorded';
ALTER TABLE binding ADD COLUMN written_by TEXT NOT NULL DEFAULT 'unrecorded';
ALTER TABLE lease ADD COLUMN written_by TEXT NOT NULL DEFAULT 'unrecorded';
