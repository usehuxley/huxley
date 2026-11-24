-- AI providers, bring your own key (design doc §10.1, ADR-031): one per
-- organization, used by it and every organization below that has none of
-- its own; and a log of every call, for budgets and review.

CREATE TABLE ai_providers (
    id                    uuid PRIMARY KEY,
    org_id                uuid NOT NULL UNIQUE REFERENCES organizations (id) ON DELETE CASCADE,
    kind                  text NOT NULL CHECK (kind IN ('anthropic', 'openai', 'azure_openai', 'openai_compatible')),
    model                 text NOT NULL,
    base_url              text,
    deployment            text,
    -- The API key, encrypted (purpose ai_provider_key, record id).
    key_sealed            bytea NOT NULL,
    -- Tokens (in and out) this provider may use a calendar month, across
    -- every organization using it. NULL: no limit.
    monthly_token_budget  bigint CHECK (monthly_token_budget > 0),
    created_by            uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE ai_providers ENABLE ROW LEVEL SECURITY;
CREATE POLICY ai_providers_all ON ai_providers FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));

CREATE TABLE ai_calls (
    id             uuid PRIMARY KEY,
    -- The organization the call was made for.
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    provider_id    uuid REFERENCES ai_providers (id) ON DELETE SET NULL,
    feature        text NOT NULL,
    run_id         uuid,
    model          text NOT NULL,
    input_tokens   bigint NOT NULL DEFAULT 0,
    output_tokens  bigint NOT NULL DEFAULT 0,
    -- ok, refused, failed, over_budget
    outcome        text NOT NULL,
    error          text,
    created_at     timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX ai_calls_provider_month ON ai_calls (provider_id, created_at);
CREATE INDEX ai_calls_org ON ai_calls (org_id, created_at DESC);

ALTER TABLE ai_calls ENABLE ROW LEVEL SECURITY;
CREATE POLICY ai_calls_all ON ai_calls FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
