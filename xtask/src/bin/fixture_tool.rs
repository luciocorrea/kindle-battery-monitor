//! Compiled command stand-ins used only by isolated host integration tests.
#![forbid(unsafe_code)]
use serde_json::{json, Value};
use std::{
    env, fs, io,
    path::Path,
    process::{Command, ExitCode},
};
fn save(path: &Path, state: &Value) -> io::Result<()> {
    fs::write(path, serde_json::to_vec(state).map_err(io::Error::other)?)
}
fn kindle(name: &str, args: &[String]) -> io::Result<i32> {
    let root = env::var_os("KINDLE_BATTERY_ROOT")
        .ok_or_else(|| io::Error::other("fixture requires an isolated root"))?;
    let root = Path::new(&root);
    if root == Path::new("/") {
        return Err(io::Error::other("invalid fixture root"));
    }
    let path = root.join("fake-state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path)?).map_err(io::Error::other)?;
    let mut event = vec![name.to_owned()];
    event.extend_from_slice(args);
    state["events"]
        .as_array_mut()
        .ok_or_else(|| io::Error::other("events array"))?
        .push(json!(event));
    let failures = env::var("FAKE_FAIL").unwrap_or_default();
    let fail: Vec<_> = failures.split(',').collect();
    let first = args.first().map(String::as_str).unwrap_or("");
    let mut code = 0;
    match name {
        "initctl" => {
            let failed_key = format!("failed_{first}");
            let once =
                fail.contains(&format!("{first}-once").as_str()) && state[&failed_key] != true;
            if fail.contains(&first) || once {
                state[&failed_key] = json!(true);
                eprintln!("injected {first} failure");
                code = 1;
            } else {
                match first {
                    "status" => {
                        if state["installed"] != true {
                            eprintln!("initctl: Unknown job");
                            code = 1;
                        } else {
                            println!(
                                "kindle-battery-monitor {}",
                                if state["running"] == true {
                                    "start/running, process 4321"
                                } else {
                                    "stop/waiting"
                                }
                            );
                        }
                    }
                    "start" => state["running"] = json!(true),
                    "stop" => {
                        use std::io::Write;
                        state["running"] = json!(false);
                        let event = if fail.contains(&"cleanup") {
                            "monitor_failed"
                        } else {
                            "monitor_stopped"
                        };
                        let stamp = state["events"].as_array().map(Vec::len).unwrap_or(0);
                        let mut log = fs::OpenOptions::new()
                            .append(true)
                            .create(true)
                            .open(root.join("var/log/kindle-battery-monitor.jsonl"))?;
                        writeln!(
                            log,
                            "{}",
                            json!({"timestamp":stamp,"fields":{"event":event}})
                        )?;
                    }
                    "reload-configuration" => {
                        state["installed"] = json!(root
                            .join("etc/upstart/kindle-battery-monitor.conf")
                            .exists())
                    }
                    _ => return Err(io::Error::other("unexpected initctl operation")),
                }
            }
        }
        "mntroot" => {
            if fail.contains(&first) {
                code = 1;
            } else {
                state["mount"] = json!(first);
                fs::write(
                    root.join("proc/mounts"),
                    format!("/dev/root / ext3 {first},noatime 0 0\n"),
                )?;
            }
        }
        "lipc-get-prop" => println!("99"),
        "eips" => {}
        "flock" => {
            if fail.contains(&"busy") {
                code = 1;
            }
        }
        "cp" if fail.contains(&"copy") && first.ends_with("/payload/monitor") => {
            eprintln!("injected copy failure");
            code = 1;
        }
        "mv" if fail.contains(&"swap")
            && args
                .last()
                .is_some_and(|x| x.ends_with("/bin/kindle-battery-monitor")) =>
        {
            eprintln!("injected swap failure");
            code = 1;
        }
        _ => {
            code = Command::new(format!("/bin/{name}"))
                .args(args)
                .status()?
                .code()
                .unwrap_or(1)
        }
    }
    save(&path, &state)?;
    Ok(code)
}
fn tools(name: &str, args: &[String], invocation: &Path) -> io::Result<i32> {
    use std::io::Write;
    let origin = invocation
        .parent()
        .and_then(Path::file_name)
        .and_then(|x| x.to_str())
        .unwrap_or("");
    let stage = if args.iter().any(|x| x == "--version") {
        None
    } else if name == "cargo" {
        match args.first().map(String::as_str) {
            Some("fmt") => Some("format"),
            Some("doc") => Some("rustdoc"),
            Some("test") => Some("rust-tests"),
            Some("clippy") => Some("host-clippy"),
            _ => None,
        }
    } else if name == "cross" && args.first().is_some_and(|x| x == "clippy") {
        Some("arm-clippy")
    } else {
        None
    };
    let log = env::var_os("VERIFY_TEST_LOG").ok_or_else(|| io::Error::other("fixture log"))?;
    let mut file = fs::OpenOptions::new().append(true).create(true).open(log)?;
    writeln!(
        file,
        "{}",
        json!({"name":name,"args":args,"origin":origin,"stage":stage,"cwd":env::current_dir()?,"toolchain":env::var("RUSTUP_TOOLCHAIN").ok(),"wrapper":env::var("RUSTC_WRAPPER").ok(),"rustdocflags":env::var("RUSTDOCFLAGS").ok()})
    )?;
    if origin == "homebrew" {
        eprintln!("conflicting Homebrew tool executed");
        return Ok(1);
    }
    if stage.is_some_and(|stage| env::var("VERIFY_FAIL_STAGE").is_ok_and(|fail| fail == stage)) {
        return Ok(23);
    }
    if name == "rustup" {
        if args.len() == 4 && args[..3] == ["which", "--toolchain", "1.95.0"] {
            let pinned =
                env::var_os("VERIFY_PINNED_BIN").ok_or_else(|| io::Error::other("pinned tools"))?;
            let tool = Path::new(&pinned).join(&args[3]);
            if !tool.is_file() {
                eprintln!("missing pinned component: {}", args[3]);
                return Ok(1);
            }
            println!("{}", tool.display());
            return Ok(0);
        }
        return Err(io::Error::other("unexpected Rustup request"));
    }
    if name == "cargo" && args.first().is_some_and(|x| x == "fmt" || x == "clippy") {
        let program = if args[0] == "fmt" {
            "rustfmt"
        } else {
            "cargo-clippy"
        };
        return Ok(Command::new(program)
            .args(&args[1..])
            .status()?
            .code()
            .unwrap_or(1));
    }
    if name == "cross" && args.first().is_some_and(|x| x == "build") {
        return Ok(env::var("FAKE_CROSS_STATUS")
            .ok()
            .and_then(|x| x.parse().ok())
            .unwrap_or(0));
    }
    if name == "docker" {
        return Ok(env::var("VERIFY_DOCKER_STATUS")
            .ok()
            .and_then(|x| x.parse().ok())
            .unwrap_or(0));
    }
    if args.iter().any(|x| x == "--version") {
        println!(
            "{}",
            match name {
                "rustc" => "rustc 1.95.0 (pinned)",
                "cargo" => "cargo 1.95.0 (pinned)",
                "rustfmt" => "rustfmt 1.9.0-stable (pinned)",
                "cargo-clippy" => "clippy 0.1.95 (pinned)",
                "cross" => "cross 0.2.5",
                _ => name,
            }
        );
    }
    Ok(0)
}
fn run() -> io::Result<i32> {
    let arguments: Vec<_> = env::args().collect();
    let invocation = Path::new(arguments.first().map(String::as_str).unwrap_or(""));
    let name = invocation
        .file_name()
        .and_then(|x| x.to_str())
        .unwrap_or("");
    if env::var_os("VERIFY_TEST_LOG").is_some() {
        tools(name, &arguments[1..], invocation)
    } else {
        kindle(name, &arguments[1..])
    }
}
fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(error) => {
            eprintln!("fixture error: {error}");
            ExitCode::FAILURE
        }
    }
}
