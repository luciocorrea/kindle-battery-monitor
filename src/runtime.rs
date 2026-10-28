//! Cooperative runtime adapters for events, stock radio ownership, and BSA bursts.
//!
//! Channel waits and ordinary timers permit normal suspend; no wake alarm is
//! installed. The burst timer is five seconds from before radio preflight, not a
//! hard RF cutoff. Device IO and subsequent cleanup can extend elapsed time.

use crate::adapter::{BsaRadio, BsaRadioError, SocketDaemon};
use crate::broadcast::{run_broadcast, BroadcastError, EndReason, WaitForEnd};
use crate::bthome::BthomeSample;
use crate::monitor::{EventSource, Publication, Trigger};
use crate::ownership::{AdvertisementBurst, RadioPower};
use crate::powerd_events::{PowerdCleanupError, PowerdEventError, PowerdEvents};
use crate::radio::{
    DbusRadio, RadioCommand, RadioController, RadioError, RadioProtocol, RadioState,
};
use async_channel::{bounded, Receiver, TrySendError};
use futures_lite::{Stream, StreamExt};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use thiserror::Error;

#[cfg(test)]
mod event_tests;

#[cfg(test)]
mod ownership_tests;

#[cfg(test)]
mod listener_setup_tests;

#[cfg(test)]
mod listener_driver_tests;

#[cfg(test)]
mod listener_stop_tests;

/// A terminal failure of the runtime event source.
#[derive(Debug, Error)]
pub enum RuntimeEventError {
    /// The battery notification channel disconnected.
    #[error("powerd battery event listener stopped")]
    BatteryListenerStopped,
}

/// A refresh interval that is zero or outside the monotonic clock's range.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("refresh interval must be positive and fit the monotonic clock, received {interval:?}")]
pub struct InvalidRefreshInterval {
    /// The rejected duration, retained without clamping.
    pub interval: Duration,
}

/// An event source that blocks on shutdown, battery notifications, or refresh.
///
/// Ready shutdown messages or shutdown-channel disconnection take priority over
/// battery notifications, which take priority over refresh. Battery-channel
/// disconnection is an error, including when a trigger was already buffered.
/// No device probes occur while waiting.
pub(crate) struct RuntimeEvents {
    battery_changed: Receiver<()>,
    shutdown: Receiver<()>,
    refresh: RefreshStream,
}

pub(crate) type RefreshStream = Pin<Box<dyn Stream<Item = Instant> + Send>>;

struct RefreshTimer {
    timer: async_io::Timer,
    interval: Duration,
}

impl Stream for RefreshTimer {
    type Item = Instant;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Instant>> {
        match Pin::new(&mut self.timer).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(instant) => {
                // Match the previous tick channel: late refreshes do not replay
                // missed intervals or create a catch-up sampling burst.
                let interval = self.interval;
                self.timer.set_after(interval);
                Poll::Ready(Some(instant))
            }
        }
    }
}

impl RuntimeEvents {
    /// Owns event receivers and creates an ordinary periodic refresh timer.
    ///
    /// The refresh timer does not create a system wake alarm. Construction performs
    /// no device IO. Initial sampling is handled separately by the monitor.
    /// A positive interval permits blocking between refreshes. This validates the
    /// initial deadline against the current monotonic clock before creating the
    /// timer; it does not guarantee when a scheduled thread will run.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRefreshInterval`] for zero or an unrepresentable initial
    /// deadline. Invalid input creates no timer.
    pub(crate) fn new(
        battery_changed: Receiver<()>,
        shutdown: Receiver<()>,
        refresh_interval: Duration,
    ) -> Result<Self, InvalidRefreshInterval> {
        Self::with_timer(battery_changed, shutdown, refresh_interval, |interval| {
            Box::pin(RefreshTimer {
                timer: async_io::Timer::after(interval),
                interval,
            })
        })
    }

    pub(crate) fn with_timer(
        battery_changed: Receiver<()>,
        shutdown: Receiver<()>,
        refresh_interval: Duration,
        make_timer: impl FnOnce(Duration) -> RefreshStream,
    ) -> Result<Self, InvalidRefreshInterval> {
        if refresh_interval.is_zero() || Instant::now().checked_add(refresh_interval).is_none() {
            return Err(InvalidRefreshInterval {
                interval: refresh_interval,
            });
        }
        Ok(Self {
            battery_changed,
            shutdown,
            refresh: make_timer(refresh_interval),
        })
    }
}

impl EventSource for RuntimeEvents {
    type Error = RuntimeEventError;

    async fn next(&mut self) -> Result<Trigger, Self::Error> {
        futures_lite::future::or(
            async {
                let _ = self.shutdown.recv().await;
                Ok(Trigger::Shutdown)
            },
            futures_lite::future::or(
                async {
                    if self.battery_changed.is_closed() {
                        return Err(RuntimeEventError::BatteryListenerStopped);
                    }
                    self.battery_changed
                        .recv()
                        .await
                        .map_err(|_| RuntimeEventError::BatteryListenerStopped)?;
                    if self.battery_changed.is_closed() {
                        Err(RuntimeEventError::BatteryListenerStopped)
                    } else {
                        Ok(Trigger::BatteryChanged)
                    }
                },
                async {
                    let _ = self.refresh.next().await;
                    Ok(Trigger::Refresh)
                },
            ),
        )
        .await
    }
}

/// Cooperative stock radio operations built from the asynchronous DBus controller.
///
/// Each method opens a system-bus connection. Ownership inspection has one
/// five-second budget covering connection, owner lookup, and source readback.
/// Enable/disable retain their separate connection and controller deadlines.
/// Callers must acquire source 4 before releasing it.
pub(crate) struct StockRadioPower;

async fn inspect_ownership<P: RadioProtocol>(
    connect: impl Future<Output = Result<P, RadioError>>,
    expired: impl Future<Output = ()>,
) -> Result<RadioState, RadioError> {
    futures_lite::future::or(
        async {
            expired.await;
            Err(RadioError::OwnershipTimeout)
        },
        async {
            let mut radio = connect.await?;
            radio.enabled_sources().await
        },
    )
    .await
}

impl RadioPower for StockRadioPower {
    type Error = RadioError;

    async fn state(&mut self) -> Result<RadioState, Self::Error> {
        // Create the shared deadline before polling connection setup. Cancelling
        // inspection does not cancel the caller's separate source-release attempt.
        let deadline = async_io::Timer::after(Duration::from_secs(5));
        inspect_ownership(DbusRadio::system(), async {
            deadline.await;
        })
        .await
    }

    async fn enable_source4(&mut self) -> Result<(), Self::Error> {
        let mut radio = RadioController::new(DbusRadio::system().await?);
        radio.execute(RadioCommand::EnableBle).await.map(|_| ())
    }

    async fn disable_source4(&mut self) -> Result<(), Self::Error> {
        let mut radio = RadioController::new(DbusRadio::system().await?);
        radio.execute(RadioCommand::DisableBle).await.map(|_| ())
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
pub(crate) struct BsaAdvertisement {
    socket: PathBuf,
    cancelled: Receiver<()>,
}

impl BsaAdvertisement {
    /// Stores the socket path and burst-cancellation receiver without connecting.
    ///
    /// This constructor does not acquire source ownership; use it through
    /// [`crate::ownership::OwnedBroadcaster`].
    pub(crate) fn new(socket: PathBuf, cancelled: Receiver<()>) -> Self {
        Self { socket, cancelled }
    }
}

struct BurstDeadline {
    elapsed: async_io::Timer,
    cancelled: Receiver<()>,
}

impl WaitForEnd for BurstDeadline {
    async fn wait(self) -> EndReason {
        futures_lite::future::or(
            async {
                let _ = self.cancelled.recv().await;
                EndReason::Cancelled
            },
            async {
                self.elapsed.await;
                EndReason::Deadline
            },
        )
        .await
    }
}

impl AdvertisementBurst for BsaAdvertisement {
    type Error = BsaBurstError;

    async fn broadcast(&mut self, sample: BthomeSample) -> Result<Publication, Self::Error> {
        if self.cancelled.try_recv().is_ok() {
            return Ok(Publication::Cancelled);
        }
        let mut radio = BsaRadio::new(SocketDaemon::new(
            self.socket.clone(),
            Duration::from_secs(1),
        ));
        let deadline = BurstDeadline {
            elapsed: async_io::Timer::after(Duration::from_secs(5)),
            cancelled: self.cancelled.clone(),
        };
        publication_result(run_broadcast(&mut radio, sample, deadline).await)
    }
}

// Shared outcome conversion keeps the native adapter and injected session IO on
// the same publication contract without duplicating transaction policy.
pub(crate) fn publication_result(
    result: Result<EndReason, BroadcastError<BsaRadioError>>,
) -> Result<Publication, BsaBurstError> {
    match result {
        Ok(EndReason::Deadline) => Ok(Publication::Published),
        Ok(EndReason::Cancelled) => Ok(Publication::Cancelled),
        Err(BroadcastError::Unavailable(BsaRadioError::ExistingAdvertisement)) => {
            Ok(Publication::Deferred)
        }
        Err(error) => Err(BsaBurstError::Broadcast(error)),
    }
}

/// A listener setup, receive, or cleanup failure.
#[derive(Clone, Debug, Error)]
pub enum EventListenerError {
    /// Connection and acknowledged subscription exceeded the shared setup budget.
    #[error("powerd event listener setup exceeded its overall deadline")]
    SetupTimeout,
    /// Startup cancellation won before a listener worker was created.
    #[error("powerd event listener setup cancelled")]
    SetupCancelled,
    /// Connecting, subscribing, or receiving a powerd notification failed.
    #[error(transparent)]
    Powerd(#[from] PowerdEventError),
    /// A legacy listener worker reported a panic.
    #[error("powerd event listener thread panicked")]
    ThreadPanicked,
    /// The scoped driver was invoked after its one execution or shutdown.
    #[error("listener is not running")]
    NotRunning,
    /// Ordered receive, unsubscribe, and close failures from explicit shutdown.
    #[error("listener shutdown failed: {0:?}")]
    Shutdown(Vec<PowerdCleanupError>),
}

/// Owns a dedicated subscription and one caller-scoped notification driver.
///
/// [`Self::run_scoped`] drives coalescing alongside cooperative runtime IO. No
/// project thread or detached task owns the subscription. Explicit [`Self::stop`]
/// returns cleanup evidence; Drop only releases local state without waiting.
pub(crate) struct EventListener {
    events: Option<PowerdEvents>,
    changed: Option<async_channel::Sender<()>>,
    receive_error: Option<PowerdEventError>,
    stopped: Option<Vec<PowerdCleanupError>>,
}

async fn finish_listener_setup<T, R>(
    setup: impl Future<Output = Result<T, PowerdEventError>>,
    cancelled: impl Future<Output = ()>,
    expired: impl Future<Output = ()>,
    finish: impl FnOnce(T) -> R,
) -> Result<R, EventListenerError> {
    let events = futures_lite::future::or(
        async {
            cancelled.await;
            Err(EventListenerError::SetupCancelled)
        },
        futures_lite::future::or(
            async {
                expired.await;
                Err(EventListenerError::SetupTimeout)
            },
            async { setup.await.map_err(EventListenerError::from) },
        ),
    )
    .await?;
    Ok(finish(events))
}

impl EventListener {
    /// Creates the subscription, prioritizing cancellation before setup IO.
    ///
    /// One five-second budget covers connection and acknowledged registration.
    /// Failed setup drops local ownership and does not start a background driver.
    /// Remote cleanup acknowledgement is not implied by cancellation.
    ///
    /// # Errors
    /// Returns [`EventListenerError::SetupCancelled`],
    /// [`EventListenerError::SetupTimeout`], or a powerd subscription error.
    pub(crate) async fn start_with_cancellation(
        cancelled: impl Future<Output = ()>,
    ) -> Result<(Self, Receiver<()>), EventListenerError> {
        let deadline = async_io::Timer::after(Duration::from_secs(5));
        finish_listener_setup(
            PowerdEvents::system(),
            cancelled,
            async {
                deadline.await;
            },
            Self::from_events,
        )
        .await
    }

    fn from_events(events: PowerdEvents) -> (Self, Receiver<()>) {
        let (changed, receiver) = bounded(1);
        (
            Self {
                events: Some(events),
                changed: Some(changed),
                receive_error: None,
                stopped: None,
            },
            receiver,
        )
    }

    /// Drives one operation alongside a scoped, one-slot notification coalescer.
    ///
    /// The operation must use cooperative async waits. On listener failure, its
    /// trigger channel closes and the monitor must finish owned radio cleanup
    /// before returning. This method never drops an active publication to cancel
    /// it. On receive failure `cancel_publication` must signal the existing burst
    /// cancellation path. Callers must keep polling the operation until
    /// its cleanup completes. The coalescer borrows this listener and cannot
    /// outlive the call. Ten-minute refresh and event priority remain with
    /// [`RuntimeEvents`].
    ///
    /// # Errors
    /// Returns [`EventListenerError::NotRunning`] after execution or shutdown.
    /// Receive failures are retained for [`Self::stop`], alongside cleanup errors.
    pub(crate) async fn run_scoped<T>(
        &mut self,
        operation: impl Future<Output = T>,
        cancel_publication: impl FnOnce(),
    ) -> Result<T, EventListenerError> {
        let events = self.events.as_mut().ok_or(EventListenerError::NotRunning)?;
        let changed = self.changed.take().ok_or(EventListenerError::NotRunning)?;
        let receive = futures_lite::stream::unfold(events, |events| async {
            Some((events.next().await, events))
        });
        futures_lite::pin!(receive);
        Ok(drive_listener(
            receive,
            changed,
            &mut self.receive_error,
            operation,
            cancel_publication,
        )
        .await)
    }

    /// Releases the dedicated subscription under one five-second cleanup budget.
    ///
    /// Call after the scoped operation has completed its required radio cleanup.
    /// Match removal gets at most four seconds, preserving a one-second close
    /// reserve inside the original deadline. Close is attempted even after failed
    /// removal. Timeout drops all foreground listener state and reports remote
    /// cleanup as unconfirmed. Backend socket finalization and scheduler delays
    /// are outside this cooperative bound. Repeated calls return the recorded
    /// outcome without new IO. Cancelling the stop future releases local state;
    /// it cannot acknowledge remote cleanup.
    ///
    /// # Errors
    /// Returns [`EventListenerError::Shutdown`] with all failures in operation order.
    pub(crate) async fn stop(&mut self) -> Result<(), EventListenerError> {
        self.stop_with_cleanup(async |events, failures| {
            let now = Instant::now();
            match (
                now.checked_add(Duration::from_secs(4)),
                now.checked_add(Duration::from_secs(5)),
            ) {
                (Some(unsubscribe_at), Some(close_at)) => {
                    events
                        .shutdown_recording(
                            async {
                                async_io::Timer::at(unsubscribe_at).await;
                            },
                            async {
                                async_io::Timer::at(close_at).await;
                            },
                            failures,
                        )
                        .await
                }
                _ => {
                    drop(events);
                    failures.push(PowerdCleanupError::InvalidDeadline);
                }
            }
        })
        .await
    }

    async fn stop_with_cleanup(
        &mut self,
        cleanup: impl AsyncFnOnce(PowerdEvents, &mut Vec<PowerdCleanupError>),
    ) -> Result<(), EventListenerError> {
        if let Some(failures) = &self.stopped {
            return shutdown_result(failures.clone());
        }
        // A caller dropping this future must not turn its next stop into a
        // success merely because the foreground state was already taken.
        drop(self.changed.take());
        let mut failures: Vec<_> = self
            .receive_error
            .take()
            .map(PowerdCleanupError::Receive)
            .into_iter()
            .collect();
        failures.push(PowerdCleanupError::Interrupted);
        self.stopped = Some(failures);
        // Keep progress in the owner while cleanup awaits. Dropping this stop
        // future must retain failures already observed before interruption.
        let failures = self.stopped.get_or_insert_with(Vec::new);
        if let Some(events) = self.events.take() {
            cleanup(events, failures).await;
        }
        failures.retain(|error| !matches!(error, PowerdCleanupError::Interrupted));
        shutdown_result(failures.clone())
    }
}

fn shutdown_result(mut failures: Vec<PowerdCleanupError>) -> Result<(), EventListenerError> {
    // The interruption marker follows every failure observed before cancellation.
    failures.sort_by_key(|error| matches!(error, PowerdCleanupError::Interrupted));
    if failures.is_empty() {
        Ok(())
    } else {
        Err(EventListenerError::Shutdown(failures))
    }
}

pub(crate) async fn drive_listener<T>(
    mut receive: impl Stream<Item = Result<(), PowerdEventError>> + Unpin,
    changed: async_channel::Sender<()>,
    receive_error: &mut Option<PowerdEventError>,
    operation: impl Future<Output = T>,
    cancel_publication: impl FnOnce(),
) -> T {
    let driver = async {
        while let Some(notification) = receive.next().await {
            match notification {
                Ok(()) => match changed.try_send(()) {
                    Ok(()) | Err(TrySendError::Full(())) => {}
                    Err(TrySendError::Closed(())) => return std::future::pending::<T>().await,
                },
                Err(error) => {
                    *receive_error = Some(error);
                    cancel_publication();
                    drop(changed);
                    return std::future::pending::<T>().await;
                }
            }
            // Yield after actual notifications so floods cannot starve cleanup.
            futures_lite::future::yield_now().await;
        }
        *receive_error = Some(PowerdEventError::Closed);
        cancel_publication();
        drop(changed);
        std::future::pending::<T>().await
    };
    futures_lite::future::or(driver, operation).await
}

impl Drop for EventListener {
    fn drop(&mut self) {
        // Dedicated streams own no automatic removal task. No await, join,
        // executor spawn, or retry is introduced by this fallback.
        drop(self.changed.take());
        drop(self.events.take());
    }
}

#[cfg(test)]
mod cancellation_tests;

#[cfg(test)]
mod integration_event_tests;
