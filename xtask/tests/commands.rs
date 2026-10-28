//! Host automation behavior through named compiled external-tool stand-ins.
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::{Command, Output},
};
fn project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace")
        .to_owned()
}
struct FakeVerificationTools {
    _temp: tempfile::TempDir,
    root: PathBuf,
    bootstrap: PathBuf,
    pinned: PathBuf,
    homebrew: PathBuf,
    log: PathBuf,
}
impl FakeVerificationTools {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .prefix("kindle verify ")
            .tempdir()
            .expect("fixture");
        let root = temp.path().join("source copy");
        fs::create_dir_all(&root).expect("root");
        for name in ["rust-toolchain.toml", "Cross.toml"] {
            fs::copy(project().join(name), root.join(name)).expect("configuration");
        }
        fs::write(root.join("README.md"), "# Project\n").expect("valid docs");
        let bootstrap = temp.path().join("bootstrap");
        let pinned = temp.path().join("pinned");
        let homebrew = temp.path().join("homebrew");
        let log = temp.path().join("calls.jsonl");
        let fixture = Self {
            _temp: temp,
            root,
            bootstrap,
            pinned,
            homebrew,
            log,
        };
        fixture.install(&fixture.bootstrap, "rustup");
        for name in ["rustc", "cargo", "rustfmt", "cargo-clippy"] {
            fixture.install(&fixture.pinned, name);
            fixture.install(&fixture.homebrew, name);
        }
        fixture
    }
    fn install(&self, dir: &Path, name: &str) {
        fs::create_dir_all(dir).expect("tools");
        symlink(env!("CARGO_BIN_EXE_fixture_tool"), dir.join(name)).expect("compiled fixture");
    }
    fn command(&self, task: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_xtask"));
        cmd.arg(task)
            .arg("--root")
            .arg(&self.root)
            .current_dir(self._temp.path())
            .env(
                "PATH",
                std::env::join_paths([&self.homebrew, &self.bootstrap]).expect("PATH"),
            )
            .env("VERIFY_TEST_LOG", &self.log)
            .env("VERIFY_PINNED_BIN", &self.pinned)
            .env("RUSTC_WRAPPER", "conflicting-wrapper")
            .env("RUSTDOCFLAGS", "--cfg verification");
        cmd
    }
    fn run(&self, task: &str, args: &[&str]) -> Output {
        if self.log.exists() {
            fs::remove_file(&self.log).expect("reset log");
        }
        self.command(task)
            .args(args)
            .output()
            .expect("task process")
    }
    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("call JSON"))
            .collect()
    }
}
#[test]
fn test_build_selects_the_locked_arm_release_and_propagates_failure() {
    let tools = FakeVerificationTools::new();
    tools.install(&tools.bootstrap, "cross");
    let output = tools.run("build", &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = tools.calls();
    let cross = calls
        .iter()
        .find(|call| call["name"] == "cross")
        .expect("cross build");
    assert_eq!(
        cross["args"],
        serde_json::json!([
            "build",
            "--package",
            "kindle-battery-monitor",
            "--locked",
            "--release",
            "--target",
            "arm-unknown-linux-musleabihf",
            "--config",
            "build.rustc-wrapper=\"\""
        ])
    );
    assert_eq!(
        Path::new(cross["cwd"].as_str().expect("cwd"))
            .canonicalize()
            .expect("cwd"),
        tools.root.canonicalize().expect("root")
    );
    assert_eq!(cross["toolchain"], "1.95.0");
    assert_eq!(cross["wrapper"], "");
    let failed = tools
        .command("build")
        .env("FAKE_CROSS_STATUS", "23")
        .output()
        .expect("failure");
    assert_eq!(failed.status.code(), Some(23));
}
fn stages(calls: &[Value]) -> Vec<&str> {
    calls
        .iter()
        .filter_map(|call| call["stage"].as_str())
        .collect()
}
#[test]
fn test_host_checks_use_pinned_tools_and_run_all_native_workspace_suites() {
    let tools = FakeVerificationTools::new();
    let output = tools.run("verify", &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = tools.calls();
    assert_eq!(
        stages(&calls),
        ["format", "rustdoc", "host-clippy", "rust-tests"]
    );
    assert!(!calls.iter().any(|call| call["origin"] == "homebrew"));
    let cargo: Vec<_> = calls
        .iter()
        .filter(|call| call["name"] == "cargo" && call["stage"].is_string())
        .collect();
    assert_eq!(
        cargo.iter().map(|x| x["args"].clone()).collect::<Vec<_>>(),
        vec![
            serde_json::json!(["fmt", "--all", "--check"]),
            serde_json::json!(["doc", "--workspace", "--no-deps", "--locked"]),
            serde_json::json!([
                "clippy",
                "--workspace",
                "--all-targets",
                "--locked",
                "--",
                "-D",
                "warnings"
            ]),
            serde_json::json!(["test", "--workspace", "--locked"])
        ]
    );
    for call in cargo {
        assert_eq!(call["toolchain"], "1.95.0");
        assert_eq!(call["wrapper"], "");
        assert_eq!(
            Path::new(call["cwd"].as_str().expect("cwd"))
                .canonicalize()
                .expect("cwd"),
            tools.root.canonicalize().expect("root")
        );
        if call["stage"] == "rustdoc" {
            let flags = call["rustdocflags"].as_str().expect("flags");
            assert!(flags.contains("--cfg verification"));
            assert!(flags.contains("-D warnings"));
        }
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for identity in [
        "rustc 1.95.0",
        "cargo 1.95.0",
        "rustfmt 1.9.0",
        "clippy 0.1.95",
        "ARM: skipped",
        "Verification passed",
    ] {
        assert!(stdout.contains(identity), "{stdout}");
    }
}
#[test]
fn test_requested_arm_runs_after_host_with_locked_target_and_clean_wrapper() {
    let tools = FakeVerificationTools::new();
    tools.install(&tools.bootstrap, "cross");
    tools.install(&tools.bootstrap, "docker");
    let output = tools.run("verify", &["--arm"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = tools.calls();
    assert_eq!(
        stages(&calls),
        [
            "format",
            "rustdoc",
            "host-clippy",
            "rust-tests",
            "arm-clippy"
        ]
    );
    let arm = calls
        .iter()
        .find(|call| call["stage"] == "arm-clippy")
        .expect("ARM");
    assert_eq!(
        arm["args"],
        serde_json::json!([
            "clippy",
            "--workspace",
            "--exclude",
            "xtask",
            "--all-targets",
            "--target",
            "arm-unknown-linux-musleabihf",
            "--locked",
            "--config",
            "build.rustc-wrapper=\"\"",
            "--",
            "-D",
            "warnings"
        ])
    );
    assert_eq!(arm["toolchain"], "1.95.0");
    assert_eq!(arm["wrapper"], "");
    assert!(!calls.iter().any(|call| call["origin"] == "homebrew"));
    let docker = calls
        .iter()
        .position(|call| call["name"] == "docker")
        .expect("Docker access");
    let format = calls
        .iter()
        .position(|call| call["stage"] == "format")
        .expect("format");
    assert!(docker < format);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ARM: passed"));
    assert!(!stdout.contains("ARM: skipped"));
}
#[test]
fn test_each_stage_failure_stops_later_stages_and_preserves_status() {
    let tools = FakeVerificationTools::new();
    tools.install(&tools.bootstrap, "cross");
    tools.install(&tools.bootstrap, "docker");
    let order = [
        "format",
        "rustdoc",
        "host-clippy",
        "rust-tests",
        "arm-clippy",
    ];
    let labels = [
        "Formatting",
        "Rustdoc",
        "Host Clippy",
        "Rust tests and doctests",
        "ARM Clippy",
    ];
    for (index, stage) in order.iter().enumerate() {
        if tools.log.exists() {
            fs::remove_file(&tools.log).expect("reset calls");
        }
        let output = tools
            .command("verify")
            .arg("--arm")
            .env("VERIFY_FAIL_STAGE", stage)
            .output()
            .expect("failed task");
        assert_eq!(output.status.code(), Some(23));
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains(&format!("Verification failed: {}", labels[index])));
        assert_eq!(stages(&tools.calls()), order[..=index]);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("Verification passed"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("ARM: passed"));
    }
    fs::write(tools.root.join("README.md"), "[bad](absent.md)\n").expect("broken docs");
    let output = tools.run("verify", &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Verification failed: Markdown links"));
    assert_eq!(stages(&tools.calls()), ["format"]);
}
#[test]
fn test_verification_runs_without_python() {
    let tools = FakeVerificationTools::new();
    let output = tools
        .command("verify")
        .env("PYTHON", "/missing/interpreter")
        .output()
        .expect("native verification");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!tools
        .calls()
        .iter()
        .any(|call| call["name"].as_str().is_some_and(|x| x.contains("python"))));
}
#[test]
fn test_missing_pinned_component_fails_before_verification_stages() {
    let tools = FakeVerificationTools::new();
    fs::remove_file(tools.pinned.join("cargo-clippy")).expect("missing component");
    let output = tools.run("verify", &[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Verification failed: Clippy"));
    assert!(stages(&tools.calls()).is_empty());
    assert!(!tools
        .calls()
        .iter()
        .any(|call| call["origin"] == "homebrew"));
}
#[test]
fn test_arm_prerequisites_fail_before_host_checks() {
    for missing in ["cross", "docker", "docker-access"] {
        let tools = FakeVerificationTools::new();
        if missing != "cross" {
            tools.install(&tools.bootstrap, "cross");
        }
        if missing != "docker" {
            tools.install(&tools.bootstrap, "docker");
        }
        let output = tools
            .command("verify")
            .arg("--arm")
            .env(
                "VERIFY_DOCKER_STATUS",
                if missing == "docker-access" {
                    "31"
                } else {
                    "0"
                },
            )
            .output()
            .expect("prerequisites");
        assert!(!output.status.success());
        let expected = match missing {
            "cross" => "requires cross",
            "docker" => "requires Docker",
            _ => "ARM Docker access",
        };
        assert!(String::from_utf8_lossy(&output.stderr).contains(expected));
        assert!(stages(&tools.calls()).is_empty());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("Verification passed"));
        if missing == "docker-access" {
            assert_eq!(output.status.code(), Some(31));
        }
    }
}
#[test]
fn test_help_and_invalid_arguments_do_not_run_tools() {
    let tools = FakeVerificationTools::new();
    for (args, status) in [
        (vec!["--help"], 0),
        (vec!["--unknown"], 2),
        (vec!["--arm", "extra"], 2),
    ] {
        let output = tools.run("verify", &args);
        assert_eq!(output.status.code(), Some(status));
        assert!(format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .contains("Usage:"));
        assert!(!tools.log.exists());
    }
}
#[test]
fn package_without_binary_builds_the_selected_target_before_staging() {
    let tools = FakeVerificationTools::new();
    tools.install(&tools.bootstrap, "cross");
    fs::write(
        tools.root.join("Cargo.toml"),
        "[package]\nversion = \"0.1.1\"\n",
    )
    .expect("version");
    for name in [
        "LICENSE",
        "packaging/kindle-battery-monitor.conf",
        "packaging/kual/config.xml",
        "packaging/kual/menu.json",
        "packaging/kual/bin/control.sh",
    ] {
        let to = tools.root.join(name);
        fs::create_dir_all(to.parent().expect("parent")).expect("assets");
        fs::copy(project().join(name), to).expect("asset");
    }
    let binary = tools
        .root
        .join("custom target/arm-unknown-linux-musleabihf/release/kindle-battery-monitor");
    fs::create_dir_all(binary.parent().expect("parent")).expect("target");
    let mut elf = vec![0; 64];
    elf[..7].copy_from_slice(b"\x7fELF\x01\x01\x01");
    elf[16..20].copy_from_slice(b"\x02\x00\x28\x00");
    fs::write(binary, elf).expect("built ELF");
    let output_dir = tools.root.join("output");
    let output = tools
        .command("package")
        .arg("--output")
        .arg(&output_dir)
        .env("CARGO_TARGET_DIR", "custom target")
        .output()
        .expect("build then package");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(tools
        .calls()
        .iter()
        .any(|call| call["name"] == "cross" && call["args"][0] == "build"));
    assert!(output_dir.join("kindle-battery-monitor-0.1.1.zip").exists());
    let failed = tools
        .command("package")
        .arg("--output")
        .arg(tools.root.join("failure-output"))
        .env("FAKE_CROSS_STATUS", "23")
        .output()
        .expect("cross failure");
    assert_eq!(failed.status.code(), Some(23));
    assert!(!tools.root.join("failure-output").exists());
}
