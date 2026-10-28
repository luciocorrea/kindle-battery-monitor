//! Sampling and publication policy independent of device IO.
//!
//! Only successful publications advance the wrapping packet ID. The continuous
//! runner logs invalid or failed battery reads and waits for the next trigger;
//! publication and event-source errors terminate the session.

use crate::bthome::{BatteryLevel, BthomeSample, InvalidBatteryLevel};
use std::error::Error;
use std::future::Future;
use thiserror::Error;

/// Reads the reported battery percentage on demand.
///
/// Implementations own any IO deadlines. Range validation belongs to [`Monitor`].
pub(crate) trait BatterySource {
    /// A failure to read the reported percentage.
    type Error: Error;

    /// Reads a fresh reported percentage, leaving range checks to the caller.
    fn read_percentage(&mut self) -> impl Future<Output = Result<i32, Self::Error>>;
}

/// Publishes one sample and completes its owned cleanup before returning.
///
/// Implementations must preserve other radio clients and return publication or
/// cleanup failures as errors. The monitor stops on either failure.
/// Once publication IO starts, drive its future to completion; dropping it does
/// not perform the owned cleanup.
pub(crate) trait Broadcaster {
    /// A publication or owned-cleanup failure.
    type Error: Error;

    /// Attempts one sample and completes owned cleanup before returning an outcome.
    fn broadcast(
        &mut self,
        sample: BthomeSample,
    ) -> impl Future<Output = Result<Publication, Self::Error>>;
}

/// The result of a publication attempt after its cleanup has completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Publication {
    /// The burst completed successfully; this does not prove receiver delivery.
    Published,
    /// The sample was not published, for example because another client owns the radio.
    Deferred,
    /// Cancellation ended the attempt; the monitor should stop.
    Cancelled,
}

/// Supplies the next sampling or shutdown trigger.
///
/// Implementations must await events, ordinary deadline timers, or cancellation
/// rather than poll or prevent suspend.
pub(crate) trait EventSource {
    /// A failure to obtain the next trigger.
    type Error: Error;

    /// Waits for one trigger or reports a terminal event-source failure.
    fn next(&mut self) -> impl Future<Output = Result<Trigger, Self::Error>>;
}

/// A session event interpreted by [`Monitor::handle`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Trigger {
    /// Take the initial sample for a new session.
    Started,
    /// Resample after a battery-change notification.
    BatteryChanged,
    /// Resample after a periodic refresh timer becomes ready.
    Refresh,
    /// Stop without reading the battery or touching the broadcaster.
    Shutdown,
}

/// Whether handling a trigger published, deferred, or stopped the monitor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The burst completed successfully; this does not prove receiver delivery.
    Published,
    /// The sample was not published, for example because another client owns the radio.
    Deferred,
    /// Shutdown or publication cancellation ended the session.
    Stopped,
}

/// A single sample failed to read, validate, or publish.
#[derive(Debug, Error)]
pub enum MonitorError<BE: Error + 'static, PE: Error + 'static> {
    /// The battery source failed before publication began; its error is the source.
    #[error("battery read failed: {0}")]
    Battery(#[source] BE),
    /// The source returned a percentage outside the valid range.
    #[error(transparent)]
    InvalidBattery(#[from] InvalidBatteryLevel),
    /// Publication or its required cleanup failed; its error is the source.
    #[error("battery broadcast failed: {0}")]
    Broadcast(#[source] PE),
}

/// A terminal failure of the continuous monitor loop.
#[derive(Debug, Error)]
pub enum RunError<E: Error + 'static, PE: Error + 'static> {
    /// The next trigger could not be obtained; its error is the source.
    #[error("monitor event source failed: {0}")]
    Event(#[source] E),
    /// A burst or its cleanup failed; its error is the source.
    /// The session must stop for inspection.
    #[error("battery publication failed: {0}")]
    Publication(#[source] PE),
}

/// Owns the next packet ID and applies sampling policy.
///
/// Device dependencies are borrowed only while handling a trigger.
pub(crate) struct Monitor {
    next_packet_id: u8,
}

impl Monitor {
    /// Starts a packet sequence at the caller-selected ID.
    pub(crate) const fn new(first_packet_id: u8) -> Self {
        Self {
            next_packet_id: first_packet_id,
        }
    }

    /// Handles one trigger, awaiting battery IO and publication cleanup.
    ///
    /// Shutdown performs no IO. Other triggers read and validate a fresh percentage.
    /// The packet ID advances with wrapping arithmetic only on
    /// [`Publication::Published`]; deferred, cancelled, or failed attempts retain it.
    ///
    /// # Errors
    ///
    /// Returns battery IO, invalid-percentage, or publication errors to the caller.
    /// This method does not retry or log battery failures itself.
    pub(crate) async fn handle<B: BatterySource, P: Broadcaster>(
        &mut self,
        trigger: Trigger,
        battery: &mut B,
        broadcaster: &mut P,
    ) -> Result<Outcome, MonitorError<B::Error, P::Error>>
    where
        B::Error: 'static,
        P::Error: 'static,
    {
        match trigger {
            Trigger::Shutdown => Ok(Outcome::Stopped),
            Trigger::Started | Trigger::BatteryChanged | Trigger::Refresh => {
                let percentage = battery
                    .read_percentage()
                    .await
                    .map_err(MonitorError::Battery)?;
                let level = BatteryLevel::try_from(percentage)?;
                let sample = BthomeSample::new(level, self.next_packet_id);
                match broadcaster
                    .broadcast(sample)
                    .await
                    .map_err(MonitorError::Broadcast)?
                {
                    Publication::Published => {
                        self.next_packet_id = self.next_packet_id.wrapping_add(1);
                        Ok(Outcome::Published)
                    }
                    Publication::Cancelled => Ok(Outcome::Stopped),
                    Publication::Deferred => {
                        tracing::info!(event = "publication_deferred");
                        Ok(Outcome::Deferred)
                    }
                }
            }
        }
    }
}

/// Publishes at startup, then awaits triggers until shutdown or cancellation.
///
/// Failed battery reads and invalid percentages are logged and retried only when
/// another trigger arrives. Deferred attempts also wait for the next trigger.
/// The event source and broadcaster supply waiting and cancellation behavior;
/// this loop adds no polling, suspend inhibitor, or wake alarm.
///
/// # Errors
///
/// Event-source and publication errors terminate the loop. Cleanup is the
/// broadcaster's responsibility before its future completes. Request cancellation
/// through the event source or broadcaster, and continue polling this future until
/// completion rather than dropping an in-flight publication.
pub(crate) async fn run_monitor<B, P, E>(
    monitor: &mut Monitor,
    battery: &mut B,
    broadcaster: &mut P,
    events: &mut E,
) -> Result<(), RunError<E::Error, P::Error>>
where
    B: BatterySource,
    P: Broadcaster,
    E: EventSource,
    B::Error: 'static,
    P::Error: 'static,
    E::Error: 'static,
{
    match monitor.handle(Trigger::Started, battery, broadcaster).await {
        Ok(Outcome::Published | Outcome::Deferred) => {}
        Ok(Outcome::Stopped) => return Ok(()),
        Err(MonitorError::Broadcast(error)) => return Err(RunError::Publication(error)),
        Err(error) => {
            tracing::warn!(event = "battery_sample_failed", trigger = ?Trigger::Started, error = %error);
        }
    }
    loop {
        let trigger = events.next().await.map_err(RunError::Event)?;
        match monitor.handle(trigger, battery, broadcaster).await {
            Ok(Outcome::Stopped) => return Ok(()),
            Ok(Outcome::Published | Outcome::Deferred) => {}
            Err(MonitorError::Broadcast(error)) => return Err(RunError::Publication(error)),
            Err(error) => {
                tracing::warn!(event = "battery_sample_failed", trigger = ?trigger, error = %error);
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod loop_tests;
