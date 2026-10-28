//! Static ARM ELF validation and KUAL distribution assembly.
use crate::{read_bytes, Result, ToolError};
use std::path::Path;
pub(crate) fn verify_binary(path: &Path) -> Result<()> {
    let data = read_bytes(path)?;
    if data.len() < 52 || &data[..7] != b"\x7fELF\x01\x01\x01" || data[16..20] != [2, 0, 40, 0] {
        return Err(ToolError::Invalid(
            "release must be a 32-bit little-endian ARM executable".into(),
        ));
    }
    let u16_at = |n| u16::from_le_bytes([data[n], data[n + 1]]);
    let offset = u32::from_le_bytes([data[28], data[29], data[30], data[31]]) as usize;
    let size = u16_at(42) as usize;
    let count = u16_at(44) as usize;
    for index in 0..count {
        let start = index
            .checked_mul(size)
            .and_then(|x| offset.checked_add(x))
            .ok_or_else(|| ToolError::Invalid("invalid ELF program headers".into()))?;
        if size < 32 || start.checked_add(size).is_none_or(|x| x > data.len()) {
            return Err(ToolError::Invalid("invalid ELF program headers".into()));
        }
        let segment = u32::from_le_bytes([
            data[start],
            data[start + 1],
            data[start + 2],
            data[start + 3],
        ]);
        if segment == 2 || segment == 3 {
            return Err(ToolError::Invalid(
                "release must have no dynamic segment or interpreter".into(),
            ));
        }
    }
    Ok(())
}

use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::PermissionsExt};
pub(crate) const NAME: &str = "kindle-battery-monitor";
const MODEL: &str = "AJRLVDTOPT1LE";
const FIRMWARE: &str = "System Software Version: 042-juno_1605_moonshine_rex-414285";
const LEGACY_BINARY: &str = "e116ffce39c396bb8d101d3714a4b40b1c436459cb5e4f8f422fa9a6a670a4e8";
const LEGACY_JOB: &str = "dcde5f72b4e2e13325e03993b5637e090b7509158bb536edf4fb7915f6d33641";
pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn mkdir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| crate::io(format!("create {}", path.display()), e))
}
fn write(path: &Path, data: impl AsRef<[u8]>) -> Result<()> {
    fs::write(path, data).map_err(|e| crate::io(format!("write {}", path.display()), e))
}
fn executable(path: &Path) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|e| crate::io(format!("chmod {}", path.display()), e))
}
fn copy(source: &Path, destination: &Path) -> Result<()> {
    write(destination, read_bytes(source)?)
}
pub(crate) fn stage(root: &Path, binary: &Path, output: &Path) -> Result<String> {
    verify_binary(binary)?;
    if output.exists() {
        return Err(ToolError::Invalid(
            "staging output must be a new directory".into(),
        ));
    }
    let manifest: toml::Value = toml::from_str(&crate::read(&root.join("Cargo.toml"))?)?;
    let version = manifest
        .get("package")
        .and_then(|x| x.get("version"))
        .and_then(|x| x.as_str())
        .ok_or_else(|| ToolError::Invalid("missing package.version".into()))?;
    mkdir(&output.join("bin"))?;
    mkdir(&output.join("payload"))?;
    let packaging = root.join("packaging");
    for name in ["config.xml", "menu.json"] {
        copy(&packaging.join("kual").join(name), &output.join(name))?;
    }
    copy(
        &packaging.join("kual/bin/control.sh"),
        &output.join("bin/control.sh"),
    )?;
    executable(&output.join("bin/control.sh"))?;
    copy(binary, &output.join("payload/monitor"))?;
    executable(&output.join("payload/monitor"))?;
    let automatic = crate::read(&packaging.join("kindle-battery-monitor.conf"))?;
    write(&output.join("payload/automatic.conf"), &automatic)?;
    let on_demand: String = automatic
        .split_inclusive('\n')
        .filter(|line| !line.starts_with("start on "))
        .collect();
    write(&output.join("payload/on-demand.conf"), on_demand)?;
    let mut fields = vec![
        ("format", "1".to_owned()),
        ("version", version.to_owned()),
        ("device_type", MODEL.into()),
        ("firmware", FIRMWARE.into()),
    ];
    for (field, path) in [
        ("binary_sha256", "payload/monitor"),
        ("automatic_sha256", "payload/automatic.conf"),
        ("on_demand_sha256", "payload/on-demand.conf"),
        ("controller_sha256", "bin/control.sh"),
        ("menu_sha256", "menu.json"),
        ("config_sha256", "config.xml"),
    ] {
        fields.push((field, digest(&read_bytes(&output.join(path))?)));
    }
    fields.extend([
        ("legacy_binary_sha256", LEGACY_BINARY.into()),
        ("legacy_job_sha256", LEGACY_JOB.into()),
    ]);
    write(
        &output.join("payload/manifest"),
        fields
            .into_iter()
            .map(|(key, value)| format!("{key}={value}\n"))
            .collect::<String>(),
    )?;
    write(
        &output.join("README.txt"),
        include_str!("templates/README.txt").replace("{version}", version),
    )?;
    Ok(version.to_owned())
}

fn files(dir: &Path, result: &mut Vec<std::path::PathBuf>) -> Result<()> {
    for entry in
        fs::read_dir(dir).map_err(|e| crate::io(format!("read directory {}", dir.display()), e))?
    {
        let entry = entry.map_err(|e| crate::io("read staged entry", e))?;
        let path = entry.path();
        if path.is_file() {
            result.push(path);
        } else if path.is_dir() {
            files(&path, result)?;
        }
    }
    Ok(())
}
pub(crate) fn release(root: &Path, binary: &Path, output: &Path) -> Result<()> {
    use std::io::Write;
    verify_binary(binary)?;
    mkdir(output)?;
    let temporary = tempfile::Builder::new()
        .prefix(".kindle-release-")
        .tempdir_in(output)
        .map_err(|e| crate::io("create release staging directory", e))?;
    let staged = temporary.path().join(NAME);
    let version = stage(root, binary, &staged)?;
    println!("{}", staged.display());
    copy(&root.join("LICENSE"), &staged.join("LICENSE"))?;
    if version.is_empty()
        || !version
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return Err(ToolError::Invalid("invalid release version".into()));
    }
    let name = format!("{NAME}-{version}.zip");
    let archive = temporary.path().join(&name);
    let file = fs::File::create(&archive).map_err(|e| crate::io("create release archive", e))?;
    let mut zip = zip::ZipWriter::new(file);
    let mut entries = Vec::new();
    files(&staged, &mut entries)?;
    entries.sort();
    for path in entries {
        let name = path
            .strip_prefix(temporary.path())
            .map_err(|_| ToolError::Invalid("staged entry is outside the release tree".into()))?
            .to_str()
            .ok_or_else(|| ToolError::Invalid("non-UTF-8 archive name".into()))?;
        let mode = fs::metadata(&path)
            .map_err(|e| crate::io("read staged permissions", e))?
            .permissions()
            .mode()
            & 0o777;
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(mode)
            .last_modified_time(zip::DateTime::default());
        zip.start_file(name, options)?;
        zip.write_all(&read_bytes(&path)?)
            .map_err(|e| crate::io("write archive entry", e))?;
    }
    zip.finish()?;
    let checksum = temporary.path().join(format!("{name}.sha256"));
    write(
        &checksum,
        format!("{}  {name}\n", digest(&read_bytes(&archive)?)),
    )?;
    fs::rename(&archive, output.join(&name)).map_err(|e| crate::io("publish archive", e))?;
    fs::rename(&checksum, output.join(format!("{name}.sha256")))
        .map_err(|e| crate::io("publish checksum", e))?;
    println!("{}", output.join(&name).display());
    println!("{}", output.join(format!("{name}.sha256")).display());
    Ok(())
}
