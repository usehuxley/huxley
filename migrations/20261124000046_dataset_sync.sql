-- Dataset syncs (design doc §9): a dataset can sync itself from an
-- integration list action on a schedule, per organization.
ALTER TABLE datasets
    ADD COLUMN sync jsonb,
    ADD COLUMN sync_next_at timestamptz,
    ADD COLUMN sync_last_at timestamptz;

CREATE INDEX datasets_sync_due ON datasets (sync_next_at) WHERE sync IS NOT NULL;

-- Whether organization `child` is `ancestor` or below it, whatever the
-- caller's scope (a client's run writing to a dataset of its MSP can't see
-- the MSP's row). Reveals only that one relation.
CREATE FUNCTION huxley_org_within(child uuid, ancestor uuid) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp AS $$
    SELECT EXISTS (
        SELECT 1 FROM organizations c, organizations a
         WHERE c.id = child AND a.id = ancestor AND c.path <@ a.path)
$$;

REVOKE EXECUTE ON FUNCTION huxley_org_within(uuid, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION huxley_org_within(uuid, uuid) TO huxley_app;
