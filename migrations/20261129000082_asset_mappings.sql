-- Dataset → asset mapping rules (design doc §8.5 "Discovery and sync"):
-- rows of a dataset (an RMM's devices, Microsoft 365's tenants) become
-- assets of the row's organization. Each rule matches existing assets by
-- keys in order (external id, serial number, hostname, name), maps columns
-- to asset fields and type attributes, and has a conflict policy. Each
-- asset field remembers where its value came from.

CREATE TABLE cmdb_mappings (
    id              uuid PRIMARY KEY,
    dataset_id      uuid NOT NULL REFERENCES datasets (id) ON DELETE CASCADE,
    name            text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    -- The source's name: the key under assets.external_ids, and the field
    -- provenance (e.g. "rmm", "m365").
    source          text NOT NULL CHECK (source ~ '^[a-z][a-z0-9_]{0,31}$'),
    -- The type of assets it creates.
    type_id         uuid NOT NULL REFERENCES cmdb_asset_types (id),
    -- [{"field": "external_id"|"serial_number"|"hostname"|"name", "column": "…"}], in order.
    match_keys      jsonb NOT NULL CHECK (jsonb_typeof(match_keys) = 'array'),
    -- {"name": col, "serial_number": col, "hostname": col, "manufacturer": col,
    --  "model": col, "external_id": col, "attributes": {"key": col}}
    fields          jsonb NOT NULL CHECK (jsonb_typeof(fields) = 'object'),
    -- source_wins: the dataset's values replace what's there; manual_wins:
    -- values someone set by hand stay.
    conflict        text NOT NULL DEFAULT 'manual_wins' CHECK (conflict IN ('source_wins', 'manual_wins')),
    create_missing  boolean NOT NULL DEFAULT true,
    enabled         boolean NOT NULL DEFAULT true,
    last_run_at     timestamptz,
    last_result     jsonb,
    created_by      uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    version         integer NOT NULL DEFAULT 1,
    UNIQUE (dataset_id, source)
);

-- Where each field's value came from: {"field": "manual" | "<source>"}.
ALTER TABLE assets ADD COLUMN field_sources jsonb NOT NULL DEFAULT '{}'
    CHECK (jsonb_typeof(field_sources) = 'object');
ALTER TABLE assets ADD COLUMN synced_at timestamptz;
