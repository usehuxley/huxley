-- Secret (writeOnly) run inputs: kept encrypted here, with only a
-- redaction marker in runs.inputs.
ALTER TABLE runs ADD COLUMN inputs_sealed bytea;
