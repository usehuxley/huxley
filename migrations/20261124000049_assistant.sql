-- The built-in assistant (design doc §10.3): each user's conversations,
-- their messages, and tool calls waiting for the user's approval.

CREATE TABLE assistant_conversations (
    id          uuid PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- The organization it works in (whose AI provider it uses).
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    title       text NOT NULL,
    -- Tool calls of the last turn that need the user's say, with any
    -- results already in: { "calls": [...], "results": [...] }.
    pending     jsonb,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX assistant_conversations_user ON assistant_conversations (user_id, updated_at DESC);

CREATE TABLE assistant_messages (
    conversation_id  uuid NOT NULL REFERENCES assistant_conversations (id) ON DELETE CASCADE,
    seq              integer NOT NULL,
    org_id           uuid NOT NULL,
    -- A huxley_ai::chat::Message.
    message          jsonb NOT NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (conversation_id, seq)
);

ALTER TABLE assistant_conversations ENABLE ROW LEVEL SECURITY;
CREATE POLICY assistant_conversations_all ON assistant_conversations FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
ALTER TABLE assistant_messages ENABLE ROW LEVEL SECURITY;
CREATE POLICY assistant_messages_all ON assistant_messages FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));

UPDATE roles SET permissions = permissions || '{ai:use}' WHERE key = 'technician' AND is_system;
