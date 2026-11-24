-- SLAs (ADR-037): policies with targets per priority, where they apply,
-- and each ticket's clocks.

CREATE TABLE sd_sla_policies (
    id            uuid PRIMARY KEY,
    name          text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    description   text NOT NULL DEFAULT '' CHECK (length(description) <= 2000),
    -- Whose hours count: the client's primary site, the policy's own, or all.
    hours_source  text NOT NULL DEFAULT 'client_site'
                  CHECK (hours_source IN ('client_site', 'policy', 'always')),
    -- `{"mon": [["09:00", "17:00"]], …}`, like a site's.
    hours         jsonb NOT NULL DEFAULT '{"mon": [["09:00", "17:00"]], "tue": [["09:00", "17:00"]],
                                           "wed": [["09:00", "17:00"]], "thu": [["09:00", "17:00"]],
                                           "fri": [["09:00", "17:00"]]}'::jsonb,
    -- For the policy's hours; none: the organization's time zone setting.
    timezone      text,
    is_default    boolean NOT NULL DEFAULT false,
    version       integer NOT NULL DEFAULT 1,
    created_at    timestamptz NOT NULL DEFAULT now(),
    archived_at   timestamptz
);
CREATE UNIQUE INDEX sd_sla_policies_name ON sd_sla_policies (lower(name)) WHERE archived_at IS NULL;
CREATE UNIQUE INDEX sd_sla_policies_default ON sd_sla_policies ((true)) WHERE is_default AND archived_at IS NULL;

CREATE TABLE sd_sla_targets (
    policy_id          uuid NOT NULL REFERENCES sd_sla_policies (id) ON DELETE CASCADE,
    priority           text NOT NULL CHECK (priority IN ('p1', 'p2', 'p3', 'p4')),
    response_minutes   integer CHECK (response_minutes > 0),
    resolution_minutes integer CHECK (resolution_minutes > 0),
    PRIMARY KEY (policy_id, priority)
);

-- The policy for an organization and those below it (nearest wins).
CREATE TABLE sd_sla_assignments (
    org_id     uuid PRIMARY KEY REFERENCES organizations (id) ON DELETE CASCADE,
    policy_id  uuid NOT NULL REFERENCES sd_sla_policies (id) ON DELETE CASCADE
);

-- One per ticket with an SLA: the terms it was opened under, and its clocks.
CREATE TABLE sd_ticket_sla (
    ticket_id               uuid PRIMARY KEY REFERENCES sd_tickets (id) ON DELETE CASCADE,
    org_id                  uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    policy_id               uuid REFERENCES sd_sla_policies (id) ON DELETE SET NULL,
    policy_name             text NOT NULL,
    -- `{"kind": "always"}` or `{"kind": "weekly", "tz": …, "hours": …}`.
    calendar                jsonb NOT NULL,
    -- `{"p1": {"response": 30, "resolution": 240}, …}` (minutes).
    targets                 jsonb NOT NULL,
    response_due_at         timestamptz,
    response_warn_at        timestamptz,
    response_met            boolean,
    response_warned_at      timestamptz,
    response_breached_at    timestamptz,
    -- Business seconds accrued before `resolution_clock_at`.
    resolution_elapsed_s    bigint NOT NULL DEFAULT 0,
    -- Running since; null while paused or stopped.
    resolution_clock_at     timestamptz,
    resolution_due_at       timestamptz,
    resolution_warn_at      timestamptz,
    resolution_met          boolean,
    resolution_warned_at    timestamptz,
    resolution_breached_at  timestamptz
);
CREATE INDEX sd_ticket_sla_response ON sd_ticket_sla (response_warn_at)
    WHERE response_breached_at IS NULL;
CREATE INDEX sd_ticket_sla_resolution ON sd_ticket_sla (resolution_warn_at)
    WHERE resolution_breached_at IS NULL;

ALTER TABLE sd_sla_assignments ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_sla_assignments_all ON sd_sla_assignments FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
ALTER TABLE sd_ticket_sla ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_ticket_sla_all ON sd_ticket_sla FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- The default: business hours at the client's site.
WITH standard AS (
    INSERT INTO sd_sla_policies (id, name, description, is_default)
    VALUES (gen_random_uuid(), 'Standard',
            'Business hours at the client''s primary site (weekdays 9 to 5 where it has none).', true)
    RETURNING id
)
INSERT INTO sd_sla_targets (policy_id, priority, response_minutes, resolution_minutes)
SELECT id, p, r, s FROM standard,
    (VALUES ('p1', 30, 240), ('p2', 60, 480), ('p3', 240, 1440), ('p4', 480, 2400)) AS t(p, r, s);
