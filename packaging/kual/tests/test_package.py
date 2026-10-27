"""Architecture invariant: the distribution layout is a KUAL/upgrade contract."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

PACKAGING=Path(__file__).resolve().parents[2]


class PackageTests(unittest.TestCase):
    def test_staged_release_has_seven_controls_and_never_overwrites_live_runtime(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); binary=root/'monitor'; out=root/'kindle-battery-monitor'
            elf=bytearray(64); elf[:7]=b'\x7fELF\x01\x01\x01'; elf[16:20]=b'\x02\x00\x28\x00'
            binary.write_bytes(elf+b'fixture')
            result=subprocess.run([sys.executable,str(PACKAGING/'package.py'),'--binary',str(binary),'--output',str(out)],capture_output=True,text=True)
            self.assertEqual(result.returncode,0,result.stderr)
            menu=json.loads((out/'menu.json').read_text())
            items=menu['items'][0]['items']
            self.assertEqual(len(items),7)
            params={x.get('params') for x in items}
            self.assertEqual(params- {None},{'install','enable','disable','start','stop','status'})
            confirmation=next(x for x in items if 'items' in x)['items']
            self.assertEqual({x['params'] for x in confirmation},{'uninstall-confirm','cancel'})
            self.assertFalse((out/'bin/kindle-battery-monitor').exists())
            self.assertFalse((out/'state').exists())
            self.assertIn('AJRLVDTOPT1LE',(out/'payload/manifest').read_text())
            values=dict(x.split('=',1) for x in (out/'payload/manifest').read_text().splitlines())
            for name,key in [('monitor','binary_sha256'),('automatic.conf','automatic_sha256'),('on-demand.conf','on_demand_sha256')]:
                self.assertEqual(hashlib.sha256((out/'payload'/name).read_bytes()).hexdigest(),values[key])
            automatic=(out/'payload/automatic.conf').read_text(); manual=(out/'payload/on-demand.conf').read_text()
            self.assertIn('start on (battery_ready and dbus_ready and started powerd and started btfd)',automatic)
            self.assertNotIn('start on ',manual)
            for job in [automatic,manual]:
                self.assertIn('kill timeout 50',job)
                self.assertIn('stop on (stopping lab126 or stopping dbus or stopping powerd or stopping btfd)',job)
                self.assertFalse(any(line.startswith(('respawn','manual')) for line in job.splitlines()))


if __name__=='__main__': unittest.main()
