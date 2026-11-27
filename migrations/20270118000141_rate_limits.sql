-- Shared rate limits (ADR-090): fixed-window counters per scope and client
-- address, so every API replica enforces the same limit (they were per
-- process). Unlogged: counters lost in a crash just start again. Rows past
-- their window are swept daily with retention.

CREATE UNLOGGED TABLE rate_limit_hits (
    key           text PRIMARY KEY,
    window_start  timestamptz NOT NULL,
    hits          integer NOT NULL
);

CREATE INDEX rate_limit_hits_window ON rate_limit_hits (window_start);
