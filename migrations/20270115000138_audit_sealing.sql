-- Audit chain sealing (ADR-086). Audit rows used to take their place in the
-- hash chain as they were inserted, by updating the one chain-head row:
-- every audited transaction held that row until it committed, which
-- serialized audit writes and deadlocked transactions that locked other
-- rows in the opposite order. Now rows go in unsealed (no seq or hashes)
-- and huxley_audit_seal() chains committed rows in order, one sealer at a
-- time, shortly after. The application still can't update or delete audit
-- rows; only the sealer (SECURITY DEFINER) fills in their chain fields.

ALTER TABLE audit_events
    ALTER COLUMN seq DROP NOT NULL,
    ALTER COLUMN prev_hash DROP NOT NULL,
    ALTER COLUMN hash DROP NOT NULL;

CREATE INDEX audit_events_unsealed ON audit_events (occurred_at, id) WHERE seq IS NULL;

CREATE OR REPLACE FUNCTION huxley_audit_chain() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    -- Sealed later, in commit order, by huxley_audit_seal().
    NEW.seq := NULL;
    NEW.prev_hash := NULL;
    NEW.hash := NULL;
    NEW.occurred_at := now();
    RETURN NEW;
END
$$;

-- Chains up to `batch` committed, unsealed rows (oldest first) onto the
-- head. Returns how many it sealed: 0 when there were none, or when another
-- sealer holds the head (it'll seal them).
CREATE FUNCTION huxley_audit_seal(batch integer DEFAULT 1000) RETURNS integer
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp AS $$
DECLARE
    head_seq  bigint;
    head_hash bytea;
    e         audit_events;
    n         integer := 0;
BEGIN
    SELECT seq, hash INTO head_seq, head_hash FROM audit_chain_head WHERE id = 1 FOR UPDATE SKIP LOCKED;
    IF NOT FOUND THEN
        RETURN 0;
    END IF;
    FOR e IN
        SELECT * FROM audit_events WHERE seq IS NULL ORDER BY occurred_at, id LIMIT batch FOR UPDATE
    LOOP
        head_seq := head_seq + 1;
        e.seq := head_seq;
        e.prev_hash := head_hash;
        e.hash := huxley_audit_hash(head_hash, e);
        UPDATE audit_events SET seq = e.seq, prev_hash = e.prev_hash, hash = e.hash WHERE id = e.id;
        head_hash := e.hash;
        n := n + 1;
    END LOOP;
    IF n > 0 THEN
        UPDATE audit_chain_head SET seq = head_seq, hash = head_hash WHERE id = 1;
    END IF;
    RETURN n;
END
$$;

GRANT EXECUTE ON FUNCTION huxley_audit_seal(integer) TO huxley_app;
