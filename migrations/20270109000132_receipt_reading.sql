-- Receipt reading (design doc §8.16, ADR-080): when a receipt is kept, the
-- built-in Receipt reading pack has AI read its amount, tax, date and
-- vendor; what it read is kept beside the entry for the person to confirm
-- (it never changes the entry by itself).

ALTER TABLE exp_entries
    -- {amount, tax, currency, spentOn, vendor, description}, as read.
    ADD COLUMN reading        jsonb,
    ADD COLUMN reading_error  text CHECK (length(reading_error) <= 1000),
    ADD COLUMN read_at        timestamptz;
