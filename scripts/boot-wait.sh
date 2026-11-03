#!/bin/sh
# ExecStartPre for lg-hue-sync: right after a TV boot, hold the daemon until
# webOS has an app in the foreground (Home is up), then let it settle, so the
# launcher and dashboard load before sync starts its capture and network work.
# Later starts (restarts, deploys, crash recovery) do not wait.
set -u

BOOT_WINDOW_S=120
MAX_WAIT_S=90
SETTLE_S=10

uptime_s=$(cut -d. -f1 /proc/uptime)
[ "$uptime_s" -ge "$BOOT_WINDOW_S" ] && exit 0

foreground_ready() {
    # This TV's luna-send exits on stdin EOF, so hold stdin open for the reply.
    (sleep 3) | timeout 3 luna-send -n 1 -f \
        luna://com.webos.applicationManager/getForegroundAppInfo '{}' 2>/dev/null |
        grep -q '"appId": *"[^"]'
}

waited=0
while [ "$waited" -lt "$MAX_WAIT_S" ] && ! foreground_ready; do
    sleep 3
    waited=$((waited + 3))
done
echo "boot-wait: foreground after ${waited}s (uptime ${uptime_s}s); settling ${SETTLE_S}s"
sleep "$SETTLE_S"
exit 0
