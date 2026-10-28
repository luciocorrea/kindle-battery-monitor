//! KUAL behavior at its management command seam, without any device IO.
mod support;
use std::fs;
use support::*;
#[test]
fn test_unsupported_target_refuses_before_any_system_mutation() {
    let kindle = FakeKindle::new();
    write(kindle.root.join("proc/idme/device_type_id"), "UNSUPPORTED");
    let result = kindle.run("install", "");
    assert!(!result.status.success());
    assert!(format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    )
    .to_lowercase()
    .contains("unsupported"));
    assert_eq!(kindle.state()["events"], serde_json::json!([]));
    assert!(!kindle
        .root
        .join("etc/upstart/kindle-battery-monitor.conf")
        .exists());
}
#[test]
fn test_install_starts_once_enables_boot_and_restores_read_only_root() {
    let kindle = FakeKindle::new();
    kindle.stage();
    let result = kindle.run("install", "");
    success(&result);
    assert_eq!(kindle.state()["running"], true);
    assert!(kindle.changed_boot());
    assert_eq!(kindle.state()["mount"], "ro");
    assert_eq!(
        fs::read(kindle.extension.join("bin/kindle-battery-monitor")).expect("live"),
        fs::read(kindle.extension.join("payload/monitor")).expect("payload")
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("Installed"));
}
fn events(kindle: &FakeKindle) -> Vec<serde_json::Value> {
    kindle.state()["events"].as_array().expect("events").clone()
}
fn mounts(kindle: &FakeKindle) -> bool {
    events(kindle).iter().any(|e| e[0] == "mntroot")
}
fn changes(kindle: &FakeKindle) -> usize {
    events(kindle)
        .iter()
        .filter(|e| e[0] == "initctl" && (e[1] == "start" || e[1] == "stop"))
        .count()
}
fn stderr(result: &std::process::Output) -> String {
    String::from_utf8_lossy(&result.stderr).into_owned()
}
fn stdout(result: &std::process::Output) -> String {
    String::from_utf8_lossy(&result.stdout).into_owned()
}

#[test]
fn test_status_reports_uninstalled_without_mount_or_service_changes() {
    let kindle = FakeKindle::new();
    let result = kindle.run("status", "");
    success(&result);
    assert!(stdout(&result).contains("Not installed"));
    assert!(!mounts(&kindle));
    assert_eq!(changes(&kindle), 0);
}

#[test]
fn test_session_and_boot_controls_are_independent_and_idempotent() {
    let kindle = FakeKindle::new();
    for enabled in [false, true] {
        for running in [false, true] {
            for action in ["start", "stop", "enable", "disable"] {
                kindle.installed(enabled, running);
                success(&kindle.run(action, ""));
                let want_running = action == "start" || action == "enable";
                let want_enabled = if action == "start" || action == "stop" {
                    enabled
                } else {
                    action == "enable"
                };
                assert_eq!(kindle.state()["running"], want_running);
                assert_eq!(kindle.changed_boot(), want_enabled);
                let before = changes(&kindle);
                success(&kindle.run(action, ""));
                assert_eq!(changes(&kindle), before);
                assert_eq!(kindle.state()["mount"], "ro");
            }
        }
    }
}

#[test]
fn test_update_preserves_session_and_boot_state() {
    let kindle = FakeKindle::new();
    for enabled in [false, true] {
        for running in [false, true] {
            kindle.installed(enabled, running);
            success(&kindle.run("install", ""));
            assert_eq!(kindle.state()["running"], running);
            assert_eq!(kindle.changed_boot(), enabled);
            assert_eq!(kindle.state()["mount"], "ro");
        }
    }
}

#[test]
fn test_symlink_destination_is_refused_without_touching_foreign_file() {
    let kindle = FakeKindle::new();
    kindle.stage();
    let foreign = kindle.root.join("foreign");
    write(&foreign, "foreign data");
    std::os::unix::fs::symlink(
        &foreign,
        kindle.root.join("etc/upstart/kindle-battery-monitor.conf"),
    )
    .expect("symlink");
    let result = kindle.run("install", "");
    assert!(!result.status.success());
    assert!(stderr(&result).to_lowercase().contains("symlink"));
    assert_eq!(
        fs::read_to_string(foreign).expect("foreign"),
        "foreign data"
    );
    assert!(!mounts(&kindle));
}

#[test]
fn test_failed_update_restores_previous_files_and_running_state() {
    let kindle = FakeKindle::new();
    kindle.installed(false, true);
    let binary = kindle.extension.join("bin/kindle-battery-monitor");
    let old = fs::read(&binary).expect("old");
    let payload = kindle.extension.join("payload/monitor");
    let mut bytes = fs::read(&payload).expect("payload");
    bytes.extend_from_slice(b"new release");
    write(&payload, bytes);
    let manifest = kindle.extension.join("payload/manifest");
    let text = fs::read_to_string(&manifest).expect("manifest");
    write(
        &manifest,
        text.lines()
            .map(|line| {
                if line.starts_with("binary_sha256=") {
                    format!("binary_sha256={}", digest(&payload))
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    );
    let result = kindle.run("install", "reload-configuration-once");
    assert!(!result.status.success());
    assert_eq!(fs::read(binary).expect("restored"), old);
    assert_eq!(kindle.state()["running"], true);
    assert!(!kindle.changed_boot());
    assert_eq!(kindle.state()["mount"], "ro");
    assert!(stderr(&result).to_lowercase().contains("recovered"));
}

#[test]
fn test_full_uninstall_requires_confirmation_and_removes_owned_files() {
    let kindle = FakeKindle::new();
    kindle.installed(true, true);
    let log = kindle.root.join("var/log/kindle-battery-monitor.jsonl");
    write(&log, "diagnostics");
    let foreign = kindle.root.join("etc/upstart/unrelated.conf");
    write(&foreign, "foreign");
    success(&kindle.run("cancel", ""));
    assert_eq!(kindle.state()["running"], true);
    assert!(!kindle.run("uninstall", "").status.success());
    assert!(kindle.extension.exists());
    success(&kindle.run("uninstall-confirm", ""));
    assert!(!kindle.extension.exists());
    assert!(!kindle
        .root
        .join("etc/upstart/kindle-battery-monitor.conf")
        .exists());
    assert_eq!(kindle.state()["running"], false);
    assert!(!log.exists());
    assert_eq!(fs::read_to_string(foreign).expect("foreign"), "foreign");
    assert_eq!(kindle.state()["mount"], "ro");
}

#[test]
fn test_cleanup_failure_aborts_update_before_replacing_files() {
    let kindle = FakeKindle::new();
    kindle.installed(true, true);
    let job = kindle.root.join("etc/upstart/kindle-battery-monitor.conf");
    let before = fs::read(&job).expect("job");
    let result = kindle.run("install", "cleanup");
    assert!(!result.status.success());
    assert_eq!(fs::read(job).expect("job"), before);
    assert!(!kindle.extension.join("state/transaction").exists());
    assert!(stderr(&result).to_lowercase().contains("cleanup"));
}

#[test]
fn test_interrupted_update_recovers_verified_previous_installation() {
    let kindle = FakeKindle::new();
    kindle.installed(false, false);
    let tx = kindle.extension.join("state/transaction");
    fs::create_dir(&tx).expect("transaction");
    for (source, target) in [
        ("bin/kindle-battery-monitor", "monitor"),
        ("state/version", "version"),
        ("state/binary.sha256", "binary.sha256"),
        ("state/job.sha256", "job.sha256"),
    ] {
        fs::copy(kindle.extension.join(source), tx.join(target)).expect("recovery copy");
    }
    fs::copy(
        kindle.root.join("etc/upstart/kindle-battery-monitor.conf"),
        tx.join("job"),
    )
    .expect("job");
    for (file, text) in [
        ("had-install", "yes\n"),
        ("running", "no\n"),
        ("ready", "ready\n"),
    ] {
        write(tx.join(file), text);
    }
    write(
        kindle.extension.join("bin/kindle-battery-monitor"),
        "interrupted replacement",
    );
    let result = kindle.run("install", "");
    success(&result);
    assert_eq!(
        fs::read(kindle.extension.join("bin/kindle-battery-monitor")).expect("restored"),
        fs::read(kindle.extension.join("payload/monitor")).expect("payload")
    );
    assert!(!tx.exists());
    assert!(stdout(&result).contains("Recovered"));
    assert_eq!(kindle.state()["running"], false);
}

#[test]
fn test_failed_boot_setting_reload_restores_previous_job() {
    let kindle = FakeKindle::new();
    kindle.installed(true, false);
    let job = kindle.root.join("etc/upstart/kindle-battery-monitor.conf");
    let old = fs::read(&job).expect("old");
    assert!(!kindle
        .run("disable", "reload-configuration-once")
        .status
        .success());
    assert_eq!(fs::read(job).expect("job"), old);
    assert_eq!(kindle.state()["mount"], "ro");
}

#[test]
fn test_corruption_unknown_ownership_and_busy_are_safe_refusals() {
    for scenario in ["corrupt", "foreign-job", "foreign-binary", "busy"] {
        let kindle = FakeKindle::new();
        kindle.installed(true, true);
        let job = kindle.root.join("etc/upstart/kindle-battery-monitor.conf");
        match scenario {
            "corrupt" => write(kindle.extension.join("payload/monitor"), "corrupt"),
            "foreign-job" => write(&job, "foreign startup job"),
            "foreign-binary" => write(
                kindle.extension.join("bin/kindle-battery-monitor"),
                "foreign runtime",
            ),
            _ => {}
        }
        let before = fs::read(&job).expect("job");
        assert!(!kindle
            .run("install", if scenario == "busy" { "busy" } else { "" })
            .status
            .success());
        assert_eq!(fs::read(job).expect("job"), before);
        assert_eq!(kindle.state()["running"], true);
        assert!(!mounts(&kindle));
    }
}

#[test]
fn test_stop_start_swap_and_recovery_failures_never_report_success() {
    for failure in ["stop", "start", "swap", "reload-configuration"] {
        let kindle = FakeKindle::new();
        kindle.installed(true, true);
        let binary = kindle.extension.join("bin/kindle-battery-monitor");
        let old = fs::read(&binary).expect("old");
        let result = kindle.run("install", failure);
        assert!(!result.status.success());
        assert!(!stdout(&result).contains("Installed"));
        assert_eq!(fs::read(binary).expect("binary"), old);
        assert_eq!(kindle.state()["mount"], "ro");
        assert_eq!(kindle.state()["running"], failure == "stop");
        if failure == "start" || failure == "reload-configuration" {
            assert!(stderr(&result).to_lowercase().contains("recovery failed"));
            assert!(kindle.extension.join("state/transaction").exists());
        }
    }
}

#[test]
fn test_writable_root_is_preserved_and_read_only_restore_failure_is_visible() {
    let kindle = FakeKindle::new();
    kindle.stage();
    let mut state = kindle.state();
    state["mount"] = serde_json::json!("rw");
    kindle.save(state);
    write(
        kindle.root.join("proc/mounts"),
        "/dev/root / ext3 rw,noatime 0 0\n",
    );
    success(&kindle.run("install", ""));
    assert_eq!(kindle.state()["mount"], "rw");
    assert!(!mounts(&kindle));
    let failed = FakeKindle::new();
    failed.stage();
    let result = failed.run("install", "ro");
    assert!(!result.status.success());
    assert!(stderr(&result).contains("root mount restoration failed"));
    assert!(!stdout(&result).contains("Installed"));
}

#[test]
fn test_known_manual_installation_is_adopted_without_changing_state() {
    let kindle = FakeKindle::new();
    kindle.installed(true, true);
    fs::remove_dir_all(kindle.extension.join("state")).expect("remove state");
    success(&kindle.run("install", ""));
    assert_eq!(kindle.state()["running"], true);
    assert_eq!(
        fs::read_to_string(kindle.extension.join("state/version"))
            .expect("version")
            .trim(),
        "0.1.0"
    );
}

#[test]
fn test_uninstall_stop_failure_retains_extension_and_hook() {
    let kindle = FakeKindle::new();
    kindle.installed(true, true);
    assert!(!kindle.run("uninstall-confirm", "stop").status.success());
    assert!(kindle.extension.exists());
    assert!(kindle
        .root
        .join("etc/upstart/kindle-battery-monitor.conf")
        .exists());
}

#[test]
fn test_release_for_a_different_target_is_rejected_before_mount_changes() {
    let kindle = FakeKindle::new();
    kindle.stage();
    let manifest = kindle.extension.join("payload/manifest");
    write(
        &manifest,
        fs::read_to_string(&manifest)
            .expect("manifest")
            .replace("device_type=AJRLVDTOPT1LE", "device_type=OTHER"),
    );
    assert!(!kindle.run("install", "").status.success());
    assert!(!mounts(&kindle));
}

#[test]
fn test_payload_copy_failure_recovers_previous_installation() {
    let kindle = FakeKindle::new();
    kindle.installed(true, true);
    let binary = kindle.extension.join("bin/kindle-battery-monitor");
    let old = fs::read(&binary).expect("binary");
    let result = kindle.run("install", "copy");
    assert!(!result.status.success());
    assert_eq!(fs::read(binary).expect("binary"), old);
    assert_eq!(kindle.state()["running"], true);
    assert_eq!(kindle.state()["mount"], "ro");
    assert!(stderr(&result).to_lowercase().contains("recovered"));
}
