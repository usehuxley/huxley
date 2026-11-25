-- Published knowledge base articles embedded for search by meaning, and
-- for suggesting articles on a ticket by its own embedding. Like ticket
-- embeddings (migration 075): kept as real[]; searching needs pgvector.
-- Stale when the published title or text, the organization or the
-- provider's model changes.

CREATE TABLE kb_article_embeddings (
    article_id    uuid PRIMARY KEY REFERENCES kb_articles (id) ON DELETE CASCADE,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    model         text NOT NULL,
    content_hash  text NOT NULL,
    embedding     real[] NOT NULL CHECK (cardinality(embedding) BETWEEN 1 AND 16000),
    embedded_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX kb_article_embeddings_org ON kb_article_embeddings (org_id, model);
ALTER TABLE kb_article_embeddings ENABLE ROW LEVEL SECURITY;
CREATE POLICY kb_article_embeddings_all ON kb_article_embeddings FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
