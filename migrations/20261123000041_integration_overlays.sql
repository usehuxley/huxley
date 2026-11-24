-- Integration overlays (design doc §7.1): edits to how an integration's
-- actions are shown, kept apart from the vendor's spec so every imported
-- version gets them.
ALTER TABLE integrations ADD COLUMN overlay jsonb NOT NULL DEFAULT '{}';
