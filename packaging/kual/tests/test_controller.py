"""Behavior tests at the KUAL management command boundary, with no device IO."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

PACKAGING = Path(__file__).resolve().parents[2]
CONTROLLER = PACKAGING / 'kual' / 'bin' / 'control.sh'
MODEL = 'AJRLVDTOPT1LE'
FIRMWARE = 'System Software Version: 042-juno_1605_moonshine_rex-414285'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class FakeKindle:
    """Isolated filesystem and named fake initctl/mount/feedback boundaries."""
    def __init__(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.extension = self.root / 'mnt/us/extensions/kindle-battery-monitor'
        self.commands = self.root / 'fake-commands'
        for directory in ['proc/idme', 'etc/upstart', 'var/tmp', 'var/log', 'usr/sbin', 'usr/bin', 'fake-commands']:
            (self.root / directory).mkdir(parents=True, exist_ok=True)
        (self.root / 'proc/idme/device_type_id').write_bytes(MODEL.encode() + b'\0')
        (self.root / 'etc/version.txt').write_text(FIRMWARE + '\n')
        (self.root / 'proc/mounts').write_text('/dev/root / ext3 ro,noatime 0 0\n')
        for name in ['powerd.conf', 'btfd.conf', 'dbus.conf']:
            (self.root / 'etc/upstart' / name).write_text('existing stock service\n')
        for name in ['bsa_server','btfd']:
            executable=self.root/'usr/bin'/name; executable.write_text('fixture'); executable.chmod(0o755)
        self.extension.mkdir(parents=True)
        self.state_file = self.root / 'fake-state.json'
        self.save(running=False, installed=False, events=[], mount='ro')
        fake_command = '''#!PYTHON
import json, os, pathlib, sys
root=pathlib.Path(os.environ['KINDLE_BATTERY_ROOT']); path=root/'fake-state.json'
s=json.loads(path.read_text()); name=pathlib.Path(sys.argv[0]).name; args=sys.argv[1:]
s['events'].append([name]+args)
fail=os.environ.get('FAKE_FAIL','').split(',')
if name == 'initctl':
 op=args[0]
 once=op+'-once' in fail and not s.get('failed_'+op,False)
 if op in fail or once:
  s['failed_'+op]=True
  path.write_text(json.dumps(s)); print('injected '+op+' failure',file=sys.stderr); sys.exit(1)
 if op == 'status':
  if not s['installed']: path.write_text(json.dumps(s)); print('initctl: Unknown job',file=sys.stderr); sys.exit(1)
  print('kindle-battery-monitor '+('start/running, process 4321' if s['running'] else 'stop/waiting'))
 elif op == 'start': s['running']=True
 elif op == 'stop':
  s['running']=False
  event='monitor_failed' if 'cleanup' in fail else 'monitor_stopped'
  with (root/'var/log/kindle-battery-monitor.jsonl').open('a') as f: f.write(json.dumps({'timestamp':len(s['events']),'fields':{'event':event}})+'\\n')
 elif op == 'reload-configuration': s['installed']=(root/'etc/upstart/kindle-battery-monitor.conf').exists()
elif name == 'mntroot':
 if args[0] in fail: path.write_text(json.dumps(s)); sys.exit(1)
 s['mount']=args[0]; (root/'proc/mounts').write_text('/dev/root / ext3 '+args[0]+',noatime 0 0\\n')
elif name == 'lipc-get-prop': print('99')
elif name == 'eips': pass
elif name == 'flock':
 if 'busy' in fail: path.write_text(json.dumps(s)); sys.exit(1)
elif name == 'cp' and 'copy' in fail and args[0].endswith('/payload/monitor'):
 path.write_text(json.dumps(s)); print('injected copy failure',file=sys.stderr); sys.exit(1)
elif name == 'mv' and 'swap' in fail and args[-1].endswith('/bin/kindle-battery-monitor'):
 path.write_text(json.dumps(s)); print('injected swap failure',file=sys.stderr); sys.exit(1)
else:
 import subprocess
 result=subprocess.run(['/bin/'+name]+args); path.write_text(json.dumps(s)); sys.exit(result.returncode)
path.write_text(json.dumps(s))
'''.replace('PYTHON', sys.executable)
        for name in ['initctl', 'mntroot', 'lipc-get-prop', 'eips', 'flock', 'mv', 'cp']:
            target = self.commands / name
            target.write_text(fake_command)
            target.chmod(0o755)

    def save(self, **state):
        self.state_file.write_text(json.dumps(state))

    @property
    def state(self):
        return json.loads(self.state_file.read_text())

    def stage(self):
        payload = self.extension / 'payload'
        payload.mkdir()
        elf = bytearray(64)
        elf[:7] = b'\x7fELF\x01\x01\x01'
        elf[16:20] = b'\x02\x00\x28\x00'
        (payload / 'monitor').write_bytes(elf + b'tested ARM payload')
        enabled = (PACKAGING / 'kindle-battery-monitor.conf').read_text()
        (payload / 'automatic.conf').write_text(enabled)
        (payload / 'on-demand.conf').write_text('\n'.join(line for line in enabled.splitlines() if not line.startswith('start on ')) + '\n')
        manifest = {'format':'1', 'version':'0.1.0', 'device_type':MODEL, 'firmware':FIRMWARE, 'controller_sha256':digest(CONTROLLER), 'binary_sha256':digest(payload/'monitor'),
                    'automatic_sha256':digest(payload/'automatic.conf'), 'on_demand_sha256':digest(payload/'on-demand.conf'),
                    'legacy_binary_sha256':digest(payload/'monitor'), 'legacy_job_sha256':digest(payload/'automatic.conf')}
        (payload / 'manifest').write_text(''.join(k+'='+v+'\n' for k,v in manifest.items()))

    def installed(self, enabled=True, running=False):
        if not (self.extension/'payload').exists():
            self.stage()
        (self.extension/'bin').mkdir(exist_ok=True)
        (self.extension/'state').mkdir(exist_ok=True)
        shutil.copyfile(self.extension/'payload/monitor', self.extension/'bin/kindle-battery-monitor')
        job = self.root/'etc/upstart/kindle-battery-monitor.conf'
        shutil.copyfile(self.extension/'payload'/('automatic.conf' if enabled else 'on-demand.conf'), job)
        (self.extension/'state/version').write_text('0.1.0\n')
        (self.extension/'state/binary.sha256').write_text(digest(self.extension/'bin/kindle-battery-monitor')+'\n')
        (self.extension/'state/job.sha256').write_text(digest(job)+'\n')
        state=self.state; state.update(installed=True, running=running); self.save(**state)

    def run(self, action, fail=''):
        env = dict(os.environ, KINDLE_BATTERY_ROOT=str(self.root), FAKE_FAIL=fail,
                   PATH=str(self.commands) + os.pathsep + os.environ['PATH'])
        return subprocess.run(['sh', str(CONTROLLER), action], env=env, text=True,
                              capture_output=True, timeout=15)

    def close(self):
        self.temp.cleanup()


class ControllerTests(unittest.TestCase):
    def setUp(self):
        self.kindle = FakeKindle()
        self.addCleanup(self.kindle.close)

    def test_unsupported_target_refuses_before_any_system_mutation(self):
        (self.kindle.root / 'proc/idme/device_type_id').write_text('UNSUPPORTED')
        result = self.kindle.run('install')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('unsupported', result.stdout.lower() + result.stderr.lower())
        self.assertEqual(self.kindle.state['events'], [])
        self.assertFalse((self.kindle.root / 'etc/upstart/kindle-battery-monitor.conf').exists())

    def test_status_reports_uninstalled_without_mount_or_service_changes(self):
        result = self.kindle.run('status')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('Not installed', result.stdout)
        self.assertFalse(any(e[0] == 'mntroot' or e[:2] in [['initctl','start'],['initctl','stop']] for e in self.kindle.state['events']))

    def test_install_starts_once_enables_boot_and_restores_read_only_root(self):
        self.kindle.stage()
        result = self.kindle.run('install')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(self.kindle.state['running'])
        job = self.kindle.root / 'etc/upstart/kindle-battery-monitor.conf'
        self.assertIn('start on ', job.read_text())
        self.assertEqual(self.kindle.state['mount'], 'ro')
        self.assertEqual((self.kindle.extension/'bin/kindle-battery-monitor').read_bytes(), (self.kindle.extension/'payload/monitor').read_bytes())
        self.assertIn('Installed', result.stdout)

    def test_session_and_boot_controls_are_independent_and_idempotent(self):
        for enabled in [False, True]:
            for running in [False, True]:
                for action in ['start', 'stop', 'enable', 'disable']:
                    with self.subTest(enabled=enabled, running=running, action=action):
                        self.kindle.installed(enabled, running)
                        result=self.kindle.run(action)
                        self.assertEqual(result.returncode, 0, result.stderr)
                        want_running=action in ['start', 'enable']
                        want_enabled=enabled if action in ['start','stop'] else action == 'enable'
                        self.assertEqual(self.kindle.state['running'], want_running)
                        self.assertEqual('start on ' in (self.kindle.root/'etc/upstart/kindle-battery-monitor.conf').read_text(), want_enabled)
                        before=len([e for e in self.kindle.state['events'] if e[:2] in [['initctl','start'], ['initctl','stop']]])
                        again=self.kindle.run(action)
                        self.assertEqual(again.returncode, 0, again.stderr)
                        after=len([e for e in self.kindle.state['events'] if e[:2] in [['initctl','start'], ['initctl','stop']]])
                        self.assertEqual(after,before)
                        self.assertEqual(self.kindle.state['mount'],'ro')

    def test_update_preserves_session_and_boot_state(self):
        for enabled in [False,True]:
            for running in [False,True]:
                with self.subTest(enabled=enabled,running=running):
                    self.kindle.installed(enabled,running)
                    result=self.kindle.run('install')
                    self.assertEqual(result.returncode,0,result.stderr)
                    self.assertEqual(self.kindle.state['running'],running)
                    self.assertEqual('start on ' in (self.kindle.root/'etc/upstart/kindle-battery-monitor.conf').read_text(),enabled)
                    self.assertEqual(self.kindle.state['mount'],'ro')

    def test_symlink_destination_is_refused_without_touching_foreign_file(self):
        self.kindle.stage()
        foreign=self.kindle.root/'foreign'; foreign.write_text('foreign data')
        (self.kindle.root/'etc/upstart/kindle-battery-monitor.conf').symlink_to(foreign)
        result=self.kindle.run('install')
        self.assertNotEqual(result.returncode,0)
        self.assertIn('symlink',result.stderr.lower())
        self.assertEqual(foreign.read_text(),'foreign data')
        self.assertFalse(any(e[0]=='mntroot' for e in self.kindle.state['events']))

    def test_failed_update_restores_previous_files_and_running_state(self):
        self.kindle.installed(enabled=False,running=True)
        old=(self.kindle.extension/'bin/kindle-battery-monitor').read_bytes()
        payload=self.kindle.extension/'payload/monitor'
        payload.write_bytes(payload.read_bytes()+b'new release')
        manifest=self.kindle.extension/'payload/manifest'
        lines=manifest.read_text().splitlines()
        manifest.write_text('\n'.join('binary_sha256='+digest(payload) if x.startswith('binary_sha256=') else x for x in lines)+'\n')
        result=self.kindle.run('install',fail='reload-configuration-once')
        self.assertNotEqual(result.returncode,0)
        self.assertEqual((self.kindle.extension/'bin/kindle-battery-monitor').read_bytes(),old)
        self.assertTrue(self.kindle.state['running'])
        self.assertNotIn('start on ',(self.kindle.root/'etc/upstart/kindle-battery-monitor.conf').read_text())
        self.assertEqual(self.kindle.state['mount'],'ro')
        self.assertIn('recovered',result.stderr.lower())

    def test_full_uninstall_requires_confirmation_and_removes_owned_files(self):
        self.kindle.installed(enabled=True,running=True)
        log=self.kindle.root/'var/log/kindle-battery-monitor.jsonl'; log.write_text('diagnostics')
        foreign=self.kindle.root/'etc/upstart/unrelated.conf'; foreign.write_text('foreign')
        cancelled=self.kindle.run('cancel')
        self.assertEqual(cancelled.returncode,0,cancelled.stderr)
        self.assertTrue(self.kindle.state['running'])
        unconfirmed=self.kindle.run('uninstall')
        self.assertNotEqual(unconfirmed.returncode,0)
        self.assertTrue(self.kindle.extension.exists())
        result=self.kindle.run('uninstall-confirm')
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertFalse(self.kindle.extension.exists())
        self.assertFalse((self.kindle.root/'etc/upstart/kindle-battery-monitor.conf').exists())
        self.assertFalse(self.kindle.state['running'])
        self.assertFalse(log.exists())
        self.assertEqual(foreign.read_text(),'foreign')
        self.assertEqual(self.kindle.state['mount'],'ro')

    def test_cleanup_failure_aborts_update_before_replacing_files(self):
        self.kindle.installed(enabled=True,running=True)
        job=self.kindle.root/'etc/upstart/kindle-battery-monitor.conf'; before=job.read_bytes()
        result=self.kindle.run('install',fail='cleanup')
        self.assertNotEqual(result.returncode,0)
        self.assertEqual(job.read_bytes(),before)
        self.assertFalse((self.kindle.extension/'state/transaction').exists())
        self.assertIn('cleanup',result.stderr.lower())

    def test_interrupted_update_recovers_verified_previous_installation(self):
        self.kindle.installed(enabled=False,running=False)
        ext=self.kindle.extension; tx=ext/'state/transaction'; tx.mkdir()
        for source,target in [('bin/kindle-battery-monitor','monitor'),('state/version','version'),('state/binary.sha256','binary.sha256'),('state/job.sha256','job.sha256')]:
            shutil.copyfile(ext/source,tx/target)
        shutil.copyfile(self.kindle.root/'etc/upstart/kindle-battery-monitor.conf',tx/'job')
        (tx/'had-install').write_text('yes\n'); (tx/'running').write_text('no\n'); (tx/'ready').write_text('ready\n')
        (ext/'bin/kindle-battery-monitor').write_text('interrupted replacement')
        result=self.kindle.run('install')
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual((ext/'bin/kindle-battery-monitor').read_bytes(),(ext/'payload/monitor').read_bytes())
        self.assertFalse(tx.exists())
        self.assertIn('Recovered',result.stdout)
        self.assertFalse(self.kindle.state['running'])

    def test_failed_boot_setting_reload_restores_previous_job(self):
        self.kindle.installed(enabled=True,running=False)
        job=self.kindle.root/'etc/upstart/kindle-battery-monitor.conf'; old=job.read_bytes()
        result=self.kindle.run('disable',fail='reload-configuration-once')
        self.assertNotEqual(result.returncode,0)
        self.assertEqual(job.read_bytes(),old)
        self.assertEqual(self.kindle.state['mount'],'ro')

    def test_corruption_unknown_ownership_and_busy_are_safe_refusals(self):
        for scenario in ['corrupt','foreign-job','foreign-binary','busy']:
            with self.subTest(scenario=scenario):
                fake=FakeKindle(); self.addCleanup(fake.close); fake.installed(True,True)
                job=fake.root/'etc/upstart/kindle-battery-monitor.conf'
                if scenario=='corrupt': (fake.extension/'payload/monitor').write_text('corrupt')
                elif scenario=='foreign-job': job.write_text('foreign startup job')
                elif scenario=='foreign-binary': (fake.extension/'bin/kindle-battery-monitor').write_text('foreign runtime')
                before=job.read_bytes()
                result=fake.run('install',fail='busy' if scenario=='busy' else '')
                self.assertNotEqual(result.returncode,0)
                self.assertEqual(job.read_bytes(),before)
                self.assertTrue(fake.state['running'])
                self.assertFalse(any(e[0]=='mntroot' for e in fake.state['events']))

    def test_stop_start_swap_and_recovery_failures_never_report_success(self):
        for failure in ['stop','start','swap','reload-configuration']:
            with self.subTest(failure=failure):
                fake=FakeKindle(); self.addCleanup(fake.close); fake.installed(True,True)
                old=(fake.extension/'bin/kindle-battery-monitor').read_bytes()
                result=fake.run('install',fail=failure)
                self.assertNotEqual(result.returncode,0)
                self.assertNotIn('Installed',result.stdout)
                self.assertEqual((fake.extension/'bin/kindle-battery-monitor').read_bytes(),old)
                self.assertEqual(fake.state['mount'],'ro')
                if failure=='stop': self.assertTrue(fake.state['running'])
                else: self.assertFalse(fake.state['running'])
                if failure in ['start','reload-configuration']:
                    self.assertIn('recovery failed',result.stderr.lower())
                    self.assertTrue((fake.extension/'state/transaction').exists())

    def test_writable_root_is_preserved_and_read_only_restore_failure_is_visible(self):
        self.kindle.stage()
        state=self.kindle.state; state['mount']='rw'; self.kindle.save(**state)
        (self.kindle.root/'proc/mounts').write_text('/dev/root / ext3 rw,noatime 0 0\n')
        result=self.kindle.run('install')
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(self.kindle.state['mount'],'rw')
        self.assertFalse(any(e[0]=='mntroot' for e in self.kindle.state['events']))
        fake=FakeKindle(); self.addCleanup(fake.close); fake.stage()
        result=fake.run('install',fail='ro')
        self.assertNotEqual(result.returncode,0)
        self.assertIn('root mount restoration failed',result.stderr)
        self.assertNotIn('Installed',result.stdout)

    def test_known_manual_installation_is_adopted_without_changing_state(self):
        self.kindle.installed(True,True)
        shutil.rmtree(self.kindle.extension/'state')
        result=self.kindle.run('install')
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertTrue(self.kindle.state['running'])
        self.assertEqual((self.kindle.extension/'state/version').read_text().strip(),'0.1.0')

    def test_uninstall_stop_failure_retains_extension_and_hook(self):
        self.kindle.installed(True,True)
        result=self.kindle.run('uninstall-confirm',fail='stop')
        self.assertNotEqual(result.returncode,0)
        self.assertTrue(self.kindle.extension.exists())
        self.assertTrue((self.kindle.root/'etc/upstart/kindle-battery-monitor.conf').exists())

    def test_release_for_a_different_target_is_rejected_before_mount_changes(self):
        self.kindle.stage()
        manifest=self.kindle.extension/'payload/manifest'
        manifest.write_text(manifest.read_text().replace('device_type='+MODEL,'device_type=OTHER'))
        result=self.kindle.run('install')
        self.assertNotEqual(result.returncode,0)
        self.assertFalse(any(e[0]=='mntroot' for e in self.kindle.state['events']))

    def test_payload_copy_failure_recovers_previous_installation(self):
        self.kindle.installed(enabled=True,running=True)
        binary=self.kindle.extension/'bin/kindle-battery-monitor'
        old=binary.read_bytes()
        result=self.kindle.run('install',fail='copy')
        self.assertNotEqual(result.returncode,0)
        self.assertEqual(binary.read_bytes(),old)
        self.assertTrue(self.kindle.state['running'])
        self.assertEqual(self.kindle.state['mount'],'ro')
        self.assertIn('recovered',result.stderr.lower())


if __name__ == '__main__':
    unittest.main()
