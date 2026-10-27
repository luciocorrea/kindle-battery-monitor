//! Command-line wiring for continuous or single-sample monitor sessions.
//!
//! Startup waits for stock services before reading powerd or acquiring BLE source 4.
//! Signal handlers notify cancellation channels; owned burst cleanup completes
//! before the session returns. Continuous sessions use a ten-minute refresh timer
//! and permit normal suspend.

use crossbeam_channel::bounded;
use kindle_battery_monitor::logging::BoundedJsonLog;
use kindle_battery_monitor::monitor::{run_monitor, Monitor, MonitorError, RunError, Trigger};
use kindle_battery_monitor::ownership::{OwnedBroadcastError, OwnedBroadcaster};
use kindle_battery_monitor::powerd::PowerdBattery;
use kindle_battery_monitor::radio::RadioError;
use kindle_battery_monitor::readiness::{system_readiness, ReadinessError};
use kindle_battery_monitor::runtime::{
    BsaAdvertisement, BsaBurstError, EventListener, EventListenerError, RuntimeEventError,
    RuntimeEvents, StockRadioPower,
};
use kindle_lipc_dbus::LipcDbusError;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::{error, info};

const USAGE: &str = "Usage: kindle-battery-monitor run | --once | --help";
const BSA_SOCKET: &str = "/var/local/zbluetooth/bt-daemon-socket";
const LOG_PATH: &str = "/var/log/kindle-battery-monitor.jsonl";
const REFRESH_INTERVAL: Duration = Duration::from_secs(600);

type BurstError = OwnedBroadcastError<RadioError, BsaBurstError>;

#[derive(Clone, Copy)]
enum RunMode {
    Once,
    Continuous,
}

#[derive(Debug, Error)]
enum AppError {
    #[error(transparent)]
    Readiness(#[from] ReadinessError<zbus::Error>),
    #[error("usage: {USAGE}")]
    Usage,
    #[error(transparent)]
    Battery(#[from] LipcDbusError),
    #[error(transparent)]
    Listener(#[from] EventListenerError),
    #[error("could not install shutdown handler: {0}")]
    Signal(#[from] ctrlc::Error),
    #[error("could not open bounded monitor log: {0}")]
    Log(#[from] std::io::Error),
    #[error(transparent)]
    Once(#[from] MonitorError<LipcDbusError, BurstError>),
    #[error(transparent)]
    Run(#[from] RunError<RuntimeEventError, BurstError>),
}

fn packet_id_seed() -> u8 {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    (seconds & 0xff) as u8
}

fn initial_packet_id(mode: RunMode) -> u8 {
    match mode {
        RunMode::Once => 0,
        RunMode::Continuous => packet_id_seed(),
    }
}

fn run(mode: RunMode) -> Result<(), AppError> {
    let (shutdown_sender, shutdown_receiver) = bounded(1);
    let (cancel_sender, cancel_receiver) = bounded(1);
    let (startup_cancel_sender, startup_cancel_receiver) = async_channel::bounded(1);
    ctrlc::set_handler(move || {
        let _ = shutdown_sender.try_send(());
        let _ = cancel_sender.try_send(());
        let _ = startup_cancel_sender.try_send(());
    })?;

    async_io::block_on(system_readiness(
        async {
            let _ = startup_cancel_receiver.recv().await;
        },
        Duration::from_secs(30),
    ))?;
    info!(event = "startup_services_ready");
    let mut battery = PowerdBattery::system()?;
    let mut radio = StockRadioPower;
    let mut advertisement = BsaAdvertisement::new(PathBuf::from(BSA_SOCKET), cancel_receiver);
    let mut broadcaster = OwnedBroadcaster::new(&mut radio, &mut advertisement);
    let mut monitor = Monitor::new(initial_packet_id(mode));
    match mode {
        RunMode::Once => {
            match monitor.handle(Trigger::Started, &mut battery, &mut broadcaster)? {
                kindle_battery_monitor::monitor::Outcome::Published => {
                    info!(event = "bounded_publication_complete")
                }
                kindle_battery_monitor::monitor::Outcome::Deferred => {
                    info!(event = "bounded_publication_deferred")
                }
                kindle_battery_monitor::monitor::Outcome::Stopped => {
                    info!(event = "monitor_stopped")
                }
            }
            return Ok(());
        }
        RunMode::Continuous => {}
    }

    let (mut listener, battery_changed) = EventListener::start()?;
    let mut events = RuntimeEvents::new(battery_changed, shutdown_receiver, REFRESH_INTERVAL);
    let result = run_monitor(&mut monitor, &mut battery, &mut broadcaster, &mut events);
    let listener_result = listener.stop();
    result?;
    listener_result?;
    info!(event = "monitor_stopped");
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
        [argument] if argument == "--once" => run(RunMode::Once),
        [argument] if argument == "run" => run(RunMode::Continuous),
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

#[cfg(test)]
mod tests {
    #[test]
    fn bounded_smoke_uses_a_known_packet_id() {
        assert_eq!(super::initial_packet_id(super::RunMode::Once), 0);
    }
}
