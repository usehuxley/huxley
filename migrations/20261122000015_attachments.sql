-- Attachments on records (design doc §8, §14). The bytes live in object
-- storage under storage_key; this row holds the metadata. org_id is the
-- record's owning org, for RLS.

CREATE TABLE attachments (
    id            uuid PRIMARY KEY,
    entity_type   text NOT NULL,
    entity_id     uuid NOT NULL,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    filename      text NOT NULL CHECK (length(filename) BETWEEN 1 AND 255),
    -- As the uploader declared it. Downloads only pass a few safe types through.
    content_type  text NOT NULL,
    size_bytes    bigint NOT NULL CHECK (size_bytes >= 0),
    sha256        bytea NOT NULL,
    storage_key   text NOT NULL UNIQUE,
    created_by    uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    deleted_at    timestamptz
);

CREATE INDEX attachments_entity ON attachments (entity_type, entity_id) WHERE deleted_at IS NULL;

ALTER TABLE attachments ENABLE ROW LEVEL SECURITY;
CREATE POLICY attachments_all ON attachments FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
