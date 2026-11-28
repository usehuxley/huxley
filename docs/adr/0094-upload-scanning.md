# ADR-094: Uploads are scanned for malware by an operator-configured scanner

- **Status:** Accepted. The user chose the options: off, ClamAV or OPSWAT MetaDefender. Closes the V12 gap in the ASVS review (`docs/security.md`).
- **Date:** 2026-11-28

## Context

Huxley stores files that people upload or that arrive by email, then serves them to staff and to clients' contacts. Downloads are already inert: always `attachment`, with `nosniff` and a sandboxing CSP. But nothing checked a file for malware, so a technician could download and open an infected attachment from a client's email. ASVS V12 expects uploads to be scanned. Most MSPs already run, or can run, ClamAV, and some use OPSWAT MetaDefender, on their own servers or hosted.

## Decision

- **Three choices:** `uploads.scan.provider` is `off` (the default), `clamav` or `metadefender`, in a new `huxley-scan` crate.
  - **ClamAV:** `clamd`'s `INSTREAM`, over TCP or a Unix socket, through the `clamav-client` crate (MIT). Huxley reads clamd's reply itself. Only `stream: OK` is clean, and an error is never taken for a verdict.
  - **MetaDefender:** Core and Cloud, both through the same REST flow: `POST /file`, then poll `GET /file/{data_id}`. Only part of the result format is documented clearly enough to rely on, so only "no threat, not blocked" counts as clean. An unknown answer quarantines the file.
- **Operator configuration, not an app setting.** Scanners usually sit on the internal network. A URL an admin could set in the browser would let the app reach internal hosts, an SSRF risk, and would have to go through the egress policy. Configuration also keeps the API key out of the database (`_FILE` variables). A broken setting stops startup.
- **Asynchronous, held by default.** Uploads stay fast. Each new file (attachment or receipt) gets a `file_scans` row in its upload's transaction. The scheduler's scan loop claims due rows with `SKIP LOCKED` and a lease, so any number of scheduler processes can run it, and a scan interrupted by a crash is retried. Downloads check the row:
  - quarantined files are never served;
  - pending and failed files are served only with `while_pending = "allow"`.

  The storage handle carries both settings, so the attachment and expense crates need no configuration of their own.
- **Retries:** scanner errors are retried with backoff (30 s doubling to an hour), and a file is marked `failed` after 10 tries.
- **Findings** are audited (`file.quarantined`) and raised as a `file.quarantined` event, so workflows can notify or open tickets. That reuses what exists, instead of a new notification path.
- **Administrators** (`platform:file_scans:manage`) can see scanner status, quarantined and failed files, and scan a file again or delete it. Deleting removes the attachment, or the receipt from its entry.
- **Not covered:**
  - files uploaded while scanning was off aren't scanned retroactively;
  - raw inbound emails and spilled workflow payloads aren't scanned. They're internal and never downloaded as files. Email attachments are scanned as attachments.

## Consequences

- With scanning on, a newly uploaded file can't be downloaded for the few seconds its scan takes (or longer while the scanner is down, unless `allow`).
- Tests:
  - fake clamd and mock MetaDefender servers cover both protocols, including errors, rate limits and refused keys;
  - an optional test runs against a real clamd (`HUXLEY_TEST_CLAMAV`), and passed against ClamAV 1.4.7;
  - an API test covers the whole flow: held, clean, quarantined, audited, listed, rescanned, deleted.
- MetaDefender hasn't been checked against the real service yet. `docs/attachment-scanning.md` says so until it is.
