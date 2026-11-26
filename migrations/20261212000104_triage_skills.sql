-- AI triage suggests the skills a ticket needs too (ADR-047, ADR-048), kept
-- with the suggestion; applied as the ticket's own requirements.
ALTER TABLE sd_triage ADD COLUMN skills text[] NOT NULL DEFAULT '{}';
