-- Linked and merged tickets (design doc §8.2).

-- A merged ticket is closed and points at the one it was merged into.
ALTER TABLE sd_tickets ADD COLUMN merged_into_id uuid REFERENCES sd_tickets (id) ON DELETE SET NULL;

-- Links, one row per side so each side's organization governs who sees it:
-- `other` is this ticket's `kind` (related, its parent, or its child).
CREATE TABLE sd_ticket_links (
    ticket_id  uuid NOT NULL REFERENCES sd_tickets (id) ON DELETE CASCADE,
    other_id   uuid NOT NULL REFERENCES sd_tickets (id) ON DELETE CASCADE,
    org_id     uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    kind       text NOT NULL CHECK (kind IN ('related', 'parent', 'child')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (ticket_id, other_id),
    CHECK (ticket_id <> other_id)
);
-- At most one parent.
CREATE UNIQUE INDEX sd_ticket_links_one_parent ON sd_ticket_links (ticket_id) WHERE kind = 'parent';
ALTER TABLE sd_ticket_links ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_ticket_links_all ON sd_ticket_links FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
