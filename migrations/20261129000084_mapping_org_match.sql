-- Asset mappings can place each row with the organization whose variable
-- matches one of its columns (e.g. an RMM's client id against each client's
-- automate_client_id), instead of the organization the row was written for.
-- {"column": "...", "variable": "..."}; NULL: the row's own organization.
ALTER TABLE cmdb_mappings ADD COLUMN org_match jsonb
    CHECK (org_match IS NULL OR jsonb_typeof(org_match) = 'object');
