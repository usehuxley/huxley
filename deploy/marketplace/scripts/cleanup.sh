#!/bin/sh
# Leaves nothing of the build in the image: no keys, logs, history or
# cloud-init state (what the DigitalOcean Marketplace's img_check.sh and
# the other marketplaces check for).
set -eu
export DEBIAN_FRONTEND=noninteractive

apt-get -y autoremove
apt-get -y clean
rm -rf /var/lib/apt/lists/*

# Keys and history from the build. Each server gets new host keys on boot.
rm -f /root/.ssh/authorized_keys /home/*/.ssh/authorized_keys
rm -f /etc/ssh/ssh_host_*
rm -f /root/.bash_history /home/*/.bash_history

# Logs, and cloud-init's record of this instance, so it runs again.
find /var/log -type f -exec truncate -s 0 {} +
rm -rf /var/log/*.gz /var/log/*.[0-9] /var/log/journal/*
cloud-init clean --logs
rm -rf /tmp/* /var/tmp/*

# No machine identity carried over.
truncate -s 0 /etc/machine-id
rm -f /var/lib/dbus/machine-id

# Zero free space, so the image compresses (and leaves no deleted data).
dd if=/dev/zero of=/zerofile bs=4M 2>/dev/null || true
rm -f /zerofile
sync
