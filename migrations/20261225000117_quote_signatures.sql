-- Clients answer quotes in the portal (design doc §8.10, ADR-065): the
-- quote's contact, or the company's approvers and admins, accept with a
-- typed name and a drawn signature, or decline. Each answer keeps what
-- was agreed to (a hash of the sent revision) and how (who, from where,
-- when).

CREATE TABLE sales_quote_signatures (
    id                 uuid PRIMARY KEY,
    quote_id           uuid NOT NULL REFERENCES sales_quotes (id) ON DELETE CASCADE,
    org_id             uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    revision           integer NOT NULL,
    accepted           boolean NOT NULL,
    -- The name typed: the signature in words.
    name               text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    -- The signature drawn, as a PNG (accepting only; at most 200 KB).
    image              bytea CHECK (octet_length(image) <= 204800),
    contact_id         uuid REFERENCES crm_contacts (id) ON DELETE SET NULL,
    portal_account_id  uuid,
    -- SHA-256 of the sent revision's snapshot: what was agreed to.
    document_sha256    text NOT NULL CHECK (document_sha256 ~ '^[0-9a-f]{64}$'),
    ip                 inet,
    user_agent         text CHECK (length(user_agent) <= 512),
    signed_at          timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX sales_quote_signatures_quote ON sales_quote_signatures (quote_id);

ALTER TABLE sales_quote_signatures ENABLE ROW LEVEL SECURITY;
CREATE POLICY sales_quote_signatures_all ON sales_quote_signatures FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
