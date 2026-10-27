#!/usr/bin/env python3
"""Build a versioned KUAL ZIP and its SHA-256 sidecar."""
import argparse
import hashlib
import os
from pathlib import Path
import re
import subprocess
import tempfile
import zipfile

from package import stage, verify_binary

PROJECT = Path(__file__).resolve().parent.parent
TARGET = "arm-unknown-linux-musleabihf"
NAME = "kindle-battery-monitor"


def release(binary, output):
    verify_binary(binary)
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".kindle-release-", dir=output) as directory:
        temporary = Path(directory)
        staged = temporary / NAME
        stage(binary, staged)
        (staged / "LICENSE").write_bytes((PROJECT / "LICENSE").read_bytes())
        manifest = dict(line.split("=", 1) for line in
                        (staged / "payload/manifest").read_text().splitlines())
        version = manifest["version"]
        if not re.fullmatch(r"[0-9A-Za-z._-]+", version):
            raise ValueError("invalid release version")
        archive = temporary / f"{NAME}-{version}.zip"
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            for path in sorted(staged.rglob("*")):
                if path.is_file():
                    info = zipfile.ZipInfo(path.relative_to(temporary).as_posix())
                    info.compress_type = zipfile.ZIP_DEFLATED
                    info.create_system = 3
                    info.external_attr = (0o100000 | path.stat().st_mode & 0o777) << 16
                    bundle.writestr(info, path.read_bytes())
        checksum = temporary / (archive.name + ".sha256")
        checksum.write_text(f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n")
        os.replace(archive, output / archive.name)
        os.replace(checksum, output / checksum.name)
        print(output / archive.name)
        print(output / checksum.name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, help="package an existing static ARM binary")
    parser.add_argument("--output", type=Path, default=PROJECT / "dist")
    args = parser.parse_args()
    binary = args.binary.resolve() if args.binary else None
    output = args.output.resolve()
    try:
        if binary is None:
            subprocess.run(["sh", str(PROJECT / "scripts/build.sh")], check=True)
            target = Path(os.environ.get("CARGO_TARGET_DIR", "target"))
            if not target.is_absolute():
                target = PROJECT / target
            binary = target / TARGET / "release" / NAME
        release(binary, output)
    except subprocess.CalledProcessError as error:
        parser.exit(error.returncode, "Cross build failed\n")
    except (OSError, ValueError) as error:
        parser.exit(1, f"Packaging failed: {error}\n")


if __name__ == "__main__":
    main()
