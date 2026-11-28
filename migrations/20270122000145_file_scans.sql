-- Malware scanning of uploads (ADR-094). With a scanner configured, each
-- new upload (record attachments, expense receipts) gets a row here, keyed
-- by where the file is stored, and the scheduler's scan loop checks it.
-- Downloads look the file up: quarantined files are never served, and
-- pending ones only when `uploads.scan.while_pending = "allow"`. Files
-- uploaded with scanning off have no row and are served as before.

CREATE TABLE file_scans (
    id               uuid PRIMARY KEY,
    storage_key      text NOT NULL UNIQUE,
    org_id           uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    -- What the file belongs to: an attachment or an expense entry's receipt.
    source           text NOT NULL CHECK (source IN ('attachment', 'receipt')),
    record_id        uuid NOT NULL,
    filename         text NOT NULL,
    size_bytes       bigint NOT NULL,
    status           text NOT NULL DEFAULT 'pending'
                     CHECK (status IN ('pending', 'clean', 'infected', 'failed')),
    -- `clamav` or `metadefender`, once scanned.
    engine           text,
    -- The threats found (or why the scanner wouldn't pass it).
    finding          text,
    attempts         integer NOT NULL DEFAULT 0,
    last_error       text,
    next_attempt_at  timestamptz NOT NULL DEFAULT now(),
    scanned_at       timestamptz,
    created_at       timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX file_scans_due ON file_scans (next_attempt_at) WHERE status = 'pending';
CREATE INDEX file_scans_status ON file_scans (status, created_at DESC) WHERE status <> 'clean';

ALTER TABLE file_scans ENABLE ROW LEVEL SECURITY;
CREATE POLICY file_scans_all ON file_scans FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
