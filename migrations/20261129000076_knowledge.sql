-- The knowledge base (design doc §8.5): articles with revisions, a review
-- flow, categories and full-text search. An article belongs to an
-- organization (the MSP's for shared know-how, a client's for its own
-- documentation) and is seen by those who may read the knowledge base
-- there.

CREATE TABLE kb_categories (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name        text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    description text NOT NULL DEFAULT '' CHECK (length(description) <= 500),
    position    integer NOT NULL DEFAULT 0,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX kb_categories_name ON kb_categories (org_id, lower(name));
ALTER TABLE kb_categories ENABLE ROW LEVEL SECURITY;
CREATE POLICY kb_categories_all ON kb_categories FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- Tags as text, for the search column (array_to_string isn't marked
-- immutable, though for text it is).
CREATE FUNCTION kb_tags_text(tags text[]) RETURNS text
    LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$ SELECT array_to_string(tags, ' ') $$;

-- An article's working copy (title, body) is its latest revision. Readers
-- see the published revision; editing a published article makes a draft
-- next to it until that is published.
CREATE TABLE kb_articles (
    id                 uuid PRIMARY KEY,
    org_id             uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    category_id        uuid REFERENCES kb_categories (id) ON DELETE SET NULL,
    title              text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 300),
    body               text NOT NULL DEFAULT '' CHECK (length(body) <= 200000),
    -- internal: technicians only; clients: also the organization's portal
    -- contacts (Phase 7); public: anyone on the portal.
    visibility         text NOT NULL DEFAULT 'internal' CHECK (visibility IN ('internal', 'clients', 'public')),
    status             text NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'in_review', 'published', 'archived')),
    tags               text[] NOT NULL DEFAULT '{}' CHECK (cardinality(tags) <= 20),
    revision           integer NOT NULL DEFAULT 1,
    published_revision integer,
    published_at       timestamptz,
    review_requested_by uuid REFERENCES users (id) ON DELETE SET NULL,
    created_by         uuid REFERENCES users (id) ON DELETE SET NULL,
    updated_by         uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now(),
    version            integer NOT NULL DEFAULT 1,
    search             tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('english', title), 'A')
        || setweight(to_tsvector('english', kb_tags_text(tags)), 'B')
        || setweight(to_tsvector('english', body), 'C')
    ) STORED,
    CHECK (published_revision IS NULL OR published_revision <= revision)
);
CREATE INDEX kb_articles_org ON kb_articles (org_id, status);
CREATE INDEX kb_articles_category ON kb_articles (category_id);
CREATE INDEX kb_articles_search ON kb_articles USING gin (search);
CREATE INDEX kb_articles_title_trgm ON kb_articles USING gin (title gin_trgm_ops);
ALTER TABLE kb_articles ENABLE ROW LEVEL SECURITY;
CREATE POLICY kb_articles_all ON kb_articles FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- Every saved change of title or body.
CREATE TABLE kb_revisions (
    article_id  uuid NOT NULL REFERENCES kb_articles (id) ON DELETE CASCADE,
    number      integer NOT NULL,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    title       text NOT NULL,
    body        text NOT NULL,
    created_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (article_id, number)
);
ALTER TABLE kb_revisions ENABLE ROW LEVEL SECURITY;
CREATE POLICY kb_revisions_all ON kb_revisions FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- What happened to an article: created, edited, submitted, approved,
-- rejected (with a comment), published, archived, restored.
CREATE TABLE kb_activity (
    id          uuid PRIMARY KEY,
    -- Order of what happened (many changes share a transaction's now()).
    seq         bigint GENERATED ALWAYS AS IDENTITY,
    article_id  uuid NOT NULL REFERENCES kb_articles (id) ON DELETE CASCADE,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    actor_id    uuid REFERENCES users (id) ON DELETE SET NULL,
    kind        text NOT NULL,
    details     jsonb NOT NULL DEFAULT '{}',
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX kb_activity_article ON kb_activity (article_id, seq);
ALTER TABLE kb_activity ENABLE ROW LEVEL SECURITY;
CREATE POLICY kb_activity_all ON kb_activity FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- Technicians read and write articles; read-only users read them. Who
-- publishes is up to each MSP (owners and admins hold `*`).
UPDATE roles SET permissions = permissions || '{kb:read,kb:write}'
 WHERE key = 'technician' AND is_system;
UPDATE roles SET permissions = permissions || '{kb:read}'
 WHERE key = 'read_only' AND is_system;
