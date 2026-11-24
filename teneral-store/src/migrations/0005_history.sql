-- The history's bound: what has been evicted from the event log, and
-- the index that makes an age bound affordable.
--
-- Applied to a database already carrying the identity, the event log,
-- and the networks, attachments, ranges, bindings and leases. As with
-- the migrations before it, this file is never edited once it has been
-- released: a change to the shape of this table is a new file beside
-- it, because an edit here describes a database that only a fresh
-- install has ever had.

-- How far the event log has been truncated, as exactly one row.
--
-- The retained history is bounded, so events are evicted, and a
-- consumer presenting a position from before the eviction has to be
-- told that rather than handed an empty answer. Those two are
-- different facts with different consequences: "nothing has happened
-- since your position" means carry on, and "what happened since your
-- position is gone" means start again from a snapshot. Inferring the
-- second from the first is the silent wrong answer in its watch-shaped
-- form -- a consumer that was away long enough would be told it is up
-- to date while holding a state the store left hours ago.
--
-- `evicted_through` is the highest sequence number whose event has
-- been removed, and zero means nothing has been. It is a complete
-- description of what is gone because the log is only ever truncated
-- from its oldest end: eviction deletes a prefix of the order and the
-- retained events are always a suffix of it, so one number separates
-- the two. A bound that deleted from the middle -- by age alone, with
-- a clock that had moved backwards, say -- would need a set rather
-- than a number, which is why the truncation is always expressed as a
-- sequence floor rather than as the predicate that produced it.
--
-- It is a table rather than an in-memory counter because retention
-- belongs to the store and has to survive a restart. A mark kept in
-- memory would reset to zero when the daemon started, and the store
-- would then tell a consumer whose position really had been evicted
-- that it could resume -- and send it nothing, with an in-sync marker
-- after it.
--
-- "Exactly one row" is held by the two constraints together, as it is
-- for the store's identity: the primary key stops a second row from
-- taking id 1 and the CHECK stops it from taking any other id.
CREATE TABLE event_history (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    evicted_through INTEGER NOT NULL CHECK (evicted_through >= 0)
) STRICT;

INSERT INTO event_history (id, evicted_through) VALUES (1, 0);

-- Finding the newest event that is older than the age bound's cutoff,
-- which is the one question the log is asked that its own key cannot
-- answer.
--
-- The count bound needs no index: it is arithmetic on the sequence a
-- write was just assigned. The age bound has to ask which events were
-- recorded before an instant, and without an index that is a scan of
-- the whole log on every write -- which would make the cost of writing
-- grow with the history retained, the opposite of what a bound is for.
-- With the index it is a lookup of the few rows that have fallen out
-- of the window, which in a running store is none or one.
--
-- The sequence is carried in the index so the lookup is answered from
-- it without reading the rows themselves.
CREATE INDEX event_log_by_time ON event_log (recorded_at, seq);
