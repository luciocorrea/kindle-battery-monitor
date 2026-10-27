//! Architecture invariant for the packaged startup job and installed executable path.

// Architecture invariant: the job's event and executable path are the on-device install contract.
#[test]
fn startup_job_waits_for_userstore_and_has_a_shutdown_inverse() {
    let job = include_str!("../packaging/kindle-battery-monitor.conf");
    assert!(
        job.contains("start on (battery_ready and dbus_ready and started powerd and started btfd)")
    );
    assert!(job.contains(
        "stop on (stopping lab126 or stopping dbus or stopping powerd or stopping btfd)"
    ));
    assert!(job
        .contains("exec /mnt/us/extensions/kindle-battery-monitor/bin/kindle-battery-monitor run"));
    assert!(!job.contains(">> /var/log/kindle-battery-monitor.jsonl"));
    assert!(job
        .lines()
        .all(|line| !line.trim_start().starts_with("respawn")));
    assert!(!job.contains("preventScreenSaver"));
    assert!(
        job.contains("kill timeout 50"),
        "allow the bounded enable/cleanup transaction to finish on stop"
    );
}
