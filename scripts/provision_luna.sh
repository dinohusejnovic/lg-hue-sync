#!/usr/bin/env bash
set -euo pipefail

# provision_luna.sh
# Provisions Luna Service 2 security manifests, roles, and client permissions
# for the lg-hue-sync daemon (/var/home/root/lg-hue-sync/lg-hue-sync).
#
# The daemon's files are named org.webosbrew.lg-hue-sync.daemon.*: the TV app
# shares the id org.webosbrew.lg-hue-sync, and appInstallService writes the
# app's own manifest as manifests.d/org.webosbrew.lg-hue-sync.json. Writing
# the daemon's manifest under that name replaced the app's, which left the app
# unable to call any Luna service (the Homebrew Channel included). Earlier
# versions did exactly that; their files are removed and the app's manifest is
# restored below.

TV_IP="${1:?Usage: ./scripts/provision_luna.sh <tv-ip|local> [ssh-port]}"
SSH_PORT="${2:-22}"

SSH_OPTS="-p $SSH_PORT -o StrictHostKeyChecking=accept-new -o ConnectTimeout=5"

APP_ID=org.webosbrew.lg-hue-sync
NAME=$APP_ID.daemon
# Overridable for scripts/test_provision_luna.py.
LUNA_ROOTS="${LUNA_ROOTS:-/var/luna-service2 /var/luna-service2-dev}"
APP_DIRS="${APP_DIRS:-/media/developer/apps/usr/palm/applications /media/cryptofs/apps/usr/palm/applications}"
LS_CONTROL="${LS_CONTROL:-ls-control}"

provision_files() {
    local root stage changed=0
    stage=$(mktemp -d)

    # Install only changed files: an unconditional hub rescan on every boot
    # competes with the TV's own startup.
    put() {
        if ! cmp -s "$1" "$2"; then
            cp -f "$1" "$2"
            changed=1
        fi
    }
    drop() {
        if [ -e "$1" ]; then
            rm -f "$1"
            changed=1
        fi
    }

    cat << 'JSON' > "$stage/role.json"
{
    "exeName": "/var/home/root/lg-hue-sync/lg-hue-sync",
    "type": "privileged",
    "allowedNames": [
        "com.webos.service.capture",
        "com.webos.service.capture.client*",
        "com.webos.rm.client.*",
        "com.webos.service.capturepermission",
        "org.webosbrew.lg-hue-sync*"
    ],
    "permissions": [
        {
            "service": "com.webos.service.capture",
            "inbound": ["*"],
            "outbound": ["*"]
        },
        {
            "service": "com.webos.service.capture.client*",
            "inbound": ["*"],
            "outbound": ["*"]
        },
        {
            "service": "com.webos.rm.client.*",
            "inbound": ["*"],
            "outbound": ["*"]
        },
        {
            "service": "com.webos.service.capturepermission",
            "inbound": ["*"],
            "outbound": ["*"]
        },
        {
            "service": "org.webosbrew.lg-hue-sync*",
            "inbound": ["*"],
            "outbound": ["*"]
        }
    ]
}
JSON

    cat << 'JSON' > "$stage/perm.json"
{
    "org.webosbrew.lg-hue-sync*": [
        "all"
    ],
    "com.webos.service.capture.client*": [
        "all"
    ],
    "com.webos.service.capture": [
        "all"
    ],
    "com.webos.service.capturepermission": [
        "all"
    ],
    "com.webos.rm.client.*": [
        "all"
    ]
}
JSON

    for root in $LUNA_ROOTS; do
        mkdir -p "$root/manifests.d" "$root/roles.d" "$root/client-permissions.d"
        cat << JSON > "$stage/manifest.json"
{
    "id": "$NAME",
    "version": "1.0.0",
    "roleFiles": [
        "$root/roles.d/$NAME.json"
    ],
    "clientPermissionFiles": [
        "$root/client-permissions.d/$NAME.json"
    ],
    "serviceFiles": [],
    "apiPermissionFiles": []
}
JSON
        put "$stage/role.json" "$root/roles.d/$NAME.json"
        put "$stage/perm.json" "$root/client-permissions.d/$NAME.json"
        put "$stage/manifest.json" "$root/manifests.d/$NAME.json"

        # Files from earlier versions, which used the app's id as their name.
        drop "$root/roles.d/$APP_ID.json"
        drop "$root/client-permissions.d/$APP_ID.json"
        local legacy="$root/manifests.d/$APP_ID.json"
        if [ -f "$legacy" ] && grep -q "roles.d/$APP_ID.json\"" "$legacy"; then
            drop "$legacy"
            # Put back the installed app's manifest, as appInstallService
            # writes it, if the app's role files are still there.
            if [ -f "$root/roles.d/$APP_ID.app.json" ] \
                && [ -f "$root/client-permissions.d/$APP_ID.app.json" ]; then
                local version=1.0.0 dir info
                for dir in $APP_DIRS; do
                    info="$dir/$APP_ID/appinfo.json"
                    if [ -f "$info" ]; then
                        version=$(sed -n 's/.*"version"[^"]*"\([^"]*\)".*/\1/p' "$info" | head -n 1)
                        break
                    fi
                done
                printf '{"version":"%s","id":"%s","roleFiles":["%s"],"clientPermissionFiles":["%s"]}' \
                    "${version:-1.0.0}" "$APP_ID" "$root/roles.d/$APP_ID.app.json" \
                    "$root/client-permissions.d/$APP_ID.app.json" > "$legacy"
            fi
        fi
    done

    rm -rf "$stage"
    if [ "$changed" = 1 ]; then
        $LS_CONTROL scan-services
        echo "[+] Luna permissions provisioned and scanned successfully."
    else
        echo "[+] Luna permissions already current; no rescan needed."
    fi
}

if [ "$TV_IP" = "local" ] || [ "$TV_IP" = "localhost" ] || [ "$TV_IP" = "127.0.0.1" ]; then
    echo "[*] Provisioning Luna permissions locally..."
    provision_files
else
    echo "[*] Provisioning Luna permissions for $NAME on $TV_IP:$SSH_PORT..."
    # The same script, run on the TV.
    ssh $SSH_OPTS root@"$TV_IP" "bash -s local" < "$0"
fi
