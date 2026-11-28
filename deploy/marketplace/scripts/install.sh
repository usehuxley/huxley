#!/bin/sh
# Builds the image: Docker, the Compose deployment in /opt/huxley, its
# images, a firewall, and what runs on first boot and first login. No
# secrets: those are made per server, on first boot.
set -eu
export DEBIAN_FRONTEND=noninteractive

apt-get update -q
apt-get -q -y -o Dpkg::Options::=--force-confnew upgrade
apt-get install -q -y docker.io docker-compose-v2 ufw unattended-upgrades
systemctl enable docker
systemctl start docker

# The Compose deployment, pinned to this release.
mkdir -p /opt/huxley
cp -r /tmp/huxley/deploy /opt/huxley/
cd /opt/huxley/deploy/compose
sed "s/^HUXLEY_VERSION=.*/HUXLEY_VERSION=${HUXLEY_VERSION}/" .env.example > .env.example.release
mv .env.example.release .env.example
if [ "${PREPULL}" = "true" ]; then
	# The images Huxley runs (ClamAV too: scanning is one setting away).
	HUXLEY_VERSION="${HUXLEY_VERSION}" docker compose --env-file .env.example --profile clamav pull
fi

# First boot (each server's own secrets) and first login (setup).
install -m 0755 /tmp/huxley/first-boot.sh /var/lib/cloud/scripts/per-instance/001-huxley-secrets
install -m 0755 /tmp/huxley/huxley-setup /usr/local/bin/huxley-setup
install -m 0755 /tmp/huxley/99-huxley /etc/update-motd.d/99-huxley
install -m 0644 /tmp/huxley/huxley-setup.sh /etc/profile.d/huxley-setup.sh

# Only SSH, HTTP and HTTPS (and HTTP/3) are open.
ufw default deny incoming
ufw default allow outgoing
ufw limit ssh
ufw allow http
ufw allow https
ufw --force enable

# Security updates apply themselves.
printf 'APT::Periodic::Update-Package-Lists "1";\nAPT::Periodic::Unattended-Upgrade "1";\n' \
	> /etc/apt/apt.conf.d/20auto-upgrades

rm -rf /tmp/huxley
