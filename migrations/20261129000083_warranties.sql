-- Warranties (design doc §8.5, §8.7): per asset, from the manufacturer,
-- an extension or a support contract, entered by hand or recorded by a
-- vendor lookup. Expiring and expired warranties are announced once each
-- (asset.warranty_expiring, asset.warranty_expired); changing the end date
-- re-arms them. assets.warranty_ends_on follows the latest warranty.

CREATE TABLE cmdb_warranties (
    id          uuid PRIMARY KEY,
    asset_id    uuid NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    kind        text NOT NULL DEFAULT 'manufacturer' CHECK (kind IN ('manufacturer', 'extended', 'support', 'other')),
    provider    text NOT NULL DEFAULT '' CHECK (length(provider) <= 200),
    -- A contract number, service tag or order reference.
    reference   text NOT NULL DEFAULT '' CHECK (length(reference) <= 200),
    starts_on   date,
    ends_on     date NOT NULL,
    notes       text NOT NULL DEFAULT '' CHECK (length(notes) <= 2000),
    -- manual, or the lookup that recorded it (e.g. dell).
    source      text NOT NULL DEFAULT 'manual' CHECK (source ~ '^[a-z][a-z0-9_]{0,31}$'),
    warned_at   timestamptz,
    expired_at  timestamptz,
    created_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    CHECK (starts_on IS NULL OR starts_on <= ends_on)
);
CREATE INDEX cmdb_warranties_asset ON cmdb_warranties (asset_id, ends_on DESC);
CREATE INDEX cmdb_warranties_ends ON cmdb_warranties (ends_on) WHERE expired_at IS NULL;
-- A lookup records each of its warranties once (by reference).
CREATE UNIQUE INDEX cmdb_warranties_lookup ON cmdb_warranties (asset_id, source, reference) WHERE source <> 'manual';
ALTER TABLE cmdb_warranties ENABLE ROW LEVEL SECURITY;
CREATE POLICY cmdb_warranties_all ON cmdb_warranties FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
