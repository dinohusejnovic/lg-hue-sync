import gzip
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("build_hbc_package.py")
ROOT = SCRIPT.parent.parent
APP = json.loads((ROOT / "webos-app" / "appinfo.json").read_text())


def ar_entries(data):
    assert data.startswith(b"!<arch>\n"), "not an ar archive"
    offset, entries = 8, {}
    while offset < len(data):
        header = data[offset:offset + 60]
        size = int(header[48:58])
        entries[header[:16].decode().strip()] = data[offset + 60:offset + 60 + size]
        offset += 60 + size + size % 2
    return entries


def elf(machine, bits=1):
    header = bytearray(64)
    header[:4] = b"\x7fELF"
    header[4], header[5] = bits, 1
    header[18:20] = machine.to_bytes(2, "little")
    return bytes(header)


class BuildHbcPackage(unittest.TestCase):
    def build(self, binary):
        temp = Path(tempfile.mkdtemp())
        (temp / "daemon").write_bytes(binary)
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--binary", str(temp / "daemon"), "--out", str(temp / "out"),
             "--release-url", "https://example.invalid/releases/download/v1/",
             "--repo-url", "https://example.invalid/repo",
             "--source-url", "https://example.invalid/src"],
            capture_output=True, text=True)
        return temp / "out", result

    def test_package_carries_the_daemon_and_installer_with_matching_manifests(self):
        out, result = self.build(elf(40) + b"daemon")
        self.assertEqual(result.returncode, 0, result.stderr)
        name = f"{APP['id']}_{APP['version']}_all.ipk"
        ipk = (out / name).read_bytes()
        manifest = json.loads((out / "manifest.json").read_text())
        index = json.loads((out / "apps.json").read_text())
        self.assertEqual(manifest["ipkHash"]["sha256"], hashlib.sha256(ipk).hexdigest())
        self.assertEqual(manifest["ipkSize"], len(ipk))
        self.assertEqual(manifest["ipkUrl"], f"https://example.invalid/releases/download/v1/{name}")
        self.assertTrue(manifest["rootRequired"])
        self.assertEqual(index["packages"][0]["manifest"], manifest)
        self.assertEqual(index["packages"][0]["manifestUrl"],
                         "https://example.invalid/releases/download/v1/manifest.json")

        entries = ar_entries(ipk)
        self.assertEqual(list(entries), ["debian-binary", "control.tar.gz", "data.tar.gz"])
        with tarfile.open(fileobj=io.BytesIO(gzip.decompress(entries["data.tar.gz"]))) as tar:
            members = {m.name: m for m in tar.getmembers()}
            payload = f"./usr/palm/applications/{APP['id']}/payload/"
            for name, mode in [("lg-hue-sync", 0o755), ("install.sh", 0o755), ("50-lg-hue-sync.sh", 0o755),
                               ("provision_luna.sh", 0o755), ("boot-wait.sh", 0o755),
                               ("log-maintenance.sh", 0o755), ("lg-hue-sync.service", 0o644)]:
                self.assertEqual(members[payload + name].mode, mode, name)
                self.assertEqual(members[payload + name].uid, 0, name)
            daemon = tar.extractfile(members[payload + "lg-hue-sync"]).read()
            self.assertEqual(daemon, elf(40) + b"daemon")
            self.assertIn(f"./usr/palm/applications/{APP['id']}/index.html", members)
            self.assertIn(f"./usr/palm/packages/{APP['id']}/packageinfo.json", members)

    def test_rejects_a_daemon_not_built_for_32_bit_arm(self):
        for binary in (b"not an elf", elf(62), elf(40, bits=2)):
            _, result = self.build(binary)
            self.assertNotEqual(result.returncode, 0)

    def test_launch_page_runs_the_packaged_installer(self):
        page = (ROOT / "webos-app" / "index.html").read_text()
        self.assertIn("org.webosbrew.hbchannel.service/exec", page)
        self.assertIn("/payload/install.sh", page)


if __name__ == "__main__":
    unittest.main()
