-- Row-level security that scales with the organization tree.
--
-- Policies called huxley_org_in_scope(org) on every row, which parsed the
-- whole scope (one id per organization the caller can reach) each time:
-- quadratic, so a user scoped to 4,000 organizations waited over 5 seconds
-- to list them. Policies now read the scope through uncorrelated
-- subqueries, which Postgres evaluates once per query (InitPlans): about
-- 40 ms for the same query. huxley_org_in_scope stays, for the few places
-- that call it once.

-- Whether the scope is everything (system work).
CREATE OR REPLACE FUNCTION huxley_scope_all() RETURNS boolean
LANGUAGE sql STABLE AS $$
    SELECT coalesce(current_setting('huxley.org_scope', true), '') = '*'
$$;

-- The scope's organization ids (empty for none, and for everything).
CREATE OR REPLACE FUNCTION huxley_scope_ids() RETURNS uuid[]
LANGUAGE sql STABLE AS $$
    SELECT CASE
        WHEN coalesce(current_setting('huxley.org_scope', true), '') IN ('', '*') THEN '{}'::uuid[]
        ELSE string_to_array(current_setting('huxley.org_scope', true), ',')::uuid[]
    END
$$;

-- The policy expression for "column `col` is in scope", in the fast form.
CREATE OR REPLACE FUNCTION huxley_scope_check(col text) RETURNS text
LANGUAGE sql IMMUTABLE AS $$
    SELECT format('((SELECT public.huxley_scope_all()) OR %s = ANY ((SELECT public.huxley_scope_ids())::uuid[]))', col)
$$;

-- Rewrite every policy that calls huxley_org_in_scope(<column>).
DO $$
DECLARE
    p record;
    -- The argument: a column, or an expression with one level of parentheses.
    pattern constant text := '(public\.)?huxley_org_in_scope\(((?:[^()]|\([^()]*\))+)\)';
    replacement constant text := '((SELECT public.huxley_scope_all()) OR (\2) = ANY ((SELECT public.huxley_scope_ids())::uuid[]))';
    using_expr text;
    check_expr text;
    statement text;
BEGIN
    FOR p IN
        SELECT pol.polname, n.nspname, c.relname,
               pg_get_expr(pol.polqual, pol.polrelid) AS qual,
               pg_get_expr(pol.polwithcheck, pol.polrelid) AS with_check
          FROM pg_policy pol
          JOIN pg_class c ON c.oid = pol.polrelid
          JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE pg_get_expr(pol.polqual, pol.polrelid) LIKE '%huxley_org_in_scope(%'
            OR pg_get_expr(pol.polwithcheck, pol.polrelid) LIKE '%huxley_org_in_scope(%'
    LOOP
        using_expr := regexp_replace(p.qual, pattern, replacement, 'g');
        check_expr := regexp_replace(p.with_check, pattern, replacement, 'g');
        statement := format('ALTER POLICY %I ON %I.%I', p.polname, p.nspname, p.relname);
        IF using_expr IS NOT NULL THEN
            statement := statement || format(' USING (%s)', using_expr);
        END IF;
        IF check_expr IS NOT NULL THEN
            statement := statement || format(' WITH CHECK (%s)', check_expr);
        END IF;
        EXECUTE statement;
    END LOOP;
END
$$;

-- Dataset tables made from now on get the fast form too.
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
            'USING (%s) WITH CHECK (%s)', tbl,
            public.huxley_scope_check('_org_id'), public.huxley_scope_check('_org_id'));
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
