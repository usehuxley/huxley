-- Payload spill (design doc §6.4): a step input or output larger than the
-- spill threshold is kept in object storage. The row's `input`/`output`
-- then holds a small marker, and these columns the object's key.
ALTER TABLE step_runs
    ADD COLUMN input_ref text,
    ADD COLUMN output_ref text;
