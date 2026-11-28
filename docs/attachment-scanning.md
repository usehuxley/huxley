# Malware scanning

Huxley can check every uploaded file for malware before anyone downloads it. That covers files attached to records, attachments taken from inbound email, and expense receipts. Scanning is off by default. To turn it on, run a scanner and point Huxley at it:

- **ClamAV:** free and open source, runs next to Huxley as a container.
- **OPSWAT MetaDefender:**
  - **Core:** on your own servers, with many antivirus engines.
  - **Cloud:** hosted by OPSWAT. It needs nothing to run, but every uploaded file is sent to OPSWAT. Make sure that's acceptable for your clients' data before you turn it on.

## How it works

1. A new upload is stored and marked **pending**.
2. The scheduler sends it to the scanner, usually within seconds.
3. The scanner calls it **clean**, or it's **quarantined**: it can never be downloaded, and the finding is recorded.

While a file is pending, it can't be downloaded. People see "Scanning…" until it's done. Set `while_pending = "allow"` to let pending files be downloaded until a scan finds something, for when a slow or unavailable scanner mustn't stop work.

If the scanner can't be reached, or returns an error, Huxley tries again: after 30 seconds, then doubling up to an hour. After 10 failed tries the file is marked **failed**. It's treated like a pending one, so it isn't downloadable unless `while_pending = "allow"`.

When a file is quarantined, Huxley:

- writes a `file.quarantined` audit record, with the file, what it belongs to, and the finding;
- raises a `file.quarantined` event, which a workflow can trigger on, for example to notify your security team or open a ticket.

Under **Settings → Malware scanning**, administrators with `platform:file_scans:manage` see:

- whether the scanner is answering;
- files that were quarantined, or couldn't be scanned.

From there they can scan a file again or delete it. Deleting removes the attachment, or the receipt from its expense entry (the entry stays). The API is `/api/v1/platform/file-scans`.

Files uploaded while scanning was off aren't scanned later, and stay downloadable. Turning scanning off again makes pending files downloadable, since nothing would scan them. Quarantined files stay blocked either way.

## Setting it up

Scanning is server configuration, set by whoever runs Huxley, not a setting in the app. Scanners usually sit on your internal network, which the app shouldn't be able to point anywhere. A broken setting stops Huxley from starting, rather than holding uploads with nothing to scan them.

### ClamAV

```toml
[uploads.scan]
provider = "clamav"

[uploads.scan.clamav]
address = "tcp://clamav:3310"   # or "unix:/run/clamav/clamd.sock"
timeout_seconds = 120
```

Run `clamav/clamav` alongside Huxley. It downloads signature updates itself. ClamAV refuses streams over its `StreamMaxLength` (100 MB by default in recent versions). Keep it above `storage.max_upload_mb`, or larger files will fail to scan.

### MetaDefender

```toml
[uploads.scan]
provider = "metadefender"

[uploads.scan.metadefender]
url = "https://api.metadefender.com/v4"   # Cloud (the default); Core is e.g. "http://metadefender:8008"
# api_key: set HUXLEY_UPLOADS__SCAN__METADEFENDER__API_KEY_FILE to a file holding it
rule = "multiscan"                         # Core only, optional: the workflow rule to scan with
timeout_seconds = 300
```

Huxley sends the file to `POST /file` and asks `GET /file/{data_id}` for the result until it's finished. A file counts as clean only when MetaDefender reports no threat (`scan_all_result_i` 0) and doesn't block it. Any other result quarantines it, including one Huxley doesn't recognize. A rate-limited scan (253) is tried again later.

> MetaDefender support has been tested against a stand-in built from OPSWAT's documented API, not yet against the real service.

### All settings

| Setting | Default | |
|---|---|---|
| `uploads.scan.provider` | `off` | `off`, `clamav` or `metadefender` |
| `uploads.scan.while_pending` | `hold` | `hold`: not downloadable until clean. `allow`: downloadable until a scan finds something. |
| `uploads.scan.clamav.address` | `tcp://localhost:3310` | |
| `uploads.scan.clamav.timeout_seconds` | `120` | |
| `uploads.scan.metadefender.url` | `https://api.metadefender.com/v4` | |
| `uploads.scan.metadefender.api_key` | none | Use the `_FILE` variable. |
| `uploads.scan.metadefender.rule` | none | |
| `uploads.scan.metadefender.timeout_seconds` | `300` | |
| `uploads.scan.metadefender.poll_milliseconds` | `1000` | |

As with all settings, environment variables work too: for example `HUXLEY_UPLOADS__SCAN__PROVIDER=clamav`.
