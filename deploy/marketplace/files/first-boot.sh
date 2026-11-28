#!/bin/sh
# Runs once per server (cloud-init, per instance): makes its own database
# password and key-encryption key, so no two servers share them. Huxley
# starts after setup (huxley-setup), not before.
set -eu
cd /opt/huxley/deploy/compose
[ -f .env ] || cp .env.example .env
sh install.sh > /var/log/huxley-first-boot.log 2>&1
chmod 600 .env
