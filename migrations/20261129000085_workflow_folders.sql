-- Folders to organize workflows (per organization, nestable). An
-- organization's System folder holds the workflows installed from Huxley's
-- built-in packs; it can't be renamed, moved or deleted, and holds nothing
-- else (enforced by the API).

CREATE TABLE workflow_folders (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    parent_id   uuid REFERENCES workflow_folders (id) ON DELETE RESTRICT,
    name        text NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    system      boolean NOT NULL DEFAULT false,
    created_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    CHECK (NOT system OR parent_id IS NULL)
);

-- Names are unique among siblings (any case).
CREATE UNIQUE INDEX workflow_folders_name
    ON workflow_folders (org_id, COALESCE(parent_id, '00000000-0000-0000-0000-000000000000'), lower(name));
CREATE UNIQUE INDEX workflow_folders_system ON workflow_folders (org_id) WHERE system;
CREATE INDEX workflow_folders_parent ON workflow_folders (parent_id);

ALTER TABLE workflow_folders ENABLE ROW LEVEL SECURITY;
CREATE POLICY workflow_folders_all ON workflow_folders FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- system: installed from a built-in pack (kept in the System folder).
ALTER TABLE workflows
    ADD COLUMN folder_id uuid REFERENCES workflow_folders (id) ON DELETE SET NULL,
    ADD COLUMN system boolean NOT NULL DEFAULT false;
CREATE INDEX workflows_folder ON workflows (folder_id);

-- Built-in workflows already installed move into their organization's
-- System folder.
UPDATE workflows w SET system = true
  FROM pack_installs p, jsonb_each_text(COALESCE(p.items -> 'workflows', '{}')) AS e (key, id)
 WHERE p.trust = 'built_in' AND w.id = e.id::uuid AND w.org_id = p.org_id;

INSERT INTO workflow_folders (id, org_id, name, system)
SELECT gen_random_uuid(), org_id, 'System', true
  FROM workflows WHERE system GROUP BY org_id;

UPDATE workflows w SET folder_id = f.id
  FROM workflow_folders f
 WHERE w.system AND f.org_id = w.org_id AND f.system;
