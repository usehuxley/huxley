#!/bin/sh
# Runs first boot and huxley-setup in a Debian container (dash, as on
# Ubuntu), with a stand-in docker that records what it's asked to do.
# Checks: secrets are made per server; setup writes .env (root-only), starts
# Huxley and creates the first user once; a second run changes the domain
# without another first user. From the repository root:
#   sh deploy/marketplace/test/setup.sh
set -eu
root=$(cd "$(dirname "$0")/../../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"
cat > "$work/bin/docker" <<'FAKE'
#!/bin/sh
echo "docker $*" >> /tmp/docker.log
FAKE
chmod +x "$work/bin/docker"
cat > "$work/check.sh" <<'CHECK'
#!/bin/sh
set -eu
fail() { echo "FAIL: $*" >&2; exit 1; }
mkdir -p /opt/huxley/deploy && cp -r /src/compose /opt/huxley/deploy/compose
export PATH=/fake:$PATH
sh /src/marketplace/files/first-boot.sh
cd /opt/huxley/deploy/compose
for f in db_password db_url kek; do [ -s "secrets/$f" ] || fail "no secrets/$f"; done
[ "$(stat -c %a .env)" = 600 ] || fail ".env isn't root-only"

HUXLEY_DOMAIN=huxley.example.com HUXLEY_SMTP_URL= HUXLEY_EMAIL_FROM="Huxley <h@example.com>" \
HUXLEY_ORG_NAME="Acme IT" HUXLEY_ADMIN_EMAIL=ada@example.com HUXLEY_ADMIN_NAME=Ada \
	sh /src/marketplace/files/huxley-setup < /dev/null > /dev/null
grep -qx "HUXLEY_DOMAIN=huxley.example.com" .env || fail "domain not set"
grep -qx "HUXLEY_PORTAL_DOMAIN=portal.example.com" .env || fail "portal domain not derived"
grep -q "compose up -d --wait huxley web portal caddy" /tmp/docker.log || fail "not started"
[ "$(grep -c 'admin bootstrap' /tmp/docker.log)" = 1 ] || fail "first user not created once"
[ -f .configured ] && [ -f .bootstrapped ] || fail "not marked set up"
[ "$(stat -c %a .env)" = 600 ] || fail ".env isn't root-only after setup"

: > /tmp/docker.log
HUXLEY_DOMAIN=desk.example.com HUXLEY_SMTP_URL= sh /src/marketplace/files/huxley-setup < /dev/null > /dev/null
grep -qx "HUXLEY_DOMAIN=desk.example.com" .env || fail "domain not changed"
[ "$(grep -c '^HUXLEY_DOMAIN=' .env)" = 1 ] || fail "domain duplicated"
grep -q "admin bootstrap" /tmp/docker.log && fail "first user created twice"
sh /src/marketplace/files/99-huxley | grep -q "docker compose" || fail "login message not updated"
echo "setup ok"
CHECK
docker run --rm --network "${DOCKER_NETWORK:-bridge}" \
	-v "$root/deploy/compose:/src/compose:ro" \
	-v "$root/deploy/marketplace:/src/marketplace:ro" \
	-v "$work/bin:/fake:ro" -v "$work/check.sh:/check.sh:ro" \
	debian:stable-slim sh /check.sh
