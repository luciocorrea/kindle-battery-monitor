//! Repository-local host verification, build and release automation.
#![forbid(unsafe_code)]
mod links;
mod package;
mod tools;

use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitCode,
};
use thiserror::Error;

#[derive(Debug, Error)]
enum ToolError {
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Regex(#[from] regex::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
    #[error("{program} exited with status {code}: {detail}")]
    Command {
        program: String,
        code: i32,
        detail: String,
    },
    #[error("Verification failed: {label}: {source}")]
    Stage {
        label: &'static str,
        #[source]
        source: Box<ToolError>,
    },
    #[error("{0}")]
    Invalid(String),
    #[error("{0}\nUsage: cargo xtask verify [--arm] | build | package [--binary PATH] [--output PATH] | stage --binary PATH --output PATH | check-links [--root PATH]")]
    Usage(String),
}
impl ToolError {
    fn code(&self) -> u8 {
        match self {
            Self::Usage(_) => 2,
            Self::Command { code, .. } => u8::try_from(*code).unwrap_or(1),
            Self::Stage { source, .. } => source.code(),
            _ => 1,
        }
    }
}
type Result<T> = std::result::Result<T, ToolError>;
fn io(context: impl Into<String>, source: std::io::Error) -> ToolError {
    ToolError::Io {
        context: context.into(),
        source,
    }
}
fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| io(format!("read {}", path.display()), source))
}
fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|source| io(format!("read {}", path.display()), source))
}
struct Args {
    command: String,
    root: PathBuf,
    binary: Option<PathBuf>,
    output: Option<PathBuf>,
    arm: bool,
}
fn parse(args: Vec<OsString>) -> Result<Option<Args>> {
    if args.is_empty() || args.iter().any(|x| x == "--help") {
        println!("Usage: cargo xtask verify [--arm] | build | package [--binary PATH] [--output PATH] | stage --binary PATH --output PATH | check-links [--root PATH]");
        return Ok(None);
    }
    let command = args[0].to_string_lossy().into_owned();
    if !["check-links", "stage", "package", "build", "verify"].contains(&command.as_str()) {
        return Err(ToolError::Usage("unknown task".into()));
    }
    let mut parsed = Args {
        command,
        root: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(PathBuf::from)
            .unwrap_or_default(),
        binary: None,
        output: None,
        arm: false,
    };
    let mut index = 1;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        if flag == "--arm" && parsed.command == "verify" && !parsed.arm {
            parsed.arm = true;
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| ToolError::Usage(format!("missing value for {flag}")))?;
        match flag.as_ref() {
            "--root" => parsed.root = PathBuf::from(value),
            "--binary" if parsed.command == "package" || parsed.command == "stage" => {
                parsed.binary = Some(value.into())
            }
            "--output" if parsed.command == "package" || parsed.command == "stage" => {
                parsed.output = Some(value.into())
            }
            _ => return Err(ToolError::Usage(format!("unknown argument {flag}"))),
        }
        index += 2;
    }
    Ok(Some(parsed))
}
fn run(args: Args) -> Result<()> {
    match args.command.as_str() {
        "check-links" => {
            let (files, errors) = links::check(&args.root)?;
            if !errors.is_empty() {
                return Err(ToolError::Invalid(errors.join("\n")));
            }
            println!("Markdown links passed ({files} maintained documents).");
            Ok(())
        }
        "verify" => tools::Toolchain::load(&args.root, true)?.verify(args.arm),
        "build" => tools::Toolchain::load(&args.root, false)?.build(),
        "package" | "stage" => {
            let binary = match args.binary {
                Some(binary) => binary,
                None if args.command == "package" => {
                    tools::Toolchain::load(&args.root, false)?.build()?;
                    let target = env::var_os("CARGO_TARGET_DIR")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("target"));
                    let target = if target.is_absolute() {
                        target
                    } else {
                        args.root.join(target)
                    };
                    target
                        .join("arm-unknown-linux-musleabihf/release")
                        .join(package::NAME)
                }
                None => return Err(ToolError::Usage("--binary is required".into())),
            };
            package::verify_binary(&binary)?;
            let output = if args.command == "stage" {
                args.output
                    .ok_or_else(|| ToolError::Usage("--output is required".into()))?
            } else {
                args.output.unwrap_or_else(|| args.root.join("dist"))
            };
            if args.command == "stage" {
                package::stage(&args.root, &binary, &output)?;
                println!("{}", output.display());
                Ok(())
            } else {
                package::release(&args.root, &binary, &output)
            }
        }
        _ => Err(ToolError::Usage("unknown task".into())),
    }
}
fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let packaging = args.first().is_some_and(|x| x == "package" || x == "stage");
    let result = parse(args).and_then(|args| args.map(run).unwrap_or(Ok(())));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(ToolError::Usage(message)) => {
            eprintln!("{}", ToolError::Usage(message));
            ExitCode::from(2)
        }
        Err(error) => {
            let code = error.code();
            if packaging {
                eprintln!("Packaging failed: {error}");
            } else {
                eprintln!("{error}");
            }
            ExitCode::from(code)
        }
    }
}
