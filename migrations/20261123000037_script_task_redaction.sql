-- What a script task's output and logs must not show: the secret values
-- and connection credentials it was given (encrypted; set when claimed).
ALTER TABLE script_tasks ADD COLUMN redact_sealed bytea;
