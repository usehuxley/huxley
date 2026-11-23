-- Custom fields and tags, shared by every entity type (design doc §8, §14).
-- Organizations are the first entity type; modules add theirs.

CREATE TYPE custom_field_type AS ENUM (
    'text', 'long_text', 'number', 'boolean', 'date', 'select', 'multi_select', 'url', 'email'
);

-- A field for one entity type. With scope_org_id it applies only to entities
-- owned by that org or its descendants (e.g. a field only one client needs).
CREATE TABLE custom_field_defs (
    id            uuid PRIMARY KEY,
    entity_type   text NOT NULL CHECK (entity_type ~ '^[a-z][a-z_]*$'),
    -- The JSON key values are stored under. Never reused, even after archiving.
    key           text NOT NULL CHECK (key ~ '^[a-z][a-z0-9_]{0,62}$'),
    label         text NOT NULL CHECK (length(btrim(label)) BETWEEN 1 AND 100),
    help          text NOT NULL DEFAULT '' CHECK (length(help) <= 500),
    field_type    custom_field_type NOT NULL,
    -- For select and multi_select: [{"value": "...", "label": "..."}].
    options       jsonb NOT NULL DEFAULT '[]'::jsonb,
    required      boolean NOT NULL DEFAULT false,
    scope_org_id  uuid REFERENCES organizations (id) ON DELETE CASCADE,
    position      integer NOT NULL DEFAULT 0,
    archived_at   timestamptz,
    version       integer NOT NULL DEFAULT 1,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT custom_field_defs_key UNIQUE (entity_type, key)
);

CREATE TRIGGER custom_field_defs_touch
    BEFORE UPDATE ON custom_field_defs
    FOR EACH ROW EXECUTE FUNCTION huxley_touch_updated_at();

-- Values live on the entity row, keyed by field key.
ALTER TABLE organizations ADD COLUMN custom_fields jsonb NOT NULL DEFAULT '{}'::jsonb;

CREATE TABLE tags (
    id           uuid PRIMARY KEY,
    name         text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 50),
    color        text NOT NULL DEFAULT 'neutral'
                 CHECK (color IN ('neutral', 'accent', 'success', 'warning', 'danger', 'info')),
    description  text NOT NULL DEFAULT '' CHECK (length(description) <= 200),
    created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX tags_name_unique ON tags (lower(name));

-- A tag on an entity. org_id is the entity's owning org, for RLS.
CREATE TABLE entity_tags (
    tag_id       uuid NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    entity_type  text NOT NULL,
    entity_id    uuid NOT NULL,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    created_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    PRIMARY KEY (tag_id, entity_type, entity_id)
);

CREATE INDEX entity_tags_entity ON entity_tags (entity_type, entity_id);

ALTER TABLE entity_tags ENABLE ROW LEVEL SECURITY;
CREATE POLICY entity_tags_all ON entity_tags FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
