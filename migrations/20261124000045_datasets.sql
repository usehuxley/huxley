-- Datasets (design doc §9, ADR-029): definitions here, rows in managed
-- tables in the `datasets` schema whose DDL the database itself generates.

CREATE TABLE datasets (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    key          text NOT NULL CHECK (key ~ '^[a-z][a-z0-9-]{0,62}$'),
    name         text NOT NULL,
    description  text,
    -- [{ "name": "upn", "type": "text" }, …]
    columns      jsonb NOT NULL,
    primary_key  text[] NOT NULL DEFAULT '{}',
    created_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, key)
);

ALTER TABLE datasets ENABLE ROW LEVEL SECURITY;
CREATE POLICY datasets_all ON datasets FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));

-- A dataset column type's SQL type, or NULL if it isn't one.
CREATE FUNCTION huxley_dataset_sql_type(kind text) RETURNS text
LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE kind
        WHEN 'text' THEN 'text'
        WHEN 'integer' THEN 'bigint'
        WHEN 'number' THEN 'double precision'
        WHEN 'boolean' THEN 'boolean'
        WHEN 'timestamp' THEN 'timestamp with time zone'
        WHEN 'date' THEN 'date'
        WHEN 'json' THEN 'jsonb'
        WHEN 'uuid' THEN 'uuid'
    END
$$;

-- Creates dataset `dataset_id`'s table, or brings it in line with its
-- definition (columns added, dropped, retyped; key index rebuilt). Every
-- name and type is checked here, and identifiers are quoted with %I.
-- SECURITY DEFINER: the app role can't run DDL.
CREATE FUNCTION huxley_dataset_apply(dataset_id uuid) RETURNS void
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

-- Drops dataset `dataset_id`'s table (its name comes from the id alone).
-- Call it before deleting the definition: the caller must see that.
CREATE FUNCTION huxley_dataset_drop(dataset_id uuid) RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM datasets WHERE id = dataset_id AND huxley_org_in_scope(org_id)) THEN
        RAISE EXCEPTION 'no dataset %', dataset_id;
    END IF;
    EXECUTE format('DROP TABLE IF EXISTS datasets.%I', 'ds_' || replace(dataset_id::text, '-', ''));
END
$$;

REVOKE EXECUTE ON FUNCTION huxley_dataset_apply(uuid) FROM PUBLIC;
REVOKE EXECUTE ON FUNCTION huxley_dataset_drop(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION huxley_dataset_apply(uuid) TO huxley_app;
GRANT EXECUTE ON FUNCTION huxley_dataset_drop(uuid) TO huxley_app;

UPDATE roles SET permissions = permissions || '{dataset:read}'
 WHERE key IN ('technician', 'read_only') AND is_system;
