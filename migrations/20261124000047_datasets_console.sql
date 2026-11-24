-- The datasets SQL console (design doc §9): read-only queries by instance
-- admins, through a separate login that is a member of this role only.
-- (Running them on the app's own connection isn't safe: SQL can call
-- set_config('role', …), which Postgres checks against the session user.)

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'huxley_datasets_reader') THEN
        CREATE ROLE huxley_datasets_reader NOLOGIN;
    END IF;
END
$$;

GRANT USAGE ON SCHEMA datasets TO huxley_datasets_reader;

-- Existing dataset tables.
DO $$
DECLARE t text;
BEGIN
    FOR t IN SELECT tablename FROM pg_tables WHERE schemaname = 'datasets' LOOP
        EXECUTE format('GRANT SELECT ON datasets.%I TO huxley_datasets_reader', t);
        IF NOT EXISTS (SELECT 1 FROM pg_policies WHERE schemaname = 'datasets' AND tablename = t AND policyname = 'console_reads') THEN
            EXECUTE format('CREATE POLICY console_reads ON datasets.%I FOR SELECT TO huxley_datasets_reader USING (true)', t);
        END IF;
    END LOOP;
END
$$;

-- New ones.
CREATE OR REPLACE FUNCTION huxley_dataset_apply(dataset_id uuid) RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp AS $$
DECLARE
    tbl text := 'ds_' || replace(dataset_id::text, '-', '');
    def record;
    col jsonb;
    col_name text;
    col_type text;
    existing text;
    wanted text[] := '{}';
    key_cols text := '';
    k text;
BEGIN
    -- Definer rights skip row security: check the caller's scope here.
    SELECT columns, primary_key INTO def FROM datasets
     WHERE id = dataset_id AND huxley_org_in_scope(org_id);
    IF NOT FOUND THEN
        RAISE EXCEPTION 'no dataset %', dataset_id;
    END IF;
    IF jsonb_typeof(def.columns) <> 'array' OR jsonb_array_length(def.columns) > 200 THEN
        RAISE EXCEPTION 'columns must be a list of at most 200';
    END IF;

    IF to_regclass('datasets.' || tbl) IS NULL THEN
        EXECUTE format(
            'CREATE TABLE datasets.%I (_org_id uuid NOT NULL REFERENCES public.organizations (id) ON DELETE CASCADE, '
            '_synced_at timestamptz NOT NULL DEFAULT now())', tbl);
        EXECUTE format('ALTER TABLE datasets.%I ENABLE ROW LEVEL SECURITY', tbl);
        EXECUTE format(
            'CREATE POLICY rows_in_scope ON datasets.%I FOR ALL TO huxley_app '
            'USING (public.huxley_org_in_scope(_org_id)) WITH CHECK (public.huxley_org_in_scope(_org_id))', tbl);
        EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON datasets.%I TO huxley_app', tbl);
        -- The SQL console's role reads every row (only instance admins use it).
        EXECUTE format('GRANT SELECT ON datasets.%I TO huxley_datasets_reader', tbl);
        EXECUTE format('CREATE POLICY console_reads ON datasets.%I FOR SELECT TO huxley_datasets_reader USING (true)', tbl);
        EXECUTE format('CREATE INDEX %I ON datasets.%I (_org_id)', tbl || '_org', tbl);
    END IF;

    FOR col IN SELECT * FROM jsonb_array_elements(def.columns) LOOP
        col_name := col->>'name';
        col_type := huxley_dataset_sql_type(col->>'type');
        IF col_name IS NULL OR col_name !~ '^[a-z][a-z0-9_]{0,62}$' THEN
            RAISE EXCEPTION 'invalid column name %', col_name;
        END IF;
        IF col_type IS NULL THEN
            RAISE EXCEPTION 'invalid type % for column %', col->>'type', col_name;
        END IF;
        IF col_name = ANY (wanted) THEN
            RAISE EXCEPTION 'column % appears twice', col_name;
        END IF;
        wanted := wanted || col_name;
        SELECT format_type(a.atttypid, a.atttypmod) INTO existing
          FROM pg_attribute a
         WHERE a.attrelid = ('datasets.' || tbl)::regclass AND a.attname = col_name
           AND a.attnum > 0 AND NOT a.attisdropped;
        IF existing IS NULL THEN
            EXECUTE format('ALTER TABLE datasets.%I ADD COLUMN %I %s', tbl, col_name, col_type);
        ELSIF existing <> col_type THEN
            EXECUTE format('ALTER TABLE datasets.%I ALTER COLUMN %I TYPE %s USING %I::%s',
                           tbl, col_name, col_type, col_name, col_type);
        END IF;
    END LOOP;

    -- Columns no longer defined go (reserved _ columns stay).
    FOR existing IN
        SELECT a.attname FROM pg_attribute a
         WHERE a.attrelid = ('datasets.' || tbl)::regclass AND a.attnum > 0 AND NOT a.attisdropped
           AND left(a.attname, 1) <> '_' AND NOT (a.attname = ANY (wanted))
    LOOP
        EXECUTE format('ALTER TABLE datasets.%I DROP COLUMN %I', tbl, existing);
    END LOOP;

    -- The key: unique per organization.
    EXECUTE format('DROP INDEX IF EXISTS datasets.%I', tbl || '_key');
    IF cardinality(def.primary_key) > 0 THEN
        FOREACH k IN ARRAY def.primary_key LOOP
            IF NOT (k = ANY (wanted)) THEN
                RAISE EXCEPTION 'key column % isn''t a column', k;
            END IF;
            key_cols := key_cols || ', ' || quote_ident(k);
        END LOOP;
        EXECUTE format('CREATE UNIQUE INDEX %I ON datasets.%I (_org_id%s)', tbl || '_key', tbl, key_cols);
    END IF;
END
$$;
