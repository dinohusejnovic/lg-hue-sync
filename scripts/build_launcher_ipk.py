#!/usr/bin/env python3
"""Build target/<id>_<version>_all.ipk for the launcher only, with no Ares dependency.

Adapted from magic-remote-unlock's tools/build_ipk.py. The launcher opens the
on-TV dashboard (http://127.0.0.1:8088/?tv=1) in Magic Remote mode; the daemon
itself is still installed and updated over SSH. Local installation only: per
AGENTS.md, do not publish a launcher-only IPK or Homebrew manifest.

Usage: python3 scripts/build_launcher_ipk.py   (prints the IPK path and SHA-256)
"""
import gzip
import hashlib
import io
import json
import os
import tarfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
APP_DIR = os.path.join(ROOT, "webos-app")
APP = json.load(open(os.path.join(APP_DIR, "appinfo.json")))
APP_ID, VERSION = APP["id"], APP["version"]


def info(name, size, mode=0o644, kind=tarfile.REGTYPE):
    ti = tarfile.TarInfo("./" + name)
    ti.size, ti.mode, ti.mtime, ti.type = size, mode, 0, kind
    ti.uname = ti.gname = "root"
    return ti


def add_bytes(tar, name, data, mode=0o644):
    tar.addfile(info(name, len(data), mode), io.BytesIO(data))


def tgz(build):
    raw = io.BytesIO()
    with gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode="w", format=tarfile.GNU_FORMAT) as tar:
            build(tar)
    return raw.getvalue()


def data(tar):
    app_root = "usr/palm/applications/" + APP_ID
    pkg_root = "usr/palm/packages/" + APP_ID
    for d in ["usr", "usr/palm", "usr/palm/applications", app_root, "usr/palm/packages", pkg_root]:
        tar.addfile(info(d, 0, 0o755, tarfile.DIRTYPE))
    for name in sorted(os.listdir(APP_DIR)):
        path = os.path.join(APP_DIR, name)
        if os.path.isfile(path) and not name.startswith("."):
            add_bytes(tar, f"{app_root}/{name}", open(path, "rb").read())
    package = {"id": APP_ID, "version": VERSION, "app": APP_ID}
    add_bytes(tar, f"{pkg_root}/packageinfo.json", json.dumps(package, indent=2).encode())


def control(tar):
    text = (
        f"Package: {APP_ID}\nVersion: {VERSION}\nSection: misc\nPriority: optional\n"
        "Architecture: all\nInstalled-Size: 64\nMaintainer: lg-hue-sync\n"
        "Description: On-TV control app for the lg-hue-sync daemon.\n"
        "webOS-Package-Format-Version: 2\nwebOS-Packager-Version: x.y.x\n"
    )
    add_bytes(tar, "control", text.encode())


def ar(members):
    out = b"!<arch>\n"
    for name, blob in members:
        out += ("%-16s%-12d%-6d%-6d%-8s%-10d`\n" % (name, 0, 0, 0, "100644", len(blob))).encode()
        out += blob + (b"\n" if len(blob) % 2 else b"")
    return out


def main():
    blob = ar([("debian-binary", b"2.0\n"), ("control.tar.gz", tgz(control)), ("data.tar.gz", tgz(data))])
    out_dir = os.path.join(ROOT, "target")
    os.makedirs(out_dir, exist_ok=True)
    path = os.path.join(out_dir, f"{APP_ID}_{VERSION}_all.ipk")
    open(path, "wb").write(blob)
    print(path, len(blob), "bytes sha256", hashlib.sha256(blob).hexdigest())


if __name__ == "__main__":
    main()
