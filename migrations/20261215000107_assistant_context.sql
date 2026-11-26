-- The assistant, opened from a ticket or a client (design doc §8.8,
-- ADR-053): what the conversation is about, { kind, id, label }.
ALTER TABLE assistant_conversations ADD COLUMN context jsonb;
