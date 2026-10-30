//! Caller-owned monitor sessions with cooperative cancellation and explicit cleanup.
//!
//! Construction performs no device IO. Keep polling [`crate::session::Session::run`] after
//! requesting cancellation so owned advertisement and source cleanup can finish.
//! Its listener cleanup budget starts afterward; it is not a whole-process bound.
//! Publication acknowledgement does not establish receiver delivery or suspend.

use crate::monitor::{run_monitor, BatterySource, Monitor, MonitorError, Trigger};
use crate::ownership::{AdvertisementBurst, OwnedBroadcaster, RadioPower};
use crate::powerd::SystemPowerdBattery;
use crate::readiness::system_readiness;
use crate::runtime::{BsaAdvertisement, EventListener, RuntimeEvents, StockRadioPower};
use async_channel::{bounded, Receiver, Sender};
use std::future::Future;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::info;

pub mod errors;

use errors::{
    ContinuousError, EventListenerError, InvalidRefreshInterval, PowerdReadError, SampleError,
    StartupError,
};

/// The result after publication cleanup or normal continuous termination.
pub use crate::monitor::Outcome as SessionOutcome;

const BSA_SOCKET: &str = "/var/local/zbluetooth/bt-daemon-socket";
const REFRESH_INTERVAL: Duration = Duration::from_secs(600);
const READINESS_TIMEOUT: Duration = Duration::from_secs(30);

/// Selects a single sample or an event-driven continuous monitor session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunMode {
    /// Publish one sample with packet ID zero and complete required cleanup.
    Once,
    /// Sample initially, on battery/charging notifications, and every ten minutes while awake.
    Continuous,
}

/// A cloneable, nonblocking request to cancel this session's existing waits.
///
/// Requests fan out to startup, battery/burst and event-loop channels. Repeated
/// requests and requests after completion are harmless. Dropping a handle does
/// not cancel the session. Cancellation cannot preempt an owned IO transaction;
/// its future must continue through cleanup.
#[derive(Clone)]
pub struct Cancellation {
    startup: Sender<()>,
    burst: Sender<()>,
    shutdown: Sender<()>,
}

impl Cancellation {
    /// Requests cancellation without waiting for IO or cleanup acknowledgement.
    pub fn request(&self) {
        let _ = self.shutdown.try_send(());
        let _ = self.burst.try_send(());
        let _ = self.startup.try_send(());
    }
}

/// Owns the channels and mode for one consuming monitor run.
///
/// No process signal handler is installed by the library. The caller installs
/// its handler using [`Self::cancellation`] and owns logging setup and exit status.
/// Dropping an unpolled session performs no device IO. Dropping an active run
/// future is not a radio-cleanup mechanism: request cancellation and await it.
pub struct Session {
    mode: RunMode,
    cancellation: Cancellation,
    startup: Receiver<()>,
    burst: Receiver<()>,
    shutdown: Receiver<()>,
}

impl Session {
    /// Allocates local bounded channels without device IO, tasks or clock reads.
    ///
    /// Production refresh, readiness and cleanup durations are fixed by the
    /// repository protocol contract; construction adds no configurable timeouts.
    ///
    /// # Examples
    ///
    /// Constructing and dropping a session or cancellation handle is device-free.
    ///
    /// ```
    /// use kindle_battery_monitor::session::{RunMode, Session};
    /// let session = Session::new(RunMode::Continuous);
    /// let cancellation = session.cancellation();
    /// drop(cancellation); // Dropping a handle does not request cancellation.
    /// drop(session); // An unpolled session has acquired no device resources.
    /// ```
    pub fn new(mode: RunMode) -> Self {
        let (startup, startup_receiver) = bounded(1);
        let (burst, burst_receiver) = bounded(1);
        let (shutdown, shutdown_receiver) = bounded(1);
        Self {
            mode,
            cancellation: Cancellation {
                startup,
                burst,
                shutdown,
            },
            startup: startup_receiver,
            burst: burst_receiver,
            shutdown: shutdown_receiver,
        }
    }

    /// Returns an independent handle; the session retains its own senders.
    pub fn cancellation(&self) -> Cancellation {
        self.cancellation.clone()
    }

    /// Runs the selected mode and finishes required cleanup before returning.
    ///
    /// Startup waits for stock services before battery reads or radio ownership.
    /// Pending battery reads observe cancellation. Once radio ownership is
    /// attempted, cancellation must finish advertisement and source cleanup.
    /// Listener failure requests burst cancellation without dropping publication;
    /// explicit listener teardown follows publication cleanup. Failed battery
    /// reads in continuous mode retry only on another event or ordinary timer.
    ///
    /// The five-second listener cleanup budget excludes prior radio work,
    /// scheduler delays and backend-internal finalization. Battery connection
    /// retains its existing separate startup/cancellation limits. No polling,
    /// wake alarms, suspend inhibitors or physical qualification are added.
    ///
    /// # Errors
    ///
    /// Returns typed stage errors, source chains and ordered cleanup failures.
    /// Simultaneous monitor and listener failures are both retained.
    pub async fn run(self) -> Result<SessionOutcome, SessionError> {
        self.run_with(&mut SystemIo).await
    }

    async fn run_with(self, io: &mut impl SessionIo) -> Result<SessionOutcome, SessionError> {
        io.ready(self.startup.clone(), READINESS_TIMEOUT).await?;
        info!(event = "startup_services_ready");
        let mut battery = io.battery(self.burst.clone()).await?;
        let mut radio = io.radio();
        let mut advertisement = io.advertisement(self.burst.clone());
        let mut broadcaster = OwnedBroadcaster::new(&mut radio, &mut advertisement);
        let seed = match self.mode {
            RunMode::Once => 0,
            RunMode::Continuous => {
                let seconds = io
                    .now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                (seconds & 0xff) as u8
            }
        };
        let mut monitor = Monitor::new(seed);
        if self.mode == RunMode::Once {
            let outcome = match monitor
                .handle(Trigger::Started, &mut battery, &mut broadcaster)
                .await
            {
                Err(MonitorError::Battery(PowerdReadError::Cancelled)) => SessionOutcome::Stopped,
                result => result.map_err(Box::new)?,
            };
            match outcome {
                SessionOutcome::Published => info!(event = "bounded_publication_complete"),
                SessionOutcome::Deferred => info!(event = "bounded_publication_deferred"),
                SessionOutcome::Stopped => info!(event = "monitor_stopped"),
            }
            return Ok(outcome);
        }
        let (mut listener, changed) = io.listener(self.startup.clone()).await?;
        let mut events = io.events(changed, self.shutdown.clone(), REFRESH_INTERVAL)?;
        let result = listener
            .drive(
                run_monitor(&mut monitor, &mut battery, &mut broadcaster, &mut events),
                || {
                    let _ = self.cancellation.burst.try_send(());
                },
            )
            .await?;
        let listener_result = listener.stop().await;
        finish_session(result, listener_result)?;
        info!(event = "monitor_stopped");
        Ok(SessionOutcome::Stopped)
    }
}

/// A session stage failure, preserving underlying causes and cleanup evidence.
#[derive(Debug, Error)]
pub enum SessionError {
    /// The fixed refresh interval could not be represented by the monotonic clock.
    #[error(transparent)]
    RefreshInterval(#[from] InvalidRefreshInterval),
    /// Stock-service readiness failed or was cancelled before device work.
    #[error(transparent)]
    Readiness(#[from] StartupError),
    /// Opening the battery connection failed.
    #[error(transparent)]
    Battery(#[from] PowerdReadError),
    /// Listener setup, receive or cleanup failed.
    #[error(transparent)]
    Listener(#[from] EventListenerError),
    /// Single-sample read, validation or publication failed.
    #[error(transparent)]
    Once(#[from] Box<SampleError>),
    /// Continuous event reception or publication failed.
    #[error(transparent)]
    Run(#[from] Box<ContinuousError>),
    /// The monitor failed and listener teardown also reported failures.
    #[error("monitor failed: {primary}; listener cleanup failed: {listener}")]
    Combined {
        /// The monitor failure, exposed by standard source traversal.
        #[source]
        primary: Box<ContinuousError>,
        /// The listener failure, retaining ordered cleanup errors separately.
        listener: Box<EventListenerError>,
    },
}

fn finish_session(
    result: Result<(), ContinuousError>,
    listener: Result<(), EventListenerError>,
) -> Result<(), SessionError> {
    match (result, listener) {
        (Err(primary), Err(listener)) => Err(SessionError::Combined {
            primary: Box::new(primary),
            listener: Box::new(listener),
        }),
        (Err(primary), Ok(())) => Err(SessionError::Run(Box::new(primary))),
        (Ok(()), Err(listener)) => Err(SessionError::Listener(listener)),
        (Ok(()), Ok(())) => Ok(()),
    }
}

// Only device construction and external waits vary. Policy stays in the real
// monitor, owned broadcaster, advertisement sequence and runtime event driver.
trait SessionIo {
    type Battery: BatterySource<Error = PowerdReadError>;
    type Radio: RadioPower<Error = errors::RadioError>;
    type Advertisement: AdvertisementBurst<Error = errors::BsaBurstError>;
    type Listener: SessionListener;

    fn ready(
        &mut self,
        cancelled: Receiver<()>,
        timeout: Duration,
    ) -> impl Future<Output = Result<(), StartupError>>;
    fn battery(
        &mut self,
        cancelled: Receiver<()>,
    ) -> impl Future<Output = Result<Self::Battery, PowerdReadError>>;
    fn radio(&mut self) -> Self::Radio;
    fn advertisement(&mut self, cancelled: Receiver<()>) -> Self::Advertisement;
    fn now(&mut self) -> SystemTime;
    fn listener(
        &mut self,
        cancelled: Receiver<()>,
    ) -> impl Future<Output = Result<(Self::Listener, Receiver<()>), EventListenerError>>;
    fn events(
        &mut self,
        changed: Receiver<()>,
        shutdown: Receiver<()>,
        interval: Duration,
    ) -> Result<RuntimeEvents, InvalidRefreshInterval> {
        RuntimeEvents::new(changed, shutdown, interval)
    }
}

trait SessionListener {
    fn drive<T>(
        &mut self,
        operation: impl Future<Output = T>,
        cancel: impl FnOnce(),
    ) -> impl Future<Output = Result<T, EventListenerError>>;
    fn stop(&mut self) -> impl Future<Output = Result<(), EventListenerError>>;
}

impl SessionListener for EventListener {
    async fn drive<T>(
        &mut self,
        operation: impl Future<Output = T>,
        cancel: impl FnOnce(),
    ) -> Result<T, EventListenerError> {
        self.run_scoped(operation, cancel).await
    }
    async fn stop(&mut self) -> Result<(), EventListenerError> {
        EventListener::stop(self).await
    }
}

struct SystemIo;
impl SessionIo for SystemIo {
    type Battery = SystemPowerdBattery;
    type Radio = StockRadioPower;
    type Advertisement = BsaAdvertisement;
    type Listener = EventListener;
    async fn ready(
        &mut self,
        cancelled: Receiver<()>,
        timeout: Duration,
    ) -> Result<(), StartupError> {
        system_readiness(
            async {
                let _ = cancelled.recv().await;
            },
            timeout,
        )
        .await
    }
    async fn battery(&mut self, cancelled: Receiver<()>) -> Result<Self::Battery, PowerdReadError> {
        Ok(SystemPowerdBattery::system()
            .await?
            .with_cancellation(cancelled))
    }
    fn radio(&mut self) -> Self::Radio {
        StockRadioPower
    }
    fn advertisement(&mut self, cancelled: Receiver<()>) -> Self::Advertisement {
        BsaAdvertisement::new(PathBuf::from(BSA_SOCKET), cancelled)
    }
    fn now(&mut self) -> SystemTime {
        SystemTime::now()
    }
    async fn listener(
        &mut self,
        cancelled: Receiver<()>,
    ) -> Result<(Self::Listener, Receiver<()>), EventListenerError> {
        EventListener::start_with_cancellation(async {
            let _ = cancelled.recv().await;
        })
        .await
    }
}

#[cfg(test)]
mod tests;
