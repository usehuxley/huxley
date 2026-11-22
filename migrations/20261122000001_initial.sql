-- Initial migration: extensions and shared helpers.
-- Module tables arrive with their phases (organizations, users, and RBAC in Phase 1).

CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE EXTENSION IF NOT EXISTS ltree;
CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- Schema for generated, typed dataset tables (design doc §9).
CREATE SCHEMA IF NOT EXISTS datasets;

-- RLS helper. huxley-db sets `huxley.org_scope` per transaction to either '*'
-- (system work) or a comma-separated list of permitted organization ids.
-- An unset or empty scope sees nothing: deny by default.
CREATE OR REPLACE FUNCTION huxley_org_in_scope(org uuid) RETURNS boolean
LANGUAGE sql STABLE AS $$
    SELECT CASE
        WHEN coalesce(current_setting('huxley.org_scope', true), '') = '' THEN false
        WHEN current_setting('huxley.org_scope', true) = '*' THEN true
        ELSE org = ANY (string_to_array(current_setting('huxley.org_scope', true), ',')::uuid[])
    END
$$;

-- Keeps updated_at current on tables that opt in via a trigger.
CREATE OR REPLACE FUNCTION huxley_touch_updated_at() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END
$$;
