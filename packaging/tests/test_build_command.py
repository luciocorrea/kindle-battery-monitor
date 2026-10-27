"""Build command contract at the external compiler boundary."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

PROJECT = Path(__file__).resolve().parents[2]


class FakeCross:
    def __init__(self, directory):
        self.log = directory / "cross-call.json"
        executable = directory / "cross"
        executable.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, pathlib, subprocess, sys\n"
            "pathlib.Path(os.environ['FAKE_CROSS_LOG']).write_text(json.dumps({"
            "'args':sys.argv[1:], 'cwd':os.getcwd(),"
            "'toolchain':os.environ.get('RUSTUP_TOOLCHAIN'),"
            "'rustc':subprocess.check_output(['rustc','--version'],text=True).strip()}))\n"
            "sys.exit(int(os.environ.get('FAKE_CROSS_STATUS','0')))\n")
        executable.chmod(0o755)
        self.env = dict(os.environ, PATH=str(directory) + os.pathsep + os.environ["PATH"],
                        FAKE_CROSS_LOG=str(self.log))


class BuildCommandTests(unittest.TestCase):
    def test_build_selects_the_locked_arm_release_and_propagates_failure(self):
        with tempfile.TemporaryDirectory(prefix="kindle-build-command-") as directory:
            root = Path(directory)
            compiler = FakeCross(root)
            result = subprocess.run(["sh", str(PROJECT / "scripts/build.sh")],
                                    cwd=root, env=compiler.env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            call = json.loads(compiler.log.read_text())
            self.assertEqual(call["args"], ["build", "--locked", "--release", "--target",
                                           "arm-unknown-linux-musleabihf", "--config",
                                           'build.rustc-wrapper=""'])
            self.assertEqual(Path(call["cwd"]).resolve(), PROJECT.resolve())
            self.assertRegex(call["toolchain"], r"^1\.95\.0-[^-]+-[^-]+-.+$")
            self.assertRegex(call["rustc"], r"^rustc 1\.95\.0 ")
            compiler.env["FAKE_CROSS_STATUS"] = "23"
            failed = subprocess.run(["sh", str(PROJECT / "scripts/build.sh")],
                                    cwd=root, env=compiler.env, capture_output=True, text=True)
            self.assertEqual(failed.returncode, 23, failed.stderr)


if __name__ == "__main__":
    unittest.main()
