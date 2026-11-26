-- Problem management (design doc §8.6, ADR-055): a problem ticket's root
-- cause, workaround and known-error status, and its incidents (ticket
-- links: an incident's `problem`, a problem's `incident`s). The workaround
-- can be drafted into the knowledge base.

ALTER TABLE sd_ticket_links DROP CONSTRAINT sd_ticket_links_kind_check;
ALTER TABLE sd_ticket_links ADD CONSTRAINT sd_ticket_links_kind_check
    CHECK (kind IN ('related', 'parent', 'child', 'problem', 'incident'));

CREATE TABLE sd_problems (
    ticket_id       uuid PRIMARY KEY REFERENCES sd_tickets (id) ON DELETE CASCADE,
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    root_cause      text NOT NULL DEFAULT '' CHECK (length(root_cause) <= 20000),
    workaround      text NOT NULL DEFAULT '' CHECK (length(workaround) <= 20000),
    -- The cause is known and there's a workaround.
    known_error     boolean NOT NULL DEFAULT false,
    known_error_at  timestamptz,
    -- The article drafted from the workaround.
    kb_article_id   uuid REFERENCES kb_articles (id) ON DELETE SET NULL,
    version         integer NOT NULL DEFAULT 1,
    CHECK (NOT known_error OR (root_cause <> '' AND workaround <> ''))
);
ALTER TABLE sd_problems ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_problems_all ON sd_problems FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
