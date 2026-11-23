-- Resuming a failed run (design doc §6.4): failed steps are kept for history
-- but superseded, so the journal walk schedules them again as new attempts.
ALTER TABLE step_runs ADD COLUMN superseded boolean NOT NULL DEFAULT false;
