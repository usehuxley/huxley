#!/usr/bin/env sh
# Prepares a Huxley compose deployment: creates .env and generates secrets.
# Safe to re-run: existing files are never overwritten.
set -eu
cd "$(dirname "$0")"

if [ ! -f .env ]; then
	cp .env.example .env
	echo "Created .env. Review HUXLEY_DOMAIN and HUXLEY_PORTAL_DOMAIN before starting."
fi

# The directory is private to the host user. The files themselves must be
# world-readable because compose bind-mounts them into containers that run as
# non-root users with different uids.
mkdir -p secrets
chmod 700 secrets

if [ ! -f secrets/db_password ]; then
	# 32 random bytes, hex-encoded: URL-safe, so it can go in the connection string as-is.
	od -An -N32 -tx1 /dev/urandom | tr -d ' \n' > secrets/db_password
	echo "Generated secrets/db_password."
fi

if [ ! -f secrets/db_url ]; then
	printf 'postgres://huxley:%s@postgres:5432/huxley' "$(cat secrets/db_password)" > secrets/db_url
	echo "Generated secrets/db_url."
fi

chmod 644 secrets/db_password secrets/db_url

echo
echo "Next: docker compose up -d --build"
