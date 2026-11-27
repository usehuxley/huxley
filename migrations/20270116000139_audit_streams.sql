-- SIEM audit streaming (ADR-087): audit records are shipped, as they're
-- sealed into the hash chain, to a SIEM over HTTPS: a generic NDJSON
-- webhook or Splunk's HTTP Event Collector. Each stream keeps its place in
-- the chain (`last_seq`), so nothing is skipped or sent twice in normal
-- running, and backs off while its destination is failing.

CREATE TABLE audit_streams (
    id               uuid PRIMARY KEY,
    name             text NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    kind             text NOT NULL CHECK (kind IN ('webhook', 'splunk_hec')),
    url              text NOT NULL CHECK (url ~ '^https?://'),
    -- Webhook: the header the secret goes in (Authorization by default).
    header_name      text,
    -- The token or header value, encrypted (bound to the stream's id).
    secret_sealed    bytea,
    enabled          boolean NOT NULL DEFAULT true,
    -- The last record shipped.
    last_seq         bigint NOT NULL DEFAULT 0,
    last_shipped_at  timestamptz,
    failures         integer NOT NULL DEFAULT 0,
    last_error       text,
    next_attempt_at  timestamptz NOT NULL DEFAULT now(),
    created_by       uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now()
);

-- Sealing on demand (before the audit list, an export, a verification or a
-- stream) waits for a sealer that's already running, so what committed
-- before the call is sealed when it returns; the background sealer still
-- skips (ADR-086).
DROP FUNCTION huxley_audit_seal(integer);
CREATE FUNCTION huxley_audit_seal(batch integer DEFAULT 1000, wait boolean DEFAULT false) RETURNS integer
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp AS $$
DECLARE
    head_seq  bigint;
    head_hash bytea;
    e         audit_events;
    n         integer := 0;
BEGIN
    IF wait THEN
        SELECT seq, hash INTO head_seq, head_hash FROM audit_chain_head WHERE id = 1 FOR UPDATE;
    ELSE
        SELECT seq, hash INTO head_seq, head_hash FROM audit_chain_head WHERE id = 1 FOR UPDATE SKIP LOCKED;
        IF NOT FOUND THEN
            RETURN 0;
        END IF;
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
GRANT EXECUTE ON FUNCTION huxley_audit_seal(integer, boolean) TO huxley_app;
