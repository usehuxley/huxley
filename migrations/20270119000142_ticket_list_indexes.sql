-- Ticket lists read their page in order from these (docs/performance.md):
-- open tickets by priority then age, and a person's open tickets likewise.
-- Partial on tickets that aren't closed, like the lists that use them.
CREATE INDEX sd_tickets_unclosed_order ON sd_tickets (priority, created_at, id) WHERE closed_at IS NULL;
CREATE INDEX sd_tickets_assignee_order ON sd_tickets (assignee_user_id, priority, created_at, id) WHERE closed_at IS NULL;
-- Covered by the second.
DROP INDEX sd_tickets_assignee;
