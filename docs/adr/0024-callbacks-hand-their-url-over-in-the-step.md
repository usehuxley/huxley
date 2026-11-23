# ADR-024: A callback step hands its URL over itself

- **Status:** Accepted
- **Date:** 2026-11-23

## Context

`wait_for_callback` (design doc §6.2, §6.4) parks a run until an external system calls a single-use URL, for example an RMM reporting that a script finished on a device. That system has to learn the URL first, usually in the same call that starts its work. If the URL were made by the waiting step, an earlier step couldn't know it. If it were made earlier, something else would have to keep it secret and tie it to the step that waits.

## Decision

- The step hands the URL over itself, in an optional `send`: an integration action (`action`, `connection`, `with`) or an HTTP request (`request`). `callback.url` is in scope there.
- **The token:** when the walker first reaches the step, it generates a random 256-bit token. The step's input, which holds the token, is sealed like a secret, so the run view shows it redacted. The `callbacks` table stores only the token's SHA-256 hash, tied to the run, the step key and the attempt.
- **The step job:**
  - records the callback, then makes the `send`. The send is recorded, so a retried job doesn't repeat it, except after a crash before the send was recorded.
  - marks the step `waiting`, and reschedules itself for the expiry (`expires`, default `24h`)
- **Calling back:** `/callbacks/{token}` (POST, PUT or GET, outside `/api/v1`, with the webhook body limit) completes the step with `{method, query, content_type, body, received_at}` and advances the run. The URL then stops working.
- **Refusals:** unknown, used, expired and no-longer-waiting tokens all answer 404, so a caller learns nothing from the difference.
- **Expiry:** if nobody calls back in time, the step fails with `kind: timeout`, which `catch`, `retry` and `continue_on_error` handle as usual.
- **Base address:** callback URLs use `auth.public_url`. Without it, the step fails with a message saying so.

## Consequences

- No new key material: the URL's security is the random token, not a signature.
- A step can only hand its URL to one system, through `send`. A workflow that needs to pass the URL somewhere else can use the HTTP request form against its own endpoint.
- A retried attempt (say the `send` failed and `retry` allows another try) keeps the input, and so the URL and its expiry. The callback moves onto the new attempt.
