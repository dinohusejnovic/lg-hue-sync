#!/bin/sh
# webOS Homebrew boot hook (/var/lib/webosbrew/init.d/50-lg-hue-sync).
#
# A Homebrew Channel install links this hook into the app: run the app's
# installer then, so an updated app brings its daemon and scripts into place
# before the daemon starts, even if the app has not been opened since.
HERE=$(dirname "$(readlink -f "$0")")
if [ -f "$HERE/install.sh" ]; then
    sh "$HERE/install.sh" >/dev/null 2>&1
    exit 0
fi
# An SSH install (scripts/deploy.sh) copies this hook. Hand the start to systemd
# without blocking: the unit's ExecStartPre waits for the TV to finish booting,
# then provisions Luna, before the daemon runs.
mkdir -p /run/systemd/system
cp -f /var/home/root/lg-hue-sync/lg-hue-sync.service /run/systemd/system/lg-hue-sync.service
systemctl daemon-reload
systemctl restart --no-block lg-hue-sync
