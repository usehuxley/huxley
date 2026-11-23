# ADR-025: Runners prove who they are with client certificates from Huxley or from you

- **Status:** Accepted (option chosen by the maintainer, 2026-11-23)
- **Date:** 2026-11-23

## Context

Scripts run in runners, separate processes (often separate hosts) that pull work from Huxley over gRPC with mutual TLS (design doc §3, §6.9). Runners never get database credentials, so their certificate is their whole identity. Someone has to issue those certificates. A small MSP has no PKI. A larger one may want runners to use certificates from its own CA.

## Decision

**Both (option C).**

- **Huxley as the CA (the default):**
  - The instance keeps its own runner CA: an ECDSA P-256 key, encrypted with the root organization's data key under the configured KEK, and a self-signed certificate.
  - An admin with `platform:runners:manage` creates a **single-use enrolment token** in the app. It's valid for 24 hours, and only its hash is stored.
  - The runner generates its own key, then calls `POST /api/v1/runners/enroll` over the normal HTTPS API with the token and a certificate signing request. It gets back a client certificate valid for **7 days** (CN and URI SAN name the runner), the CA certificate, and the gateway address.
  - Before expiry, it renews over the gateway with a new CSR.
  - Revoking a runner in the app takes effect at once: the gateway checks every connection against the runner registry, not just the signature.
- **Bring your own PKI (the option):** set `runners.client_ca_file` to a CA bundle, and register runners by name in the app. A certificate from that bundle is accepted when its common name is a registered, unrevoked external runner. Huxley doesn't issue or renew those certificates.
- **The gateway:** a separate gRPC listener (`runners.bind`). Its server certificate is issued by Huxley's CA, which the runner already trusts from enrolment. Every RPC needs a client certificate that passes the checks above.
- **Enrolment over HTTPS:** the runner can't trust Huxley's CA before it has it, so the first contact uses the API's usual TLS (`auth.public_url`). The token authenticates the runner.

## Consequences

- An enrolment token is a credential with the same weight as an API token, shown once.
- With a 7-day lifetime and a registry check on every connection, a leaked runner key is only useful until revocation or expiry.
- The runner CA's key sits under the existing key rotation (`rewrap-keys`). Rotating the CA itself, by issuing a new one and re-enrolling runners, is a later operation.
