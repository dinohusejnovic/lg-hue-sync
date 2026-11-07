#!/bin/sh
# Install or upgrade the lg-hue-sync service from the files inside this app,
# and make sure it is running. The app's launch page runs this as root through
# the Homebrew Channel's exec service each time it opens. Safe to repeat.
#
# Installs where scripts/deploy.sh does (/var/home/root/lg-hue-sync), so an
# SSH install is upgraded in place, with its config.json, and a newer binary
# installed over SSH is never replaced by an older packaged one.
#
# The boot hook is a link into this app: when the app is uninstalled the link
# leads nowhere and the service no longer starts at boot.
#
# Prints one JSON line last, which the launch page reads.

HERE=$(cd "$(dirname "$0")" && pwd)
D=/var/home/root/lg-hue-sync
HOOKS=/var/lib/webosbrew/init.d
RUN_UNIT=/run/systemd/system/lg-hue-sync.service

fail() { printf '{"ok":false,"error":"%s"}\n' "$1"; exit 0; }
version_of() { "$1" --version 2>/dev/null | awk 'NR == 1 { print $NF }'; }
sha_of() { sha256sum "$1" 2>/dev/null | cut -d' ' -f1; }
# Whether dotted version $1 is newer than $2. BusyBox sort has no -V.
newer() {
    awk -v a="$1" -v b="$2" 'BEGIN {
        n = split(a, x, "."); m = split(b, y, "."); if (m > n) n = m
        for (i = 1; i <= n; i++) { if (x[i] + 0 > y[i] + 0) exit 0; if (x[i] + 0 < y[i] + 0) exit 1 }
        exit 1 }'
}

[ "$(id -u)" = 0 ] || fail "not running as root"
[ -f "$HERE/lg-hue-sync" ] || fail "the app has no service in it"
chmod 755 "$HERE/lg-hue-sync" "$HERE"/*.sh 2>/dev/null
want=$(version_of "$HERE/lg-hue-sync")
[ -n "$want" ] || fail "the packaged service does not run on this TV"
have=$(version_of "$D/lg-hue-sync")
mkdir -p "$D" || fail "could not create $D"

action=started
if [ "$(sha_of "$HERE/lg-hue-sync")" != "$(sha_of "$D/lg-hue-sync")" ]; then
    if [ -n "$have" ] && newer "$have" "$want"; then
        action=kept-newer
    else
        # Copied beside the running binary and renamed over it: writing into a
        # running executable fails with "Text file busy".
        cp "$HERE/lg-hue-sync" "$D/lg-hue-sync.new" || fail "could not copy the service"
        chmod 755 "$D/lg-hue-sync.new"
        [ -f "$D/lg-hue-sync" ] && cp -p "$D/lg-hue-sync" "$D/lg-hue-sync.previous"
        mv -f "$D/lg-hue-sync.new" "$D/lg-hue-sync" || fail "could not replace the service"
        if [ -n "$have" ]; then action=upgraded; else action=installed; fi
    fi
fi

for f in boot-wait.sh provision_luna.sh log-maintenance.sh lg-hue-sync.service; do
    cmp -s "$HERE/$f" "$D/$f" || cp -f "$HERE/$f" "$D/$f" || fail "could not copy $f"
done
chmod 755 "$D/boot-wait.sh" "$D/provision_luna.sh" "$D/log-maintenance.sh"
chmod 644 "$D/lg-hue-sync.service"
[ -f "$D/config.json" ] && chmod 600 "$D/config.json"

# Luna permissions for the capture and TV services; rescans only on change.
"$D/provision_luna.sh" local >/dev/null 2>&1 || fail "could not set up the service permissions"

# The service runs as root, so nothing else on the TV may change the files
# its boot hook runs from.
chmod -R go-w "$(dirname "$HERE")" 2>/dev/null
mkdir -p "$HOOKS" || fail "could not create $HOOKS"
rm -f "$HOOKS/50-lg-hue-sync"
ln -s "$HERE/50-lg-hue-sync.sh" "$HOOKS/50-lg-hue-sync" || fail "could not add the boot hook"
"$D/log-maintenance.sh" --install >/dev/null 2>&1
dirname "$HERE" > "$D/.installed-from-app"

mkdir -p /run/systemd/system
if ! cmp -s "$D/lg-hue-sync.service" "$RUN_UNIT"; then
    cp -f "$D/lg-hue-sync.service" "$RUN_UNIT"
    systemctl daemon-reload
fi
case "$action" in
    installed | upgraded) systemctl restart --no-block lg-hue-sync ;;
    *) systemctl is-active -q lg-hue-sync || systemctl start --no-block lg-hue-sync ;;
esac

printf '{"ok":true,"action":"%s","version":"%s"}\n' "$action" "$(version_of "$D/lg-hue-sync")"
