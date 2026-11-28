-- Replay protection for signed webhooks (ASVS V13, docs/security.md): each
-- timestamped HMAC request's signature is kept until its timestamp leaves
-- the replay window, and a repeat is refused. Rows past expiry are swept
-- daily with retention.

CREATE TABLE webhook_deliveries (
    public_id   text NOT NULL,
    digest      bytea NOT NULL,
    expires_at  timestamptz NOT NULL,
    PRIMARY KEY (public_id, digest)
);

CREATE INDEX webhook_deliveries_expires ON webhook_deliveries (expires_at);
