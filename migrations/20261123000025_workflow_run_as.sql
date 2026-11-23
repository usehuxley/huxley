-- Workflow identity (design doc §11.1): runs started by a schedule, webhook
-- or domain event act as the workflow's run-as service account. Manual runs
-- act as whoever started them.
ALTER TABLE workflows ADD COLUMN run_as uuid REFERENCES users (id) ON DELETE SET NULL;
