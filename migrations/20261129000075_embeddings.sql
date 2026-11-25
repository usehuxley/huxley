-- Embeddings (Phase 5; design doc §10.1): an organization may set an
-- embeddings provider next to its AI provider (Anthropic has no embeddings
-- API, so it's often another vendor), and tickets are embedded for search
-- by meaning.
--
-- pgvector stays optional. Embeddings are kept as real[] whether or not it
-- is installed; searching by meaning needs it (real[] casts to vector), and
-- switches itself off without it. Installing pgvector later just works.

ALTER TABLE ai_providers ADD COLUMN purpose text NOT NULL DEFAULT 'chat'
    CHECK (purpose IN ('chat', 'embeddings'));
ALTER TABLE ai_providers DROP CONSTRAINT ai_providers_org_id_key;
ALTER TABLE ai_providers ADD CONSTRAINT ai_providers_org_purpose UNIQUE (org_id, purpose);
ALTER TABLE ai_providers DROP CONSTRAINT ai_providers_kind_check;
ALTER TABLE ai_providers ADD CONSTRAINT ai_providers_kind_check CHECK (
    kind IN ('anthropic', 'openai', 'azure_openai', 'openai_compatible', 'voyage')
    -- Voyage only embeds; Anthropic doesn't.
    AND NOT (purpose = 'chat' AND kind = 'voyage')
    AND NOT (purpose = 'embeddings' AND kind = 'anthropic')
);

-- pgvector, where it's available and we may install it.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = 'vector') THEN
        CREATE EXTENSION IF NOT EXISTS vector;
    END IF;
EXCEPTION WHEN insufficient_privilege THEN
    RAISE NOTICE 'pgvector is available but this role may not install it: searching by meaning stays off';
END
$$;

-- A ticket's embedding: of its title and description, by `model`. Stale
-- when the hash of model, title and description no longer matches, or the
-- ticket moved organization.
CREATE TABLE sd_ticket_embeddings (
    ticket_id     uuid PRIMARY KEY REFERENCES sd_tickets (id) ON DELETE CASCADE,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    model         text NOT NULL,
    content_hash  text NOT NULL,
    embedding     real[] NOT NULL CHECK (cardinality(embedding) BETWEEN 1 AND 16000),
    embedded_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sd_ticket_embeddings_org ON sd_ticket_embeddings (org_id, model);
ALTER TABLE sd_ticket_embeddings ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_ticket_embeddings_all ON sd_ticket_embeddings FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
