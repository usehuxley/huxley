-- Key-encryption keys can live in a key management service (ADR-018). Their
-- ids are `<provider>:<key path>` (e.g. `vault:transit/huxley`) rather than a
-- 16-hex-digit fingerprint of a configured key.

ALTER TABLE data_keys DROP CONSTRAINT data_keys_kek_id_check;
ALTER TABLE data_keys ADD CONSTRAINT data_keys_kek_id_check CHECK (
    kek_id ~ '^[0-9a-f]{16}$'
    OR kek_id ~ '^[a-z]+:[A-Za-z0-9._/@:-]{1,200}$'
);
