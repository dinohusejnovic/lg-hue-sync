#!/bin/sh
# webOS Homebrew boot hook (/var/lib/webosbrew/init.d/50-lg-hue-sync).
# Hand the start to systemd without blocking: the unit's ExecStartPre waits for
# the TV to finish booting, then provisions Luna, before the daemon runs.
mkdir -p /run/systemd/system
cp -f /var/home/root/lg-hue-sync/lg-hue-sync.service /run/systemd/system/lg-hue-sync.service
systemctl daemon-reload
systemctl restart --no-block lg-hue-sync
