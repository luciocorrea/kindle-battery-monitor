"""Architecture invariant: a source copy must own every local Cargo package.

This is a distribution contract. Users receive only this source directory,
without the SDK checkout or its parent workspace metadata.
"""
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

PROJECT = Path(__file__).resolve().parents[2]


class ProjectIsolationTests(unittest.TestCase):
    def test_source_only_copy_resolves_all_local_packages_inside_itself(self):
        with tempfile.TemporaryDirectory(prefix="kindle-source-isolation-") as directory:
            source = Path(directory) / "source copy"
            shutil.copytree(PROJECT, source, ignore=shutil.ignore_patterns(
                "target", "dist", ".git", "__pycache__"))
            result = subprocess.run(
                ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
                cwd=source, capture_output=True, text=True, timeout=120)
            self.assertEqual(result.returncode, 0, result.stderr)
            metadata = json.loads(result.stdout)
            self.assertEqual(Path(metadata["workspace_root"]).resolve(), source.resolve())
            self.assertEqual({p["name"] for p in metadata["packages"]},
                             {"kindle-battery-monitor", "kindle-lipc-dbus"})
            for package in metadata["packages"]:
                self.assertTrue(Path(package["manifest_path"]).resolve().is_relative_to(source.resolve()))


if __name__ == "__main__":
    unittest.main()
