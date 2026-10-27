//! One-shot system-bus signal probe command.
//!
//! Prints message metadata as JSON Lines and saves the report to the configured
//! Kindle user-storage path. The observation timeout excludes bus connection and
//! subscription setup. This command does not decode signal body values.

use std::{env, fs, process::ExitCode, time::Duration};

use kindle_lipc_dbus::{run_signal_smoke_probe, SignalSmokeExitCode};

const DEFAULT_SERVICE: &str = "com.lab126.powerd";
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const REPORT_PATH: &str = "/mnt/us/kindle-lipc-dbus-signal-smoke.jsonl";

fn main() -> ExitCode {
    let args = match SignalSmokeArgs::parse(env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let report = match async_io::block_on(run_signal_smoke_probe(
        &args.service,
        args.event.as_deref(),
        Duration::from_secs(args.timeout_seconds),
    )) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    let jsonl = report.to_jsonl();
    print!("{jsonl}");

    if let Err(error) = fs::write(REPORT_PATH, jsonl) {
        eprintln!("failed to write {REPORT_PATH}: {error}");
        return ExitCode::FAILURE;
    }

    match report.exit_code() {
        SignalSmokeExitCode::Pass => ExitCode::SUCCESS,
        SignalSmokeExitCode::TimedOut => ExitCode::from(2),
    }
}

struct SignalSmokeArgs {
    service: String,
    event: Option<String>,
    timeout_seconds: u64,
}

impl SignalSmokeArgs {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let args = args.into_iter().collect::<Vec<_>>();
        let service = args
            .first()
            .cloned()
            .unwrap_or_else(|| DEFAULT_SERVICE.to_owned());
        let event = args
            .get(1)
            .filter(|event| {
                event.as_str() != "any" && event.as_str() != "--any" && event.as_str() != "*"
            })
            .cloned();
        let timeout_seconds = args
            .get(2)
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|_| format!("timeout seconds must be an unsigned integer: {value}"))
            })
            .transpose()?
            .unwrap_or(DEFAULT_TIMEOUT_SECONDS);

        Ok(Self {
            service,
            event,
            timeout_seconds,
        })
    }
}
