//! CLI help behavior without device access.

use std::process::Command;

#[test]
fn help_lists_the_bounded_smoke_mode_without_accessing_hardware() {
    let output = Command::new(env!("CARGO_BIN_EXE_kindle-battery-monitor"))
        .arg("--help")
        .output()
        .expect("CLI starts");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help is UTF-8");
    assert!(stdout.contains("--once"));
    assert!(stdout.contains("run"));
}
