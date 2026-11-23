-- Workflow engine (design doc §6.4, ADR-002, ADR-003, ADR-021).

-- Workflows and their immutable published versions --------------------------

CREATE TYPE workflow_status AS ENUM ('active', 'archived');

CREATE TABLE workflows (
    id               uuid PRIMARY KEY,
    org_id           uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    key              text NOT NULL CHECK (key ~ '^[a-z][a-z0-9-]{0,63}$'),
    name             text NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    description      text,
    status           workflow_status NOT NULL DEFAULT 'active',
    -- The HWF document being edited. Runs never read it directly.
    draft            jsonb NOT NULL,
    current_version  integer,
    created_by       uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_by       uuid REFERENCES users (id) ON DELETE SET NULL,
    updated_at       timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, key)
);

CREATE TABLE workflow_versions (
    workflow_id   uuid NOT NULL REFERENCES workflows (id) ON DELETE CASCADE,
    version       integer NOT NULL CHECK (version > 0),
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    document      jsonb NOT NULL,
    notes         text,
    published_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    published_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workflow_id, version)
);

-- Published versions are immutable.
CREATE FUNCTION huxley_workflow_versions_immutable() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'published workflow versions cannot be changed'
        USING ERRCODE = 'check_violation', CONSTRAINT = 'workflow_versions_immutable';
END
$$;
CREATE TRIGGER workflow_versions_immutable
    BEFORE UPDATE ON workflow_versions
    FOR EACH ROW EXECUTE FUNCTION huxley_workflow_versions_immutable();

-- Runs and step runs, partitioned by month on their UUIDv7 ids ---------------

CREATE TYPE run_status AS ENUM ('queued', 'running', 'succeeded', 'failed', 'cancelled');
CREATE TYPE step_status AS ENUM ('running', 'waiting', 'succeeded', 'failed', 'skipped', 'cancelled');

CREATE TABLE runs (
    id                uuid NOT NULL PRIMARY KEY,
    org_id            uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    workflow_id       uuid REFERENCES workflows (id) ON DELETE SET NULL,
    -- NULL for a test run of the draft; the document is then in `document`.
    workflow_version  integer,
    document          jsonb,
    status            run_status NOT NULL DEFAULT 'queued',
    trigger_type      text NOT NULL,
    trigger           jsonb NOT NULL DEFAULT '{}',
    inputs            jsonb NOT NULL DEFAULT '{}',
    -- Run-scoped variables (set_var), readable as run.vars.
    vars              jsonb NOT NULL DEFAULT '{}',
    outputs           jsonb,
    error             jsonb,
    actor_id          uuid REFERENCES users (id) ON DELETE SET NULL,
    parent_run_id     uuid,
    parent_step_key   text,
    cancel_requested  boolean NOT NULL DEFAULT false,
    created_at        timestamptz NOT NULL DEFAULT now(),
    started_at        timestamptz,
    finished_at       timestamptz,
    CHECK ((workflow_version IS NULL) = (document IS NOT NULL))
) PARTITION BY RANGE (id);

CREATE INDEX runs_workflow ON runs (workflow_id, id DESC);
CREATE INDEX runs_org_status ON runs (org_id, status, id DESC);

CREATE TABLE step_runs (
    run_id       uuid NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    step_key     text NOT NULL,
    attempt      integer NOT NULL CHECK (attempt > 0),
    org_id       uuid NOT NULL,
    step_id      text NOT NULL,
    step_type    text NOT NULL,
    status       step_status NOT NULL,
    -- Rendered inputs, or a control-flow step's journaled decision.
    input        jsonb,
    output       jsonb,
    error        jsonb,
    -- A secret was read while evaluating the input: `input` holds a redacted
    -- copy and `input_sealed` the real one, encrypted (huxley-crypto).
    tainted      boolean NOT NULL DEFAULT false,
    input_sealed bytea,
    -- Likewise for an output derived from a secret (e.g. a transform that
    -- builds an Authorization header): later steps read the real value.
    output_tainted boolean NOT NULL DEFAULT false,
    output_sealed  bytea,
    started_at   timestamptz NOT NULL DEFAULT now(),
    finished_at  timestamptz,
    PRIMARY KEY (run_id, step_key, attempt)
) PARTITION BY RANGE (run_id);

-- The smallest UUIDv7 for a moment: its first 48 bits are the Unix time in ms.
CREATE FUNCTION huxley_uuid7_floor(at timestamptz) RETURNS uuid
LANGUAGE sql IMMUTABLE AS $$
    SELECT (lpad(to_hex((extract(epoch FROM at) * 1000)::bigint), 12, '0')
            || '00000000000000000000')::uuid
$$;

-- Creates month partitions for runs and step_runs from last month to
-- `months_ahead` months ahead, plus the default partitions. Idempotent; the
-- scheduler calls it daily. SECURITY DEFINER: the app role can't create
-- tables itself; partitions get the usual default privileges.
CREATE FUNCTION huxley_ensure_run_partitions(months_ahead integer DEFAULT 2) RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp AS $$
DECLARE
    month timestamptz;
    suffix text;
BEGIN
    EXECUTE 'CREATE TABLE IF NOT EXISTS runs_default PARTITION OF runs DEFAULT';
    EXECUTE 'CREATE TABLE IF NOT EXISTS step_runs_default PARTITION OF step_runs DEFAULT';
    FOR i IN -1..months_ahead LOOP
        month := date_trunc('month', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' + make_interval(months => i);
        suffix := to_char(month AT TIME ZONE 'UTC', 'YYYY_MM');
        IF to_regclass('runs_' || suffix) IS NULL THEN
            EXECUTE format(
                'CREATE TABLE runs_%s PARTITION OF runs FOR VALUES FROM (%L) TO (%L)',
                suffix, huxley_uuid7_floor(month), huxley_uuid7_floor(month + interval '1 month'));
        END IF;
        IF to_regclass('step_runs_' || suffix) IS NULL THEN
            EXECUTE format(
                'CREATE TABLE step_runs_%s PARTITION OF step_runs FOR VALUES FROM (%L) TO (%L)',
                suffix, huxley_uuid7_floor(month), huxley_uuid7_floor(month + interval '1 month'));
        END IF;
    END LOOP;
END
$$;

REVOKE EXECUTE ON FUNCTION huxley_ensure_run_partitions(integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION huxley_ensure_run_partitions(integer) TO huxley_app;
SELECT huxley_ensure_run_partitions(2);

-- The job queue (ADR-002) ----------------------------------------------------

CREATE TABLE jobs (
    id            uuid PRIMARY KEY,
    kind          text NOT NULL,
    org_id        uuid,
    run_id        uuid,
    step_key      text,
    payload       jsonb NOT NULL DEFAULT '{}',
    run_at        timestamptz NOT NULL DEFAULT now(),
    priority      smallint NOT NULL DEFAULT 0,
    attempts      integer NOT NULL DEFAULT 0,
    locked_by     text,
    locked_until  timestamptz,
    last_error    text,
    -- Exhausted its attempts: kept for the dead-letter view, never claimed.
    dead          boolean NOT NULL DEFAULT false,
    created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX jobs_due ON jobs (priority DESC, run_at) WHERE NOT dead;
CREATE INDEX jobs_run ON jobs (run_id);
-- At most one queued (unclaimed) advance per run, and one step job per step.
CREATE UNIQUE INDEX jobs_one_queued_advance ON jobs (run_id)
    WHERE kind = 'advance' AND locked_by IS NULL AND NOT dead;
CREATE UNIQUE INDEX jobs_one_step ON jobs (run_id, step_key) WHERE kind = 'step' AND NOT dead;

-- Row-level security ---------------------------------------------------------

ALTER TABLE workflows ENABLE ROW LEVEL SECURITY;
CREATE POLICY workflows_all ON workflows FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
ALTER TABLE workflow_versions ENABLE ROW LEVEL SECURITY;
CREATE POLICY workflow_versions_all ON workflow_versions FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
ALTER TABLE runs ENABLE ROW LEVEL SECURITY;
CREATE POLICY runs_all ON runs FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
ALTER TABLE step_runs ENABLE ROW LEVEL SECURITY;
CREATE POLICY step_runs_all ON step_runs FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
-- Jobs are engine-internal: only system-scoped transactions see them.
ALTER TABLE jobs ENABLE ROW LEVEL SECURITY;
CREATE POLICY jobs_system ON jobs FOR ALL TO huxley_app
    USING (current_setting('huxley.org_scope', true) = '*')
    WITH CHECK (current_setting('huxley.org_scope', true) = '*');

-- Published versions and history are kept; the app never deletes them.
REVOKE DELETE ON workflow_versions FROM huxley_app;
