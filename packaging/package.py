#!/usr/bin/env python3
"""Stage a KUAL distribution without overwriting an installed runtime or state."""
import argparse
import hashlib
from pathlib import Path
import shutil
import struct
import tomllib

HERE = Path(__file__).resolve().parent
APP = HERE.parent
MODEL = 'AJRLVDTOPT1LE'
FIRMWARE = 'System Software Version: 042-juno_1605_moonshine_rex-414285'
LEGACY_BINARY = 'e116ffce39c396bb8d101d3714a4b40b1c436459cb5e4f8f422fa9a6a670a4e8'
LEGACY_JOB = 'dcde5f72b4e2e13325e03993b5637e090b7509158bb536edf4fb7915f6d33641'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify_binary(path):
    data = path.read_bytes()
    if len(data) < 52 or data[:7] != b'\x7fELF\x01\x01\x01' or struct.unpack_from('<HH', data, 16) != (2, 40):
        raise ValueError('release must be a 32-bit little-endian ARM executable')
    offset = struct.unpack_from('<I', data, 28)[0]
    size, count = struct.unpack_from('<HH', data, 42)
    for index in range(count):
        start = offset + index * size
        if size < 32 or start + size > len(data):
            raise ValueError('invalid ELF program headers')
        if struct.unpack_from('<I', data, start)[0] in (2, 3):
            raise ValueError('release must have no dynamic segment or interpreter')


def stage(binary, output):
    verify_binary(binary)
    if output.exists():
        raise ValueError('staging output must be a new directory')
    version = tomllib.loads((APP / 'Cargo.toml').read_text())['package']['version']
    (output / 'bin').mkdir(parents=True)
    payload = output / 'payload'
    payload.mkdir()
    for name in ['config.xml', 'menu.json']:
        shutil.copyfile(HERE / 'kual' / name, output / name)
    shutil.copyfile(HERE / 'kual/bin/control.sh', output / 'bin/control.sh')
    (output / 'bin/control.sh').chmod(0o755)
    shutil.copyfile(binary, payload / 'monitor')
    (payload / 'monitor').chmod(0o755)
    automatic = (HERE / 'kindle-battery-monitor.conf').read_text()
    (payload / 'automatic.conf').write_text(automatic)
    (payload / 'on-demand.conf').write_text(''.join(line for line in automatic.splitlines(keepends=True) if not line.startswith('start on ')))
    manifest = {
        'format': '1', 'version': version, 'device_type': MODEL, 'firmware': FIRMWARE,
        'binary_sha256': digest(payload / 'monitor'),
        'automatic_sha256': digest(payload / 'automatic.conf'),
        'on_demand_sha256': digest(payload / 'on-demand.conf'),
        'controller_sha256': digest(output / 'bin/control.sh'),
        'menu_sha256': digest(output / 'menu.json'), 'config_sha256': digest(output / 'config.xml'),
        'legacy_binary_sha256': LEGACY_BINARY, 'legacy_job_sha256': LEGACY_JOB,
    }
    (payload / 'manifest').write_text(''.join(key + '=' + value + '\n' for key, value in manifest.items()))
    (output / 'README.txt').write_text(
        'Kindle battery monitor ' + version + '\n\n'
        'Tested Paperwhite 4 firmware 5.16.5 only.\n'
        'Extract the kindle-battery-monitor directory into Kindle/extensions/.\n'
        'Open KUAL > Kindle battery monitor > Install/update.\n'
        'Restart with USB disconnected; wait for Home/KUAL before reconnecting.\n'
        'Enable/Disable change automatic startup and the current session.\n'
        'Start/Stop change only the session. Status shows both.\n'
        'Updates preserve both settings. Wait for completion before another action.\n'
        'Uninstall requires confirmation and removes the entire extension and boot job.\n'
        'Home Assistant history remains intact.\n'
        'Keep a copy of this ZIP outside the Kindle if you want to reinstall.\n'
        'Do not delete the extension directory while the boot job is installed.\n'
        'If recovery is reported, inspect the result before invoking Install/update again.\n'
    )
    print(output)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    try:
        stage(args.binary, args.output)
    except (OSError, ValueError) as error:
        parser.exit(1, 'Packaging failed: ' + str(error) + '\n')
