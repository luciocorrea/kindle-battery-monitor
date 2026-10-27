"""Release artifact contracts through the public packaging script."""
import hashlib
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import tomllib
import unittest
import zipfile

PROJECT = Path(__file__).resolve().parents[2]


def arm_elf(segment_type=1, machine=40):
    data = bytearray(84)
    data[:7] = b"\x7fELF\x01\x01\x01"
    struct.pack_into("<HHI", data, 16, 2, machine, 1)
    struct.pack_into("<I", data, 28, 52)
    struct.pack_into("<HH", data, 42, 32, 1)
    struct.pack_into("<I", data, 52, segment_type)
    return data


class PackageCommandTests(unittest.TestCase):
    def test_non_arm_and_dynamic_binaries_emit_no_release_artifacts(self):
        for label, data in [("non-arm", arm_elf(machine=62)),
                            ("dynamic", arm_elf(segment_type=2)),
                            ("interpreter", arm_elf(segment_type=3))]:
            with self.subTest(label=label), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                binary = root / label
                binary.write_bytes(data)
                output = root / "output"
                result = subprocess.run(["sh", str(PROJECT / "scripts/package.sh"),
                                         "--binary", str(binary), "--output", str(output)],
                                        cwd=root, env=dict(os.environ, PYTHON=sys.executable),
                                        capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Packaging failed:", result.stderr)
                self.assertEqual(list(output.glob("*.zip*")), [])

    def test_versioned_zip_and_checksum_preserve_an_older_release(self):
        with tempfile.TemporaryDirectory(prefix="kindle-package-command-") as directory:
            root = Path(directory)
            binary = root / "monitor binary"
            binary.write_bytes(arm_elf())
            output = root / "release output"
            output.mkdir()
            previous = output / "kindle-battery-monitor-0.0.1.zip"
            previous.write_bytes(b"previous release")
            result = subprocess.run(["sh", str(PROJECT / "scripts/package.sh"),
                                     "--binary", str(binary), "--output", str(output)],
                                    cwd=root, env=dict(os.environ, PYTHON=sys.executable),
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            version = tomllib.loads((PROJECT / "Cargo.toml").read_text())["package"]["version"]
            archive = output / f"kindle-battery-monitor-{version}.zip"
            checksum = output / (archive.name + ".sha256")
            self.assertEqual(checksum.read_text(),
                             f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n")
            self.assertEqual(previous.read_bytes(), b"previous release")
            with zipfile.ZipFile(archive) as release:
                prefix = "kindle-battery-monitor/"
                manifest = dict(line.split("=", 1) for line in
                                release.read(prefix + "payload/manifest").decode().splitlines())
                self.assertEqual(manifest["version"], version)
                self.assertEqual(release.read(prefix + "LICENSE"), (PROJECT / "LICENSE").read_bytes())
                for field, name in [("binary_sha256", "payload/monitor"),
                                    ("automatic_sha256", "payload/automatic.conf"),
                                    ("on_demand_sha256", "payload/on-demand.conf"),
                                    ("controller_sha256", "bin/control.sh"),
                                    ("menu_sha256", "menu.json"),
                                    ("config_sha256", "config.xml")]:
                    self.assertEqual(hashlib.sha256(release.read(prefix + name)).hexdigest(),
                                     manifest[field])
                self.assertFalse(any(name.startswith(prefix + "state/") for name in release.namelist()))
                self.assertNotIn(prefix + "bin/kindle-battery-monitor", release.namelist())
                self.assertEqual(release.getinfo(prefix + "bin/control.sh").external_attr >> 16 & 0o111,
                                 0o111)


if __name__ == "__main__":
    unittest.main()
