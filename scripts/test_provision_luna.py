import json
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("provision_luna.sh")
APP = "org.webosbrew.lg-hue-sync"
DAEMON = APP + ".daemon"


class ProvisionLuna(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.roots = [self.tmp / "luna", self.tmp / "luna-dev"]
        self.apps = self.tmp / "apps"
        self.scans = self.tmp / "scans"
        scanner = self.tmp / "ls-control"
        scanner.write_text(f'#!/bin/sh\necho "$@" >> "{self.scans}"\n')
        scanner.chmod(0o755)
        self.env = {"LUNA_ROOTS": " ".join(map(str, self.roots)), "APP_DIRS": str(self.apps),
                    "LS_CONTROL": str(scanner), "PATH": "/usr/bin:/bin"}

    def run_script(self):
        subprocess.run(["bash", str(SCRIPT), "local"], env=self.env, check=True, capture_output=True)

    def scan_count(self):
        return len(self.scans.read_text().splitlines()) if self.scans.exists() else 0

    def test_daemon_files_never_take_the_apps_manifest_name(self):
        dev = self.roots[1]
        (dev / "manifests.d").mkdir(parents=True)
        app_manifest = dev / "manifests.d" / f"{APP}.json"
        app_manifest.write_text('{"id":"%s","roleFiles":["%s/roles.d/%s.app.json"]}' % (APP, dev, APP))
        self.run_script()
        self.assertIn(".app.json", app_manifest.read_text(), "the app's manifest must survive")
        for root in self.roots:
            manifest = json.loads((root / "manifests.d" / f"{DAEMON}.json").read_text())
            self.assertEqual(manifest["id"], DAEMON)
            self.assertEqual(manifest["roleFiles"], [f"{root}/roles.d/{DAEMON}.json"])
            role = json.loads((root / "roles.d" / f"{DAEMON}.json").read_text())
            self.assertEqual(role["exeName"], "/var/home/root/lg-hue-sync/lg-hue-sync")
            self.assertTrue((root / "client-permissions.d" / f"{DAEMON}.json").exists())
        self.assertEqual(self.scan_count(), 1)
        # Unchanged files: no second hub rescan.
        self.run_script()
        self.assertEqual(self.scan_count(), 1)

    def test_restores_an_app_manifest_overwritten_by_an_earlier_version(self):
        dev = self.roots[1]
        for sub in ("manifests.d", "roles.d", "client-permissions.d"):
            (dev / sub).mkdir(parents=True)
        legacy = dev / "manifests.d" / f"{APP}.json"
        legacy.write_text(json.dumps({"id": APP, "roleFiles": [f"{dev}/roles.d/{APP}.json"]}))
        (dev / "roles.d" / f"{APP}.json").write_text("{}")
        (dev / "client-permissions.d" / f"{APP}.json").write_text("{}")
        (dev / "roles.d" / f"{APP}.app.json").write_text("{}")
        (dev / "client-permissions.d" / f"{APP}.app.json").write_text("{}")
        (self.apps / APP).mkdir(parents=True)
        (self.apps / APP / "appinfo.json").write_text('{\n  "id": "%s",\n  "version": "1.2.1"\n}' % APP)
        self.run_script()
        restored = json.loads(legacy.read_text())
        self.assertEqual(restored, {
            "version": "1.2.1", "id": APP,
            "roleFiles": [f"{dev}/roles.d/{APP}.app.json"],
            "clientPermissionFiles": [f"{dev}/client-permissions.d/{APP}.app.json"],
        })
        self.assertFalse((dev / "roles.d" / f"{APP}.json").exists())
        self.assertFalse((dev / "client-permissions.d" / f"{APP}.json").exists())
        self.assertTrue((dev / "roles.d" / f"{APP}.app.json").exists())


if __name__ == "__main__":
    unittest.main()
