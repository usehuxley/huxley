-- Knowledge base articles linked to tickets (design doc §8.5): what a
-- ticket was solved with, and where an article was used. A link belongs to
-- the ticket's organization.

CREATE TABLE kb_article_tickets (
    article_id  uuid NOT NULL REFERENCES kb_articles (id) ON DELETE CASCADE,
    ticket_id   uuid NOT NULL REFERENCES sd_tickets (id) ON DELETE CASCADE,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    linked_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (article_id, ticket_id)
);
CREATE INDEX kb_article_tickets_ticket ON kb_article_tickets (ticket_id);
ALTER TABLE kb_article_tickets ENABLE ROW LEVEL SECURITY;
CREATE POLICY kb_article_tickets_all ON kb_article_tickets FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
