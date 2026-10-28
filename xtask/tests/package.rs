//! Release and staging contracts through the host command interface.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
fn project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace")
        .to_owned()
}
fn arm_elf(segment: u32, machine: u16) -> Vec<u8> {
    let mut data = vec![0; 84];
    data[..7].copy_from_slice(b"\x7fELF\x01\x01\x01");
    data[16..18].copy_from_slice(&2u16.to_le_bytes());
    data[18..20].copy_from_slice(&machine.to_le_bytes());
    data[20..24].copy_from_slice(&1u32.to_le_bytes());
    data[28..32].copy_from_slice(&52u32.to_le_bytes());
    data[42..44].copy_from_slice(&32u16.to_le_bytes());
    data[44..46].copy_from_slice(&1u16.to_le_bytes());
    data[52..56].copy_from_slice(&segment.to_le_bytes());
    data
}
fn package(binary: &Path, output: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("package")
        .arg("--binary")
        .arg(binary)
        .arg("--output")
        .arg(output)
        .current_dir(output.parent().expect("temp parent"))
        .output()
        .expect("packager")
}
#[test]
fn test_non_arm_and_dynamic_binaries_emit_no_release_artifacts() {
    for (label, data) in [
        ("non-arm", arm_elf(1, 62)),
        ("dynamic", arm_elf(2, 40)),
        ("interpreter", arm_elf(3, 40)),
    ] {
        let root = tempfile::tempdir().expect("temp");
        let binary = root.path().join(label);
        fs::write(&binary, data).expect("ELF");
        let output = root.path().join("output");
        let result = package(&binary, &output);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("Packaging failed:"));
        assert!(!output.exists());
    }
}
fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
#[test]
fn test_staged_release_has_seven_controls_and_never_overwrites_live_runtime() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().expect("temp");
    let binary = root.path().join("monitor");
    fs::write(&binary, arm_elf(1, 40)).expect("ELF");
    let out = root.path().join("kindle-battery-monitor");
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["stage", "--binary"])
        .arg(&binary)
        .arg("--output")
        .arg(&out)
        .output()
        .expect("stage");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let menu: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("menu.json")).expect("menu")).expect("JSON");
    let items = menu["items"][0]["items"].as_array().expect("controls");
    assert_eq!(items.len(), 7);
    let params: std::collections::BTreeSet<_> =
        items.iter().filter_map(|x| x["params"].as_str()).collect();
    assert_eq!(
        params,
        ["install", "enable", "disable", "start", "stop", "status"]
            .into_iter()
            .collect()
    );
    let confirmation = items
        .iter()
        .find_map(|x| x["items"].as_array())
        .expect("uninstall confirmation");
    assert_eq!(
        confirmation
            .iter()
            .filter_map(|x| x["params"].as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        ["uninstall-confirm", "cancel"].into_iter().collect()
    );
    assert!(!out.join("state").exists());
    assert!(!out.join("bin/kindle-battery-monitor").exists());
    let manifest = fs::read_to_string(out.join("payload/manifest")).expect("manifest");
    let values: std::collections::BTreeMap<_, _> = manifest
        .lines()
        .map(|x| x.split_once('=').expect("field"))
        .collect();
    assert_eq!(values["device_type"], "AJRLVDTOPT1LE");
    for (name, key) in [
        ("monitor", "binary_sha256"),
        ("automatic.conf", "automatic_sha256"),
        ("on-demand.conf", "on_demand_sha256"),
    ] {
        assert_eq!(
            digest(&fs::read(out.join("payload").join(name)).expect("payload")),
            values[key]
        );
    }
    let automatic = fs::read_to_string(out.join("payload/automatic.conf")).expect("boot job");
    let manual = fs::read_to_string(out.join("payload/on-demand.conf")).expect("manual job");
    assert!(automatic
        .contains("start on (battery_ready and dbus_ready and started powerd and started btfd)"));
    assert!(!manual.contains("start on "));
    for job in [&automatic, &manual] {
        assert!(job.contains("kill timeout 50"));
        assert!(job.contains(
            "stop on (stopping lab126 or stopping dbus or stopping powerd or stopping btfd)"
        ));
        assert!(!job
            .lines()
            .any(|line| line.starts_with("respawn") || line.starts_with("manual")));
    }
    assert_eq!(
        fs::metadata(out.join("bin/control.sh"))
            .expect("mode")
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        fs::read(out.join("bin/control.sh")).expect("controller"),
        fs::read(project().join("packaging/kual/bin/control.sh")).expect("source controller")
    );
}
#[test]
fn test_versioned_zip_and_checksum_preserve_an_older_release() {
    let root = tempfile::Builder::new()
        .prefix("kindle package ")
        .tempdir()
        .expect("temp");
    let binary = root.path().join("monitor binary");
    fs::write(&binary, arm_elf(1, 40)).expect("ELF");
    let output = root.path().join("release output");
    fs::create_dir(&output).expect("output");
    let previous = output.join("kindle-battery-monitor-0.0.1.zip");
    fs::write(&previous, b"previous release").expect("old archive");
    let result = package(&binary, &output);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let cargo: toml::Value =
        toml::from_str(&fs::read_to_string(project().join("Cargo.toml")).expect("manifest"))
            .expect("TOML");
    let version = cargo["package"]["version"].as_str().expect("version");
    let archive = output.join(format!("kindle-battery-monitor-{version}.zip"));
    let name = archive.file_name().expect("name").to_string_lossy();
    let checksum = output.join(format!("{name}.sha256"));
    assert_eq!(
        fs::read_to_string(checksum).expect("sidecar"),
        format!(
            "{}  {name}\n",
            digest(&fs::read(&archive).expect("archive"))
        )
    );
    assert_eq!(fs::read(previous).expect("old"), b"previous release");
    let mut zip = zip::ZipArchive::new(fs::File::open(archive).expect("archive")).expect("ZIP");
    let mut contents = std::collections::BTreeMap::new();
    for i in 0..zip.len() {
        use std::io::Read;
        let mut entry = zip.by_index(i).expect("entry");
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("entry bytes");
        assert!(!entry.name().contains("/state/"));
        assert!(!entry.name().ends_with("/bin/kindle-battery-monitor"));
        if entry.name().ends_with("/bin/control.sh") {
            assert_eq!(entry.unix_mode().expect("Unix mode") & 0o111, 0o111);
        }
        contents.insert(entry.name().to_owned(), bytes);
    }
    let prefix = "kindle-battery-monitor/";
    let manifest = String::from_utf8(contents[&format!("{prefix}payload/manifest")].clone())
        .expect("manifest");
    let values: std::collections::BTreeMap<_, _> = manifest
        .lines()
        .map(|line| line.split_once('=').expect("field"))
        .collect();
    assert_eq!(values["version"], version);
    assert_eq!(
        contents[&format!("{prefix}LICENSE")],
        fs::read(project().join("LICENSE")).expect("license")
    );
    for (field, name) in [
        ("binary_sha256", "payload/monitor"),
        ("automatic_sha256", "payload/automatic.conf"),
        ("on_demand_sha256", "payload/on-demand.conf"),
        ("controller_sha256", "bin/control.sh"),
        ("menu_sha256", "menu.json"),
        ("config_sha256", "config.xml"),
    ] {
        assert_eq!(digest(&contents[&format!("{prefix}{name}")]), values[field]);
    }
}
