-- Presales scoping (design doc §8.10, ADR-072): a workspace on an
-- opportunity where the work is scoped before it's quoted. Discovery
-- (notes, transcripts, documents' text) feeds a Scope of Work whose
-- sections come from a template per kind of engagement. AI drafts the
-- sections (through the Presales scoping pack), citing what it drew on and
-- turning gaps into open questions; people edit and approve. Approved
-- versions are attached to, and signed with, the quote.

CREATE TABLE sales_scopes (
    id              uuid PRIMARY KEY,
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    opportunity_id  uuid NOT NULL UNIQUE REFERENCES sales_opportunities (id) ON DELETE CASCADE,
    template        text NOT NULL,
    -- idle, requested (waiting for the drafting workflow), or failed.
    draft_state     text NOT NULL DEFAULT 'idle' CHECK (draft_state IN ('idle', 'requested', 'failed')),
    draft_error     text CHECK (length(draft_error) <= 2000),
    draft_requested_at timestamptz,
    drafted_at      timestamptz,
    version         integer NOT NULL DEFAULT 1,
    created_by      uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE sales_scope_sources (
    id          uuid PRIMARY KEY,
    scope_id    uuid NOT NULL REFERENCES sales_scopes (id) ON DELETE CASCADE,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    kind        text NOT NULL CHECK (kind IN ('note', 'transcript', 'document')),
    title       text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 200),
    body        text NOT NULL CHECK (length(body) BETWEEN 1 AND 100000),
    created_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sales_scope_sources_scope ON sales_scope_sources (scope_id, created_at);

CREATE TABLE sales_scope_sections (
    id          uuid PRIMARY KEY,
    scope_id    uuid NOT NULL REFERENCES sales_scopes (id) ON DELETE CASCADE,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    key         text NOT NULL CHECK (key ~ '^[a-z][a-z0-9_]{0,39}$'),
    title       text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 100),
    position    integer NOT NULL,
    body        text NOT NULL DEFAULT '' CHECK (length(body) <= 20000),
    -- What the AI drew on: [{label, kind}].
    citations   jsonb NOT NULL DEFAULT '[]',
    -- The text is the AI's, untouched since.
    drafted     boolean NOT NULL DEFAULT false,
    updated_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    updated_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (scope_id, key)
);

CREATE TABLE sales_scope_questions (
    id           uuid PRIMARY KEY,
    scope_id     uuid NOT NULL REFERENCES sales_scopes (id) ON DELETE CASCADE,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    section_key  text,
    question     text NOT NULL CHECK (length(btrim(question)) BETWEEN 1 AND 1000),
    answer       text CHECK (length(answer) <= 4000),
    from_ai      boolean NOT NULL DEFAULT false,
    resolved_at  timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sales_scope_questions_scope ON sales_scope_questions (scope_id, created_at);

-- An approved Scope of Work, as it was.
CREATE TABLE sales_scope_versions (
    id          uuid PRIMARY KEY,
    scope_id    uuid NOT NULL REFERENCES sales_scopes (id) ON DELETE CASCADE,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    number      integer NOT NULL,
    snapshot    jsonb NOT NULL,
    created_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (scope_id, number)
);

-- The approved Scope of Work a quote carries (and is signed with).
ALTER TABLE sales_quotes ADD COLUMN scope_version_id uuid REFERENCES sales_scope_versions (id) ON DELETE SET NULL;

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['sales_scopes', 'sales_scope_sources', 'sales_scope_sections',
                             'sales_scope_questions', 'sales_scope_versions'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY %I ON %I FOR ALL TO huxley_app
               USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
               WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))',
            t || '_all', t);
    END LOOP;
END $$;
