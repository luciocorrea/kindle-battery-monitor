//! Read-only system-bus powerd probe command.
//!
//! Prints diagnostic JSON Lines and writes the same report to the configured
//! Kindle user-storage path. The report can contain returned property values;
//! this command is for explicit diagnostic use, not normal monitor operation.

use std::{fs, process::ExitCode};

use kindle_lipc_dbus::{run_smoke_probe, LipcDbusClient};

const REPORT_PATH: &str = "/mnt/us/kindle-lipc-dbus-smoke.jsonl";

fn main() -> ExitCode {
    let client = match LipcDbusClient::system() {
        Ok(client) => client,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    let report = run_smoke_probe(&client);
    let jsonl = report.to_jsonl();
    print!("{jsonl}");

    if let Err(error) = fs::write(REPORT_PATH, jsonl) {
        eprintln!("failed to write {REPORT_PATH}: {error}");
        return ExitCode::FAILURE;
    }

    match report.exit_code().as_i32() {
        0 => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}
