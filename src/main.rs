//! Process-level CLI, signal handling and tracing for monitor sessions.
//!
//! Device composition and owned cleanup belong to the library session. Help
//! performs no device IO; the process owns logging setup and exit status.

use kindle_battery_monitor::logging::BoundedJsonLog;
use kindle_battery_monitor::session::{RunMode, Session, SessionError};
use std::num::NonZeroU64;
use std::process::ExitCode;
use thiserror::Error;
use tracing::error;

const USAGE: &str = "Usage: kindle-battery-monitor run | --once | --help";
const LOG_PATH: &str = "/var/log/kindle-battery-monitor.jsonl";

#[derive(Debug, Error)]
enum AppError {
    #[error("usage: {USAGE}")]
    Usage,
    #[error("could not install shutdown handler: {0}")]
    Signal(#[from] ctrlc::Error),
    #[error("could not open bounded monitor log: {0}")]
    Log(#[from] std::io::Error),
    #[error(transparent)]
    Session(#[from] Box<SessionError>),
}

async fn run(mode: RunMode) -> Result<(), AppError> {
    let session = Session::new(mode);
    let cancellation = session.cancellation();
    ctrlc::set_handler(move || cancellation.request())?;
    session.run().await.map_err(Box::new)?;
    Ok(())
}

fn initialize_logging(arguments: &[String]) -> Result<(), AppError> {
    if arguments == ["--help"] {
        return Ok(());
    }
    let format = tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .with_target(false);
    if arguments == ["run"] {
        let maximum = NonZeroU64::new(256 * 1024).expect("log limit is nonzero");
        let log = BoundedJsonLog::open(LOG_PATH, maximum)?;
        format.with_writer(log).init();
    } else {
        format.init();
    }
    Ok(())
}

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if let Err(error) = initialize_logging(&arguments) {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }
    let result = match arguments.as_slice() {
        [argument] if argument == "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        [argument] if argument == "--once" => async_io::block_on(run(RunMode::Once)),
        [argument] if argument == "run" => async_io::block_on(run(RunMode::Continuous)),
        _ => Err(AppError::Usage),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!(event = "monitor_failed", error = %error);
            ExitCode::FAILURE
        }
    }
}
