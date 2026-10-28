//! Isolated filesystem and process fixtures shared by host command tests.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output},
};
pub fn project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace")
        .to_owned()
}
pub fn write(path: impl AsRef<Path>, value: impl AsRef<[u8]>) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().expect("parent")).expect("directories");
    fs::write(path, value).expect("file");
}
pub fn digest(path: impl AsRef<Path>) -> String {
    format!(
        "{:x}",
        Sha256::digest(fs::read(path).expect("digest input"))
    )
}
pub fn executable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("executable");
}
pub fn install_tool(dir: &Path, name: &str) {
    fs::create_dir_all(dir).expect("tool directory");
    symlink(env!("CARGO_BIN_EXE_fixture_tool"), dir.join(name)).expect("compiled fixture tool");
}
pub fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub struct FakeKindle {
    _temp: tempfile::TempDir,
    pub root: PathBuf,
    pub extension: PathBuf,
    commands: PathBuf,
}
impl FakeKindle {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().expect("isolated Kindle");
        let root = temp.path().to_owned();
        let extension = root.join("mnt/us/extensions/kindle-battery-monitor");
        let commands = root.join("fake-commands");
        for directory in [
            "proc/idme",
            "etc/upstart",
            "var/tmp",
            "var/log",
            "usr/sbin",
            "usr/bin",
            "fake-commands",
        ] {
            fs::create_dir_all(root.join(directory)).expect("fixture directory");
        }
        write(root.join("proc/idme/device_type_id"), b"AJRLVDTOPT1LE\0");
        write(
            root.join("etc/version.txt"),
            b"System Software Version: 042-juno_1605_moonshine_rex-414285\n",
        );
        write(
            root.join("proc/mounts"),
            b"/dev/root / ext3 ro,noatime 0 0\n",
        );
        for name in ["powerd.conf", "btfd.conf", "dbus.conf"] {
            write(
                root.join("etc/upstart").join(name),
                "existing stock service\n",
            );
        }
        for name in ["bsa_server", "btfd"] {
            let path = root.join("usr/bin").join(name);
            write(&path, "fixture");
            executable(&path);
        }
        fs::create_dir_all(&extension).expect("extension");
        for name in [
            "initctl",
            "mntroot",
            "lipc-get-prop",
            "eips",
            "flock",
            "mv",
            "cp",
        ] {
            install_tool(&commands, name);
        }
        let fixture = Self {
            _temp: temp,
            root,
            extension,
            commands,
        };
        fixture.save(json!({"running":false,"installed":false,"events":[],"mount":"ro"}));
        fixture
    }
    pub fn state(&self) -> Value {
        serde_json::from_slice(&fs::read(self.root.join("fake-state.json")).expect("state"))
            .expect("JSON state")
    }
    pub fn save(&self, state: Value) {
        write(
            self.root.join("fake-state.json"),
            serde_json::to_vec(&state).expect("state JSON"),
        );
    }
    pub fn stage(&self) {
        let payload = self.extension.join("payload");
        fs::create_dir_all(&payload).expect("payload");
        let mut elf = vec![0; 64];
        elf[..7].copy_from_slice(b"\x7fELF\x01\x01\x01");
        elf[16..20].copy_from_slice(b"\x02\x00\x28\x00");
        elf.extend_from_slice(b"tested ARM payload");
        write(payload.join("monitor"), elf);
        let enabled = fs::read_to_string(project().join("packaging/kindle-battery-monitor.conf"))
            .expect("job template");
        write(payload.join("automatic.conf"), &enabled);
        let disabled = enabled
            .lines()
            .filter(|line| !line.starts_with("start on "))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        write(payload.join("on-demand.conf"), disabled);
        let manifest=format!("format=1\nversion=0.1.0\ndevice_type=AJRLVDTOPT1LE\nfirmware=System Software Version: 042-juno_1605_moonshine_rex-414285\ncontroller_sha256={}\nbinary_sha256={}\nautomatic_sha256={}\non_demand_sha256={}\nlegacy_binary_sha256={}\nlegacy_job_sha256={}\n",digest(project().join("packaging/kual/bin/control.sh")),digest(payload.join("monitor")),digest(payload.join("automatic.conf")),digest(payload.join("on-demand.conf")),digest(payload.join("monitor")),digest(payload.join("automatic.conf")));
        write(payload.join("manifest"), manifest);
    }
    pub fn installed(&self, enabled: bool, running: bool) {
        if !self.extension.join("payload").exists() {
            self.stage();
        }
        for directory in ["bin", "state"] {
            fs::create_dir_all(self.extension.join(directory)).expect("install directory");
        }
        let binary = self.extension.join("bin/kindle-battery-monitor");
        fs::copy(self.extension.join("payload/monitor"), &binary).expect("live binary");
        let job = self.root.join("etc/upstart/kindle-battery-monitor.conf");
        fs::copy(
            self.extension.join(if enabled {
                "payload/automatic.conf"
            } else {
                "payload/on-demand.conf"
            }),
            &job,
        )
        .expect("live job");
        write(self.extension.join("state/version"), "0.1.0\n");
        write(
            self.extension.join("state/binary.sha256"),
            digest(&binary) + "\n",
        );
        write(self.extension.join("state/job.sha256"), digest(&job) + "\n");
        let mut state = self.state();
        state["installed"] = json!(true);
        state["running"] = json!(running);
        self.save(state);
    }
    pub fn run(&self, action: &str, fail: &str) -> Output {
        let path = std::env::join_paths(std::iter::once(self.commands.clone()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .expect("PATH");
        Command::new("timeout")
            .args(["15", "sh"])
            .arg(project().join("packaging/kual/bin/control.sh"))
            .arg(action)
            .env("KINDLE_BATTERY_ROOT", &self.root)
            .env("FAKE_FAIL", fail)
            .env("PATH", path)
            .output()
            .expect("bounded controller fixture")
    }
    pub fn changed_boot(&self) -> bool {
        fs::read_to_string(self.root.join("etc/upstart/kindle-battery-monitor.conf"))
            .expect("job")
            .contains("start on ")
    }
}
