//! Blocking runtime adapters for events, stock radio ownership, and BSA bursts.
//!
//! Channel waits and ordinary timers permit normal suspend; no wake alarm is
//! installed. The burst timer is five seconds from before radio preflight, not a
//! hard RF cutoff. Device IO and subsequent cleanup can extend elapsed time.

use crate::adapter::{BsaRadio, BsaRadioError, SocketDaemon};
use crate::broadcast::{run_broadcast, BroadcastError, EndReason, WaitForEnd};
use crate::bthome::BthomeSample;
use crate::monitor::{EventSource, Publication, Trigger};
use crate::ownership::{AdvertisementBurst, RadioPower};
use crate::powerd_events::{PowerdEventError, PowerdEvents};
use crate::radio::{
    DbusRadio, RadioCommand, RadioController, RadioError, RadioProtocol, RadioState,
};
use crossbeam_channel::{after, bounded, tick, Receiver, TrySendError};
use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;

/// A terminal failure of the runtime event source.
#[derive(Debug, Error)]
pub enum RuntimeEventError {
    /// The battery notification channel disconnected.
    #[error("powerd battery event listener stopped")]
    BatteryListenerStopped,
}

/// An event source that blocks on shutdown, battery notifications, or refresh.
///
/// Ready shutdown messages or shutdown-channel disconnection take priority over
/// battery notifications, which take priority over refresh. Battery-channel
/// disconnection is an error. No device probes occur while waiting.
pub struct RuntimeEvents {
    battery_changed: Receiver<()>,
    shutdown: Receiver<()>,
    refresh: Receiver<Instant>,
}

impl RuntimeEvents {
    /// Owns event receivers and creates an ordinary periodic refresh timer.
    ///
    /// The refresh timer does not create a system wake alarm. Construction performs
    /// no device IO. Initial sampling is handled separately by the monitor.
    /// Supply a positive refresh interval to permit blocking between refreshes;
    /// zero leaves the refresh channel continually ready and is not rejected here.
    pub fn new(
        battery_changed: Receiver<()>,
        shutdown: Receiver<()>,
        refresh_interval: Duration,
    ) -> Self {
        Self {
            battery_changed,
            shutdown,
            refresh: tick(refresh_interval),
        }
    }
}

impl EventSource for RuntimeEvents {
    type Error = RuntimeEventError;

    fn next(&mut self) -> Result<Trigger, Self::Error> {
        crossbeam_channel::select_biased! {
            recv(self.shutdown) -> _ => Ok(Trigger::Shutdown),
            recv(self.battery_changed) -> message => message.map(|()| Trigger::BatteryChanged).map_err(|_| RuntimeEventError::BatteryListenerStopped),
            recv(self.refresh) -> _ => Ok(Trigger::Refresh),
        }
    }
}

/// Blocking stock radio operations built from the asynchronous DBus controller.
///
/// Each method opens a system-bus connection. Enable/disable use controller
/// deadlines; the standalone ownership read adds no overall readback timeout.
/// Callers must acquire source 4 before releasing it.
pub struct StockRadioPower;

impl RadioPower for StockRadioPower {
    type Error = RadioError;

    fn state(&mut self) -> Result<RadioState, Self::Error> {
        async_io::block_on(async { DbusRadio::system().await?.enabled_sources().await })
    }

    fn enable_source4(&mut self) -> Result<(), Self::Error> {
        async_io::block_on(async {
            let mut radio = RadioController::new(DbusRadio::system().await?);
            radio.execute(RadioCommand::EnableBle).await.map(|_| ())
        })
    }

    fn disable_source4(&mut self) -> Result<(), Self::Error> {
        async_io::block_on(async {
            let mut radio = RadioController::new(DbusRadio::system().await?);
            radio.execute(RadioCommand::DisableBle).await.map(|_| ())
        })
    }
}

/// A BSA advertisement or cleanup failure.
#[derive(Debug, Error)]
pub enum BsaBurstError {
    /// The burst failed, with advertisement cleanup errors retained.
    #[error(transparent)]
    Broadcast(#[from] BroadcastError<BsaRadioError>),
}

/// One bounded BSA advertisement implementation inside a caller-owned radio lease.
///
/// Uses one-second BSA exchange deadlines and a five-second timer created before
/// preflight. A consumed cancellation message can prevent preflight; cancellation
/// or channel disconnection during the burst wait ends it early. Configuration
/// and socket exchanges are not interrupted by cancellation. After configuration
/// begins, stop, readback, and stock-rotation requests are attempted before return.
/// An existing advertisement defers publication without modifying it.
pub struct BsaAdvertisement {
    socket: PathBuf,
    cancelled: Receiver<()>,
}

impl BsaAdvertisement {
    /// Stores the socket path and burst-cancellation receiver without connecting.
    ///
    /// This constructor does not acquire source ownership; use it through
    /// [`crate::ownership::OwnedBroadcaster`].
    pub fn new(socket: PathBuf, cancelled: Receiver<()>) -> Self {
        Self { socket, cancelled }
    }
}

struct BurstDeadline {
    elapsed: Receiver<Instant>,
    cancelled: Receiver<()>,
}

impl WaitForEnd for BurstDeadline {
    fn wait(self) -> EndReason {
        crossbeam_channel::select_biased! {
            recv(self.cancelled) -> _ => EndReason::Cancelled,
            recv(self.elapsed) -> _ => EndReason::Deadline,
        }
    }
}

impl AdvertisementBurst for BsaAdvertisement {
    type Error = BsaBurstError;

    fn broadcast(&mut self, sample: BthomeSample) -> Result<Publication, Self::Error> {
        if self.cancelled.try_recv().is_ok() {
            return Ok(Publication::Cancelled);
        }
        let mut radio = BsaRadio::new(SocketDaemon::new(
            self.socket.clone(),
            Duration::from_secs(1),
        ));
        let deadline = BurstDeadline {
            elapsed: after(Duration::from_secs(5)),
            cancelled: self.cancelled.clone(),
        };
        match run_broadcast(&mut radio, sample, deadline) {
            Ok(EndReason::Deadline) => Ok(Publication::Published),
            Ok(EndReason::Cancelled) => Ok(Publication::Cancelled),
            Err(BroadcastError::Unavailable(BsaRadioError::ExistingAdvertisement)) => {
                Ok(Publication::Deferred)
            }
            Err(error) => Err(BsaBurstError::Broadcast(error)),
        }
    }
}

/// A listener setup, receive, or thread-join failure.
#[derive(Debug, Error)]
pub enum EventListenerError {
    /// Connecting, subscribing, or receiving a powerd notification failed.
    #[error(transparent)]
    Powerd(#[from] PowerdEventError),
    /// The worker thread panicked before it could return its result.
    #[error("powerd event listener thread panicked")]
    ThreadPanicked,
}

/// Owns a powerd subscription worker and its explicit stop channel.
///
/// Repeated notifications coalesce in a one-slot channel. Dropping this value
/// requests stop and joins the worker, discarding any error; call [`Self::stop`]
/// explicitly to observe failures.
pub struct EventListener {
    stop: async_channel::Sender<()>,
    thread: Option<JoinHandle<Result<(), PowerdEventError>>>,
}

impl EventListener {
    /// Subscribes before spawning the worker and returning its notification receiver.
    ///
    /// The worker blocks on notifications or stop. Dropping the notification receiver
    /// is noticed on the next attempted send; use [`Self::stop`] for prompt shutdown.
    ///
    /// # Errors
    ///
    /// Returns connection or subscription failures. Setup has no overall timeout.
    pub fn start() -> Result<(Self, Receiver<()>), EventListenerError> {
        let mut events = async_io::block_on(PowerdEvents::system())?;
        let (changed, changed_rx) = bounded(1);
        let (stop, stopped) = async_channel::bounded(1);
        let thread = thread::spawn(move || {
            async_io::block_on(async move {
                loop {
                    enum Next {
                        Changed(Result<(), PowerdEventError>),
                        Stop,
                    }
                    let next = futures_lite::future::or(
                        async { Next::Changed(events.next().await) },
                        async {
                            let _ = stopped.recv().await;
                            Next::Stop
                        },
                    )
                    .await;
                    match next {
                        Next::Changed(Ok(())) => match changed.try_send(()) {
                            Ok(()) | Err(TrySendError::Full(())) => {}
                            Err(TrySendError::Disconnected(())) => return Ok(()),
                        },
                        Next::Changed(Err(error)) => return Err(error),
                        Next::Stop => return Ok(()),
                    }
                }
            })
        });
        Ok((
            Self {
                stop,
                thread: Some(thread),
            },
            changed_rx,
        ))
    }

    /// Requests cancellation of the pending event wait and synchronously joins once.
    ///
    /// Subsequent calls succeed without another join. Joining has no explicit timeout;
    /// the worker's stop branch can cancel its pending receive without a device event.
    ///
    /// # Errors
    ///
    /// Returns the worker's receive failure or a thread-panic error.
    pub fn stop(&mut self) -> Result<(), EventListenerError> {
        let _ = self.stop.try_send(());
        if let Some(thread) = self.thread.take() {
            match thread.join() {
                Ok(result) => result.map_err(EventListenerError::Powerd),
                Err(_) => Err(EventListenerError::ThreadPanicked),
            }
        } else {
            Ok(())
        }
    }
}

impl Drop for EventListener {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
