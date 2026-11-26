-- Dashboards shown in the client portal (ADR-040, ADR-041): a shared
-- dashboard published there is seen by the client admins of its
-- organization and those below, with their company's numbers only.

ALTER TABLE dashboards ADD COLUMN portal boolean NOT NULL DEFAULT false;
ALTER TABLE dashboards ADD CONSTRAINT dashboards_portal_shared CHECK (NOT portal OR owner_user_id IS NULL);
