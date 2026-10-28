//! Pinned subprocess selection and ARM build commands for host automation.
use crate::{io, read, Result, ToolError};
use std::os::unix::process::ExitStatusExt;
use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};
pub(crate) struct Toolchain {
    pub(crate) version: String,
    pub(crate) cargo: PathBuf,
    pub(crate) rustc: PathBuf,
    root: PathBuf,
    path: OsString,
}
pub(crate) fn stage<T>(label: &'static str, action: impl FnOnce() -> Result<T>) -> Result<T> {
    println!("\n==> {label}");
    action().map_err(|source| ToolError::Stage {
        label,
        source: Box::new(source),
    })
}
fn code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
}
pub(crate) fn run(mut command: Command) -> Result<()> {
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command
        .status()
        .map_err(|e| io(format!("run {program}"), e))?;
    if status.success() {
        Ok(())
    } else {
        Err(ToolError::Command {
            program,
            code: code(status),
            detail: String::new(),
        })
    }
}
impl Toolchain {
    pub(crate) fn load(root: &Path, checks: bool) -> Result<Self> {
        let configuration: toml::Value = toml::from_str(&read(&root.join("rust-toolchain.toml"))?)?;
        let version = configuration
            .get("toolchain")
            .and_then(|x| x.get("channel"))
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ToolError::Invalid("Missing pinned Rust toolchain".into()))?
            .to_owned();
        let resolve = |name: &str| -> Result<PathBuf> {
            let output = Command::new("rustup")
                .args(["which", "--toolchain", &version, name])
                .current_dir(root)
                .output()
                .map_err(|e| io("run rustup", e))?;
            if !output.status.success() {
                return Err(ToolError::Command {
                    program: "rustup".into(),
                    code: code(output.status),
                    detail: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                });
            }
            let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
            if !path.is_file() {
                return Err(ToolError::Invalid(format!(
                    "Missing pinned component: {name}"
                )));
            }
            Ok(path)
        };
        let rustc = stage("Pinned Rust compiler", || resolve("rustc"))?;
        let cargo = stage("Pinned Cargo", || resolve("cargo"))?;
        if checks {
            stage("Rustfmt component", || resolve("rustfmt"))?;
            stage("Clippy component", || resolve("cargo-clippy"))?;
        }
        let directory = rustc
            .parent()
            .ok_or_else(|| ToolError::Invalid("invalid pinned compiler path".into()))?;
        let path = env::join_paths(
            std::iter::once(directory.to_owned())
                .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
        )
        .map_err(|e| ToolError::Invalid(format!("invalid tool PATH: {e}")))?;
        Ok(Self {
            version,
            cargo,
            rustc,
            root: root.to_owned(),
            path,
        })
    }
    pub(crate) fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.root)
            .env("PATH", &self.path)
            .env("RUSTUP_TOOLCHAIN", &self.version)
            .env("RUSTC_WRAPPER", "")
            .env_remove("RUSTC");
        command
    }
    pub(crate) fn build(&self) -> Result<()> {
        let mut command = self.command("cross");
        command.args([
            "build",
            "--package",
            "kindle-battery-monitor",
            "--locked",
            "--release",
            "--target",
            "arm-unknown-linux-musleabihf",
            "--config",
            "build.rustc-wrapper=\"\"",
        ]);
        run(command)
    }
}

impl Toolchain {
    fn cargo(&self, args: &[&str]) -> Command {
        let mut command = self.command(&self.cargo);
        command.args(args);
        command
    }
    pub(crate) fn verify(&self, arm: bool) -> Result<()> {
        stage("Rust compiler", || {
            let mut command = self.command(&self.rustc);
            command.arg("--version");
            run(command)
        })?;
        stage("Cargo", || run(self.cargo(&["--version"])))?;
        stage("Rustfmt", || run(self.cargo(&["fmt", "--version"])))?;
        stage("Clippy", || run(self.cargo(&["clippy", "--version"])))?;
        if arm {
            for (program, description) in [("cross", "cross"), ("docker", "Docker")] {
                if !env::split_paths(&self.path).any(|dir| dir.join(program).is_file()) {
                    return Err(ToolError::Invalid(format!(
                        "ARM verification requires {description} on PATH."
                    )));
                }
            }
            stage("ARM cross", || {
                let mut command = self.command("cross");
                command.arg("--version");
                run(command)
            })?;
            stage("ARM Docker access", || {
                let mut command = self.command("docker");
                command.arg("info");
                let output = command.output().map_err(|e| io("run docker info", e))?;
                if output.status.success() {
                    Ok(())
                } else {
                    Err(ToolError::Command {
                        program: "docker".into(),
                        code: code(output.status),
                        detail: String::from_utf8_lossy(&output.stderr).into_owned(),
                    })
                }
            })?;
        }
        stage("Formatting", || {
            run(self.cargo(&["fmt", "--all", "--check"]))
        })?;
        stage("Markdown links", || {
            let (files, errors) = crate::links::check(&self.root)?;
            if !errors.is_empty() {
                return Err(ToolError::Invalid(errors.join("\n")));
            }
            println!("Markdown links passed ({files} maintained documents).");
            Ok(())
        })?;
        stage("Rustdoc", || {
            let mut command = self.cargo(&["doc", "--workspace", "--no-deps", "--locked"]);
            command.env(
                "RUSTDOCFLAGS",
                format!(
                    "{} -D warnings -D missing_docs",
                    env::var("RUSTDOCFLAGS").unwrap_or_default()
                ),
            );
            run(command)
        })?;
        stage("Host Clippy", || {
            run(self.cargo(&[
                "clippy",
                "--workspace",
                "--all-targets",
                "--locked",
                "--",
                "-D",
                "warnings",
            ]))
        })?;
        stage("Rust tests and doctests", || {
            run(self.cargo(&["test", "--workspace", "--locked"]))
        })?;
        if arm {
            stage("ARM Clippy", || {
                let mut command = self.command("cross");
                command.args([
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
                    "warnings",
                ]);
                run(command)
            })?;
            println!("\nARM: passed\nVerification passed (host and ARM)");
        } else {
            println!("\nARM: skipped (request with --arm)\nVerification passed (host)");
        }
        Ok(())
    }
}
