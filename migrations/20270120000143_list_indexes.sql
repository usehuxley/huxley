-- A record's audit history filters on target_id alone, which no index led
-- with (docs/performance.md: 276 ms -> 0.1 ms at 3 million records).
CREATE INDEX audit_events_target_seq ON audit_events (target_id, seq DESC) WHERE seq IS NOT NULL;

-- Asset lists page by name (then id), picking the page before their joins.
CREATE INDEX assets_name_order ON assets (lower(name), id);
