-- AI triage (design doc §8.8, ADR-047): the AI triage pack suggests a
-- ticket's queue and priority; suggestions are kept to compare with where
-- tickets end up. Accuracy is measured by running the same workflow over a
-- labeled sample set.

-- What the AI suggested for a ticket.
CREATE TABLE sd_triage (
    ticket_id  uuid PRIMARY KEY REFERENCES sd_tickets (id) ON DELETE CASCADE,
    org_id     uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    queue_id   uuid REFERENCES sd_queues (id) ON DELETE SET NULL,
    priority   text CHECK (priority IN ('p1', 'p2', 'p3', 'p4')),
    reason     text CHECK (reason IS NULL OR length(reason) <= 300),
    -- It changed the ticket (no one had touched it yet).
    applied    boolean NOT NULL,
    at         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sd_triage_org ON sd_triage (org_id, at);

-- Labeled examples: text and the right queue and priority.
CREATE TABLE sd_triage_samples (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    text        text NOT NULL CHECK (length(btrim(text)) BETWEEN 1 AND 8000),
    queue_id    uuid REFERENCES sd_queues (id) ON DELETE CASCADE,
    priority    text CHECK (priority IN ('p1', 'p2', 'p3', 'p4')),
    -- Taken from this resolved ticket.
    ticket_id   uuid REFERENCES sd_tickets (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    CHECK (queue_id IS NOT NULL OR priority IS NOT NULL)
);
CREATE UNIQUE INDEX sd_triage_samples_ticket ON sd_triage_samples (org_id, ticket_id) WHERE ticket_id IS NOT NULL;

CREATE TABLE sd_triage_evaluations (
    id                uuid PRIMARY KEY,
    org_id            uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    workflow_id       uuid REFERENCES workflows (id) ON DELETE SET NULL,
    workflow_version  integer,
    -- The accuracy target then, in percent.
    target            smallint NOT NULL CHECK (target BETWEEN 0 AND 100),
    started_by        uuid REFERENCES users (id) ON DELETE SET NULL,
    started_at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sd_triage_evaluations_org ON sd_triage_evaluations (org_id, started_at);

-- One per sample: what was expected (copied, so later edits don't change
-- the result) and what the workflow answered.
CREATE TABLE sd_triage_results (
    evaluation_id      uuid NOT NULL REFERENCES sd_triage_evaluations (id) ON DELETE CASCADE,
    sample_id          uuid REFERENCES sd_triage_samples (id) ON DELETE SET NULL,
    position           integer NOT NULL,
    org_id             uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    text               text NOT NULL,
    expected_queue_id  uuid REFERENCES sd_queues (id) ON DELETE SET NULL,
    expected_priority  text,
    run_id             uuid REFERENCES runs (id) ON DELETE SET NULL,
    queue_id           uuid REFERENCES sd_queues (id) ON DELETE SET NULL,
    priority           text,
    reason             text,
    answered_at        timestamptz,
    PRIMARY KEY (evaluation_id, position)
);

ALTER TABLE sd_triage ENABLE ROW LEVEL SECURITY;
ALTER TABLE sd_triage_samples ENABLE ROW LEVEL SECURITY;
ALTER TABLE sd_triage_evaluations ENABLE ROW LEVEL SECURITY;
ALTER TABLE sd_triage_results ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_triage_all ON sd_triage FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
CREATE POLICY sd_triage_samples_all ON sd_triage_samples FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
CREATE POLICY sd_triage_evaluations_all ON sd_triage_evaluations FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
CREATE POLICY sd_triage_results_all ON sd_triage_results FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
