#!/usr/bin/env python3
"""Build the Homebrew Channel package: the launcher app carrying the daemon.

Writes to dist/hbc/:
  org.webosbrew.lg-hue-sync_<version>_all.ipk   app + payload/ (daemon, installer, boot hook)
  manifest.json                                 Homebrew Channel package manifest
  apps.json                                     repository index for the Homebrew Channel

On the TV the app's launch page runs payload/install.sh as root through the
Homebrew Channel, which installs or upgrades the daemon (scripts/tv/hbc-install.sh).

Usage:
  python3 scripts/build_hbc_package.py --release-url URL --repo-url URL [--binary PATH]
    --release-url  where the .ipk and manifest.json are downloadable
                   (e.g. https://github.com/<owner>/lg-hue-sync/releases/download/v1.2.0)
    --repo-url     where apps.json and icon.png are served (GitHub Pages root)
"""
import argparse
import gzip
import hashlib
import io
import json
import os
import tarfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
APP_DIR = os.path.join(ROOT, "webos-app")
SCRIPTS = os.path.join(ROOT, "scripts")
DEFAULT_BINARY = os.path.join(ROOT, "target", "armv7-unknown-linux-gnueabi", "release", "lg-hue-sync")

# payload name -> (source, mode)
PAYLOAD = {
    "lg-hue-sync": (None, 0o755),  # the daemon, from --binary
    "install.sh": (os.path.join(SCRIPTS, "tv", "hbc-install.sh"), 0o755),
    "50-lg-hue-sync.sh": (os.path.join(SCRIPTS, "tv", "50-lg-hue-sync.sh"), 0o755),
    "lg-hue-sync.service": (os.path.join(SCRIPTS, "tv", "lg-hue-sync.service"), 0o644),
    "boot-wait.sh": (os.path.join(SCRIPTS, "boot-wait.sh"), 0o755),
    "provision_luna.sh": (os.path.join(SCRIPTS, "provision_luna.sh"), 0o755),
    "log-maintenance.sh": (os.path.join(SCRIPTS, "log-maintenance.sh"), 0o755),
}


def check_daemon(blob):
    """The daemon must be a 32-bit little-endian ARM ELF, as webOS runs."""
    if len(blob) < 20 or blob[:4] != b"\x7fELF":
        raise SystemExit("daemon binary is not an ELF file")
    if blob[4] != 1 or blob[5] != 1:
        raise SystemExit("daemon binary is not 32-bit little-endian")
    if int.from_bytes(blob[18:20], "little") != 40:
        raise SystemExit("daemon binary is not built for ARM")


def info(name, size, mode, kind=tarfile.REGTYPE):
    ti = tarfile.TarInfo("./" + name)
    ti.size, ti.mode, ti.mtime, ti.type = size, mode, 0, kind
    ti.uid = ti.gid = 0
    ti.uname = ti.gname = "root"
    return ti


def tgz(build):
    raw = io.BytesIO()
    with gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode="w", format=tarfile.GNU_FORMAT) as tar:
            build(tar)
    return raw.getvalue()


def ar(members):
    out = b"!<arch>\n"
    for name, blob in members:
        out += ("%-16s%-12d%-6d%-6d%-8s%-10d`\n" % (name, 0, 0, 0, "100644", len(blob))).encode()
        out += blob + (b"\n" if len(blob) % 2 else b"")
    return out


def build_ipk(app, daemon):
    app_id, version = app["id"], app["version"]
    app_root = "usr/palm/applications/" + app_id
    pkg_root = "usr/palm/packages/" + app_id
    files = {}
    for name in sorted(os.listdir(APP_DIR)):
        path = os.path.join(APP_DIR, name)
        if os.path.isfile(path) and not name.startswith("."):
            files[f"{app_root}/{name}"] = (open(path, "rb").read(), 0o644)
    for name, (source, mode) in PAYLOAD.items():
        blob = daemon if source is None else open(source, "rb").read()
        files[f"{app_root}/payload/{name}"] = (blob, mode)
    package = {"id": app_id, "version": version, "app": app_id}
    files[f"{pkg_root}/packageinfo.json"] = (json.dumps(package, indent=2).encode(), 0o644)

    def data(tar):
        dirs = ["usr", "usr/palm", "usr/palm/applications", app_root, app_root + "/payload",
                "usr/palm/packages", pkg_root]
        for d in dirs:
            tar.addfile(info(d, 0, 0o755, tarfile.DIRTYPE))
        for name, (blob, mode) in files.items():
            tar.addfile(info(name, len(blob), mode), io.BytesIO(blob))

    size_kb = sum(len(blob) for blob, _ in files.values()) // 1024 + 1

    def control(tar):
        text = (
            f"Package: {app_id}\nVersion: {version}\nSection: misc\nPriority: optional\n"
            f"Architecture: all\nInstalled-Size: {size_kb}\nMaintainer: lg-hue-sync\n"
            "Description: Philips Hue and Nanoleaf screen sync for rooted LG webOS TVs.\n"
            "webOS-Package-Format-Version: 2\nwebOS-Packager-Version: x.y.x\n"
        )
        blob = text.encode()
        tar.addfile(info("control", len(blob), 0o644), io.BytesIO(blob))

    return ar([("debian-binary", b"2.0\n"), ("control.tar.gz", tgz(control)), ("data.tar.gz", tgz(data))])


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", default=DEFAULT_BINARY)
    parser.add_argument("--release-url", required=True)
    parser.add_argument("--repo-url", required=True)
    parser.add_argument("--source-url", required=True, help="the project's GitHub page")
    parser.add_argument("--out", default=os.path.join(ROOT, "dist", "hbc"))
    args = parser.parse_args()

    app = json.load(open(os.path.join(APP_DIR, "appinfo.json")))
    daemon = open(args.binary, "rb").read()
    check_daemon(daemon)
    ipk = build_ipk(app, daemon)
    name = f"{app['id']}_{app['version']}_all.ipk"
    release_url, repo_url = args.release_url.rstrip("/"), args.repo_url.rstrip("/")
    manifest = {
        "id": app["id"],
        "version": app["version"],
        "type": app["type"],
        "title": app["title"],
        "appDescription": app["appDescription"],
        "iconUri": f"{repo_url}/icon.png",
        "sourceUrl": args.source_url,
        "rootRequired": True,
        "ipkUrl": f"{release_url}/{name}",
        "ipkHash": {"sha256": hashlib.sha256(ipk).hexdigest()},
        "ipkSize": len(ipk),
    }
    index = {
        "paging": {"page": 1, "count": 1, "maxPage": 1, "itemsTotal": 1, "prevUrl": None, "nextUrl": None},
        "packages": [{
            "id": app["id"],
            "title": app["title"],
            "iconUri": manifest["iconUri"],
            "manifestUrl": f"{release_url}/manifest.json",
            "manifest": manifest,
            "pool": "main",
            "shortDescription": app["appDescription"],
        }],
    }
    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, name), "wb") as f:
        f.write(ipk)
    for filename, doc in (("manifest.json", manifest), ("apps.json", index)):
        with open(os.path.join(args.out, filename), "w") as f:
            json.dump(doc, f, indent=2)
            f.write("\n")
    print(os.path.join(args.out, name), len(ipk), "bytes sha256", manifest["ipkHash"]["sha256"])


if __name__ == "__main__":
    main()
