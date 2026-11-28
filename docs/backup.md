# Backup and restore

A Huxley instance is four things. Back up all of them, and keep the key apart from the data it protects.

| What | Where | Without it |
|---|---|---|
| **The database** | PostgreSQL | Everything is lost. |
| **Object storage** | `storage.url`: a folder, or an S3 or Azure Blob bucket | Attachments, receipts, inbound emails and large workflow payloads are lost. Their records remain. |
| **The key-encryption key** | `crypto.kek`, or your key service (Vault, Azure Key Vault, AWS KMS) | Connections' credentials, vault entries and other encrypted fields **can't be decrypted, ever**. |
| **Configuration and secrets** | `huxley.toml`, environment, `.env`, Kubernetes Secrets | You rebuild them by hand. |

Keep the key-encryption key in a different place from the database backups, for example a password manager or your key service's own backup. Someone who steals a backup shouldn't get the key with it. Losing the key loses the encrypted data.

## Backing up

**The database:**

- **Managed PostgreSQL** (Azure Flexible Server, RDS, DigitalOcean): turn on its automated backups with point-in-time recovery. Keep them as long as your clients' contracts require.
- **Your own PostgreSQL:**
  - Run a nightly logical dump: `pg_dump -Fc -d huxley -f huxley-$(date +%F).dump`.
  - For point-in-time recovery, add WAL archiving with a tool such as pgBackRest or WAL-G.
  - Copy backups off the host.

**Object storage:** take it **after** the database, so every file the database refers to is in it. Extra files are harmless.

- **Buckets:** turn on versioning, or replicate to another region or account.
- **A folder** (Docker Compose's `huxley-objects` volume): copy it alongside the dump.

**Docker Compose**, every night:

```sh
cd deploy/compose && mkdir -p backups
docker compose exec -T postgres pg_dump -U huxley -d huxley -Fc > backups/huxley-$(date +%F).dump
docker run --rm -v huxley_huxley-objects:/objects:ro -v "$PWD/backups:/backups" alpine \
  tar czf /backups/objects-$(date +%F).tar.gz -C /objects .
```

The key is `secrets/kek`. Store it somewhere else.

## Restoring

Restore with **the same Huxley version** the backup came from, or a newer one, then migrate.

1. **Create an empty database.** Connect as a user with the same name as the original owner (often `huxley`). The dump's default privileges name that user.

2. **Create Huxley's roles.** PostgreSQL roles belong to the server, not the database, so a dump doesn't carry them. Without them, every grant is lost and Huxley can't run:

   ```sh
   HUXLEY_DATABASE__URL=postgres://huxley:…@db/huxley huxley admin prepare-restore
   ```

3. **Restore the dump.** Use `--exit-on-error`, so anything that doesn't restore stops the restore instead of being skipped:

   ```sh
   pg_restore -d huxley --no-owner --exit-on-error huxley-2026-11-27.dump
   ```

   With point-in-time recovery, restore through your provider or tool instead. Its server keeps the roles.

4. **Restore object storage** to the bucket or folder `storage.url` points at.

5. **Configure the same key-encryption key**, or point Huxley at the same key in your key service.

6. **Check it:**

   ```sh
   huxley admin verify-restore           # every file; add --sample 500 for a large instance
   ```

   It checks:
   - the migrations match the binary (or says to run `huxley migrate`);
   - Huxley runs as its application role and sees the data;
   - every data key unwraps with the configured key;
   - the audit log's hash chain is intact;
   - storage is writable;
   - every attachment, receipt, inbound email and spilled payload the database refers to is in storage.

   It exits non-zero if anything is wrong, and says what.

7. **Recreate logins that live outside the database,** such as the datasets SQL console's login (`datasets.console_url`):

   ```sql
   CREATE ROLE huxley_console LOGIN PASSWORD '…';
   GRANT huxley_datasets_reader TO huxley_console;
   ```

   Then start Huxley.

## Practise

Restore into a scratch environment every quarter, run `verify-restore`, and sign in. A backup that has never been restored isn't known to work. Huxley's CI does this on every change: it dumps the database the end-to-end tests filled, restores it into a new database, and runs `verify-restore`.
