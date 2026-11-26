-- What one ticket needs (design doc §8.8, ADR-048): requirements can be set
-- on a ticket as well as on its queue and type (by AI triage or a
-- technician), and dispatch recommendations weigh them.

ALTER TABLE wf_requirements DROP CONSTRAINT wf_requirements_target_type_check;
ALTER TABLE wf_requirements ADD CONSTRAINT wf_requirements_target_type_check
    CHECK (target_type IN ('queue', 'ticket_type', 'ticket'));
