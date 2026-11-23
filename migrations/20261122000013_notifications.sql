-- Notifications v0 (design doc §8.3): in-app (real time) and email channels,
-- with per-user preferences by notification kind.

CREATE TYPE notification_email_status AS ENUM ('skipped', 'pending', 'sent', 'failed');

CREATE TABLE notifications (
    id              uuid PRIMARY KEY,
    user_id         uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind            text NOT NULL CHECK (kind ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    title           text NOT NULL,
    body            text NOT NULL DEFAULT '',
    -- A path in the staff app, e.g. /profile.
    link            text CHECK (link IS NULL OR link ~ '^/'),
    -- Shown in the app (false when the user turned the in-app channel off).
    in_app          boolean NOT NULL DEFAULT true,
    read_at         timestamptz,
    email_status    notification_email_status NOT NULL DEFAULT 'skipped',
    email_attempts  integer NOT NULL DEFAULT 0,
    email_next_at   timestamptz,
    email_error     text,
    created_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX notifications_user_feed ON notifications (user_id, id DESC) WHERE in_app;
CREATE INDEX notifications_unread ON notifications (user_id) WHERE in_app AND read_at IS NULL;
CREATE INDEX notifications_email_queue ON notifications (email_next_at) WHERE email_status = 'pending';

-- Overrides of a kind's default channels. Mandatory kinds ignore them.
CREATE TABLE notification_preferences (
    user_id  uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind     text NOT NULL,
    in_app   boolean NOT NULL,
    email    boolean NOT NULL,
    PRIMARY KEY (user_id, kind)
);
