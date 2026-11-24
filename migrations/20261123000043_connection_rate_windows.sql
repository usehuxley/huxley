-- Rate limits from integration overlays: calls per connection per window,
-- counted here so every worker shares the limit.
CREATE TABLE connection_rate_windows (
    connection_id  uuid NOT NULL REFERENCES connections (id) ON DELETE CASCADE,
    window_start   timestamptz NOT NULL,
    used           integer NOT NULL,
    PRIMARY KEY (connection_id, window_start)
);

ALTER TABLE connection_rate_windows ENABLE ROW LEVEL SECURITY;
CREATE POLICY connection_rate_windows_system ON connection_rate_windows FOR ALL TO huxley_app
    USING (current_setting('huxley.org_scope', true) = '*')
    WITH CHECK (current_setting('huxley.org_scope', true) = '*');
