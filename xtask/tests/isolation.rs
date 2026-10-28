//! Architecture invariants for source-local packages and host/device separation.
use std::{fs, path::Path, process::Command};
fn copy_source(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("source directory");
    for entry in fs::read_dir(source).expect("source entries") {
        let entry = entry.expect("entry");
        let name = entry.file_name();
        if [
            "target",
            "dist",
            ".git",
            ".scratch",
            ".agents",
            ".codex",
            ".aws",
            ".cache",
            "__pycache__",
            ".DS_Store",
        ]
        .iter()
        .any(|x| name == *x)
        {
            continue;
        }
        let kind = entry.file_type().expect("entry type");
        let path = destination.join(name);
        if kind.is_dir() {
            copy_source(&entry.path(), &path);
        } else if kind.is_file() {
            fs::copy(entry.path(), path).expect("source file");
        }
    }
}
#[test]
fn test_source_only_copy_resolves_all_local_packages_inside_itself() {
    let temp = tempfile::Builder::new()
        .prefix("kindle source copy ")
        .tempdir()
        .expect("temp");
    let copy = temp.path().join("source copy");
    copy_source(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("project"),
        &copy,
    );
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| env!("CARGO").into());
    let output = Command::new(&cargo)
        .args([
            "metadata",
            "--locked",
            "--offline",
            "--no-deps",
            "--format-version",
            "1",
        ])
        .current_dir(&copy)
        .output()
        .expect("source metadata");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).expect("metadata");
    assert_eq!(
        Path::new(metadata["workspace_root"].as_str().expect("root"))
            .canonicalize()
            .expect("root"),
        copy.canonicalize().expect("copy")
    );
    let packages = metadata["packages"].as_array().expect("packages");
    let names: std::collections::BTreeSet<_> = packages
        .iter()
        .map(|p| p["name"].as_str().expect("package"))
        .collect();
    assert_eq!(
        names,
        ["kindle-battery-monitor", "kindle-lipc-dbus", "xtask"]
            .into_iter()
            .collect()
    );
    for package in packages {
        assert!(
            Path::new(package["manifest_path"].as_str().expect("manifest"))
                .canonicalize()
                .expect("path")
                .starts_with(copy.canonicalize().expect("copy"))
        );
    }
    assert_eq!(
        metadata["workspace_default_members"]
            .as_array()
            .expect("default members")
            .len(),
        1
    );
    assert!(metadata["workspace_default_members"][0]
        .as_str()
        .expect("default member")
        .contains("#kindle-battery-monitor@"));
    let output = Command::new(cargo)
        .args([
            "tree",
            "--package",
            "kindle-battery-monitor",
            "--locked",
            "--offline",
            "--edges",
            "normal",
            "--prefix",
            "none",
        ])
        .current_dir(&copy)
        .output()
        .expect("runtime dependency graph");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = String::from_utf8_lossy(&output.stdout);
    for host in [
        "xtask ",
        "zip ",
        "sha2 ",
        "toml ",
        "flate2 ",
        "html-escape ",
    ] {
        assert!(
            !tree.lines().any(|line| line.starts_with(host)),
            "host dependency entered Kindle graph: {host}"
        );
    }
}
