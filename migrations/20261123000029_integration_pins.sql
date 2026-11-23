-- Which version of each integration a run uses (keyed by integration key):
-- the one current when the run started, or when it first called it.
ALTER TABLE runs ADD COLUMN integration_pins jsonb NOT NULL DEFAULT '{}';

-- Renamed actions: old key -> current key, carried across versions so
-- workflows written against an old key keep working.
ALTER TABLE integration_versions ADD COLUMN aliases jsonb NOT NULL DEFAULT '{}';
