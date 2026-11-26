-- Net promoter surveys (design doc §8.9, ADR-043): contacts asked, every so
-- often, how likely they are to recommend the MSP (0 to 10).

CREATE TABLE cx_nps (
    id            uuid PRIMARY KEY,
    -- The contact's company.
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    contact_id    uuid NOT NULL REFERENCES crm_contacts (id) ON DELETE CASCADE,
    -- Which round it's from, e.g. 2026-11.
    campaign      text NOT NULL CHECK (length(campaign) BETWEEN 1 AND 60),
    token_hash    bytea NOT NULL UNIQUE,
    emailed       boolean NOT NULL DEFAULT false,
    score         smallint CHECK (score BETWEEN 0 AND 10),
    comment       text CHECK (length(comment) <= 4000),
    channel       text CHECK (channel IN ('email', 'portal')),
    sent_at       timestamptz NOT NULL DEFAULT now(),
    responded_at  timestamptz,
    expires_at    timestamptz NOT NULL
);
CREATE INDEX cx_nps_contact ON cx_nps (contact_id, sent_at DESC);
CREATE INDEX cx_nps_org ON cx_nps (org_id, sent_at DESC);
ALTER TABLE cx_nps ENABLE ROW LEVEL SECURITY;
CREATE POLICY cx_nps_all ON cx_nps FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
