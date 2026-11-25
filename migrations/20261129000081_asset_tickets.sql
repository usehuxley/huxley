-- Assets linked to tickets (design doc §8.5): what a ticket is about, and
-- an asset's ticket history. A link belongs to the ticket's organization.

CREATE TABLE cmdb_asset_tickets (
    asset_id    uuid NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    ticket_id   uuid NOT NULL REFERENCES sd_tickets (id) ON DELETE CASCADE,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    linked_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (asset_id, ticket_id)
);
CREATE INDEX cmdb_asset_tickets_ticket ON cmdb_asset_tickets (ticket_id);
ALTER TABLE cmdb_asset_tickets ENABLE ROW LEVEL SECURITY;
CREATE POLICY cmdb_asset_tickets_all ON cmdb_asset_tickets FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
