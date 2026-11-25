-- Assets and configuration items (design doc §8.5, the `cmdb` module):
-- typed assets with each type's own fields, a lifecycle, owner, site and
-- warranty, and typed relationships for dependency mapping and impact
-- analysis.

-- Kinds of asset. Each has its own fields ([{key, label, help, fieldType,
-- options, required}], validated like custom fields); values live in
-- assets.attributes. System types can be extended, not removed.
CREATE TABLE cmdb_asset_types (
    id          uuid PRIMARY KEY,
    key         text NOT NULL UNIQUE CHECK (key ~ '^[a-z][a-z0-9_]{0,62}$'),
    name        text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    fields      jsonb NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(fields) = 'array'),
    is_system   boolean NOT NULL DEFAULT false,
    position    integer NOT NULL DEFAULT 0,
    archived_at timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now(),
    version     integer NOT NULL DEFAULT 1
);

INSERT INTO cmdb_asset_types (id, key, name, fields, is_system, position) VALUES
    (gen_random_uuid(), 'workstation', 'Workstation', '[
        {"key":"os","label":"Operating system","fieldType":"text"},
        {"key":"cpu","label":"Processor","fieldType":"text"},
        {"key":"memory_gb","label":"Memory (GB)","fieldType":"number"},
        {"key":"ip_address","label":"IP address","fieldType":"text"}]', true, 10),
    (gen_random_uuid(), 'server', 'Server', '[
        {"key":"os","label":"Operating system","fieldType":"text"},
        {"key":"role","label":"Role","fieldType":"text","help":"e.g. domain controller, file server"},
        {"key":"virtual","label":"Virtual machine","fieldType":"boolean"},
        {"key":"memory_gb","label":"Memory (GB)","fieldType":"number"},
        {"key":"ip_address","label":"IP address","fieldType":"text"}]', true, 20),
    (gen_random_uuid(), 'network_device', 'Network device', '[
        {"key":"device_role","label":"Role","fieldType":"select","options":[
            {"value":"firewall","label":"Firewall"},{"value":"switch","label":"Switch"},
            {"value":"router","label":"Router"},{"value":"access_point","label":"Access point"}]},
        {"key":"firmware","label":"Firmware","fieldType":"text"},
        {"key":"ip_address","label":"Management IP","fieldType":"text"}]', true, 30),
    (gen_random_uuid(), 'printer', 'Printer', '[
        {"key":"ip_address","label":"IP address","fieldType":"text"}]', true, 40),
    (gen_random_uuid(), 'mobile_device', 'Mobile device', '[
        {"key":"os","label":"Operating system","fieldType":"text"},
        {"key":"phone_number","label":"Phone number","fieldType":"text"}]', true, 50),
    (gen_random_uuid(), 'saas_tenant', 'SaaS tenant', '[
        {"key":"tenant_id","label":"Tenant ID","fieldType":"text"},
        {"key":"primary_domain","label":"Primary domain","fieldType":"text"}]', true, 60),
    (gen_random_uuid(), 'license', 'License', '[
        {"key":"seats","label":"Seats","fieldType":"number"},
        {"key":"expires_on","label":"Expires on","fieldType":"date"}]', true, 70),
    (gen_random_uuid(), 'domain', 'Domain', '[
        {"key":"registrar","label":"Registrar","fieldType":"text"},
        {"key":"expires_on","label":"Expires on","fieldType":"date"}]', true, 80),
    (gen_random_uuid(), 'certificate', 'Certificate', '[
        {"key":"issuer","label":"Issuer","fieldType":"text"},
        {"key":"expires_on","label":"Expires on","fieldType":"date"}]', true, 90),
    (gen_random_uuid(), 'application', 'Application', '[
        {"key":"vendor","label":"Vendor","fieldType":"text"},
        {"key":"app_version","label":"Version","fieldType":"text"}]', true, 100),
    (gen_random_uuid(), 'other', 'Other', '[]', true, 1000);

-- An asset belongs to a client (or the MSP itself).
CREATE TABLE assets (
    id                uuid PRIMARY KEY,
    org_id            uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    type_id           uuid NOT NULL REFERENCES cmdb_asset_types (id),
    name              text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    -- planned, in_stock, active, maintenance, retired, disposed
    status            text NOT NULL DEFAULT 'active'
                      CHECK (status IN ('planned', 'in_stock', 'active', 'maintenance', 'retired', 'disposed')),
    serial_number     text NOT NULL DEFAULT '' CHECK (length(serial_number) <= 200),
    hostname          text NOT NULL DEFAULT '' CHECK (length(hostname) <= 255),
    manufacturer      text NOT NULL DEFAULT '' CHECK (length(manufacturer) <= 200),
    model             text NOT NULL DEFAULT '' CHECK (length(model) <= 200),
    site_id           uuid REFERENCES crm_sites (id) ON DELETE SET NULL,
    -- Who uses or owns it.
    contact_id        uuid REFERENCES crm_contacts (id) ON DELETE SET NULL,
    purchased_on      date,
    warranty_ends_on  date,
    notes             text NOT NULL DEFAULT '' CHECK (length(notes) <= 10000),
    -- The type's fields.
    attributes        jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(attributes) = 'object'),
    -- Custom fields shared by every asset (huxley-entity).
    custom_fields     jsonb NOT NULL DEFAULT '{}',
    -- Ids in other systems (e.g. {"rmm": "…"}), for matching on sync.
    external_ids      jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(external_ids) = 'object'),
    created_by        uuid REFERENCES users (id) ON DELETE SET NULL,
    updated_by        uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    version           integer NOT NULL DEFAULT 1
);
CREATE INDEX assets_org ON assets (org_id, status);
CREATE INDEX assets_type ON assets (type_id);
CREATE INDEX assets_serial ON assets (lower(serial_number)) WHERE serial_number <> '';
CREATE INDEX assets_hostname ON assets (lower(hostname)) WHERE hostname <> '';
CREATE INDEX assets_name_trgm ON assets USING gin (name gin_trgm_ops);
CREATE INDEX assets_external ON assets USING gin (external_ids jsonb_path_ops);
ALTER TABLE assets ENABLE ROW LEVEL SECURITY;
CREATE POLICY assets_all ON assets FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- Typed edges: from `kind` to. A runs_on B, A depends_on B, A connects_to
-- B, A hosts B, A licensed_by B, A backs_up B. They belong to the `from`
-- asset's organization; both ends are in the same organization tree.
CREATE TABLE cmdb_relationships (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    from_id     uuid NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    to_id       uuid NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    kind        text NOT NULL CHECK (kind IN ('runs_on', 'depends_on', 'connects_to', 'hosts', 'licensed_by', 'backs_up')),
    created_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    CHECK (from_id <> to_id),
    UNIQUE (from_id, to_id, kind)
);
CREATE INDEX cmdb_relationships_to ON cmdb_relationships (to_id);
ALTER TABLE cmdb_relationships ENABLE ROW LEVEL SECURITY;
CREATE POLICY cmdb_relationships_all ON cmdb_relationships FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- Technicians keep assets; read-only users see them.
UPDATE roles SET permissions = permissions || '{asset:read,asset:write}'
 WHERE key = 'technician' AND is_system;
UPDATE roles SET permissions = permissions || '{asset:read}'
 WHERE key = 'read_only' AND is_system;
