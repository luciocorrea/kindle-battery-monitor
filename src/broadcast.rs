//! Advertisement sequencing within an already acquired radio lease.
//!
//! Preflight precedes all mutation. Once configuration begins, every result path
//! attempts stop, stopped-state readback, then stock identity restoration.
//! [`crate::ownership`] manages acquisition and release of BLE source 4.

use crate::bthome::BthomeSample;
use std::error::Error;
use thiserror::Error;

/// Why the burst wait ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndReason {
    /// The configured wait elapsed; hardware or scheduler delays can extend RF time.
    Deadline,
    /// Cancellation ended the wait early.
    Cancelled,
}

/// A consuming, blocking wait for a burst deadline or cancellation.
///
/// Implementations must use events or timers and permit normal suspend.
pub trait WaitForEnd {
    /// Blocks until the deadline or cancellation becomes ready.
    fn wait(self) -> EndReason;
}

/// Radio operations needed for one owned advertisement.
///
/// The caller must hold the temporary radio lease before using this boundary.
/// Implementations must bound device IO and preserve unrelated client state.
/// Each cleanup operation must remain callable after an earlier operation fails.
pub trait BroadcastRadio {
    /// A transport, state, or firmware-protocol failure.
    type Error: Error;

    /// Checks availability without mutation; an existing advertisement must be refused.
    fn ensure_available(&mut self) -> Result<(), Self::Error>;
    /// Requests the owned burst identity and payload; failure may leave partial state.
    fn configure(&mut self, sample: BthomeSample) -> Result<(), Self::Error>;
    /// Requests advertising start and waits for the implementation's acknowledgement.
    fn start(&mut self) -> Result<(), Self::Error>;
    /// Requests advertising stop, including after partial configuration or start failure.
    fn stop(&mut self) -> Result<(), Self::Error>;
    /// Reads back advertising state and fails if advertising remains active.
    fn verify_stopped(&mut self) -> Result<(), Self::Error>;
    /// Requests the stock rotating identity policy after the stop/readback attempts.
    fn restore_identity(&mut self) -> Result<(), Self::Error>;
}

/// A burst failure with any cleanup failures retained in operation order.
#[derive(Debug, Error)]
pub enum BroadcastError<E: Error> {
    /// Preflight failed; no configuration or cleanup was attempted.
    #[error("radio preflight failed: {0}")]
    Unavailable(E),
    /// Configuration failed; cleanup was attempted and its failures are retained.
    #[error("advertisement configuration failed: {source}; cleanup errors: {cleanup_errors:?}")]
    Configure {
        /// The configuration failure.
        source: E,
        /// Failures from stop, readback, then identity restoration, in order.
        cleanup_errors: Vec<E>,
    },
    /// Starting failed; cleanup was attempted and its failures are retained.
    #[error("advertisement start failed: {source}; cleanup errors: {cleanup_errors:?}")]
    Start {
        /// The start failure.
        source: E,
        /// Failures from stop, readback, then identity restoration, in order.
        cleanup_errors: Vec<E>,
    },
    /// The wait ended, but stop, verification, or identity restoration failed.
    #[error("advertisement cleanup failed: {0:?}")]
    Cleanup(Vec<E>),
}

fn cleanup<R: BroadcastRadio>(radio: &mut R) -> Vec<R::Error> {
    let mut errors = Vec::new();
    if let Err(error) = radio.stop() {
        errors.push(error);
    }
    if let Err(error) = radio.verify_stopped() {
        errors.push(error);
    }
    if let Err(error) = radio.restore_identity() {
        errors.push(error);
    }
    errors
}

/// Runs preflight, configuration, start, wait, and cleanup synchronously.
///
/// After configuration begins, stop, stopped-state verification, and identity
/// restoration are all attempted in that order even if earlier steps fail.
/// A preflight failure returns without cleanup to avoid touching another owner.
/// Cancellation is observed by `deadline.wait()`, after configuration and start;
/// it does not interrupt those operations. Dropping or unwinding this call is not
/// a cleanup mechanism. Source acquisition and release belong to the caller.
///
/// # Errors
///
/// Returns the primary failure and collected cleanup failures. An end reason is
/// returned only when cleanup succeeded.
pub fn run_broadcast<R: BroadcastRadio>(
    radio: &mut R,
    sample: BthomeSample,
    deadline: impl WaitForEnd,
) -> Result<EndReason, BroadcastError<R::Error>> {
    radio
        .ensure_available()
        .map_err(BroadcastError::Unavailable)?;
    if let Err(source) = radio.configure(sample) {
        return Err(BroadcastError::Configure {
            source,
            cleanup_errors: cleanup(radio),
        });
    }
    if let Err(source) = radio.start() {
        return Err(BroadcastError::Start {
            source,
            cleanup_errors: cleanup(radio),
        });
    }
    let reason = deadline.wait();
    let errors = cleanup(radio);
    if errors.is_empty() {
        Ok(reason)
    } else {
        Err(BroadcastError::Cleanup(errors))
    }
}
