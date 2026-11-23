-- Envelope encryption data keys (design doc §12, ADR-018).
--
-- Each provider organization has its own data-encryption keys (DEKs), so MSP
-- brands under a common owner don't share keys. A DEK is stored only wrapped
-- (AES-256-GCM) by a key-encryption key (KEK) that never touches the database.
-- `kek_id` is a fingerprint of the KEK that wrapped it, so retired KEKs can
-- still unwrap old rows until `huxley admin rewrap-keys` moves them over.

CREATE TYPE data_key_status AS ENUM ('active', 'retired');

CREATE TABLE data_keys (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    status       data_key_status NOT NULL DEFAULT 'active',
    kek_id       text NOT NULL CHECK (kek_id ~ '^[0-9a-f]{16}$'),
    wrapped_key  bytea NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    rewrapped_at timestamptz,
    retired_at   timestamptz
);

-- One active DEK per provider org; older ones stay for decryption.
CREATE UNIQUE INDEX data_keys_one_active ON data_keys (org_id) WHERE status = 'active';
CREATE INDEX data_keys_kek ON data_keys (kek_id);

-- Only provider orgs own keys.
CREATE FUNCTION huxley_data_key_check_org() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM organizations WHERE id = NEW.org_id AND kind = 'provider') THEN
        RAISE EXCEPTION 'data keys belong to provider organizations'
            USING ERRCODE = 'check_violation', CONSTRAINT = 'data_keys_provider_org';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER data_keys_check_org
    BEFORE INSERT OR UPDATE OF org_id ON data_keys
    FOR EACH ROW EXECUTE FUNCTION huxley_data_key_check_org();

ALTER TABLE data_keys ENABLE ROW LEVEL SECURITY;
CREATE POLICY data_keys_all ON data_keys FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));

-- Keys are never deleted by the application: losing one loses its data.
REVOKE DELETE ON data_keys FROM huxley_app;
