//! Temporary stock BLE source ownership around an advertisement burst.
//!
//! An absent backend is required before acquisition. Source 4 is rechecked before
//! the burst, then release is attempted after every acquisition attempt.
//! These readbacks are observations, not an atomic system-wide lease.

use crate::bthome::BthomeSample;
use crate::monitor::{Broadcaster, Publication};
use crate::radio::RadioState;
use std::error::Error;
use std::future::Future;
use thiserror::Error;

/// Observes stock radio ownership and changes only BLE source 4.
///
/// Implementations must retain every other client's source bits. They own IO
/// timing; this trait supplies no cancellation or automatic cleanup.
pub(crate) trait RadioPower {
    /// An ownership inspection or source-change failure.
    type Error: Error;

    /// Reads ownership without activating an absent backend.
    fn state(&mut self) -> impl Future<Output = Result<RadioState, Self::Error>>;
    /// Acquires source 4; failure may still require a release attempt.
    fn enable_source4(&mut self) -> impl Future<Output = Result<(), Self::Error>>;
    /// Releases only source 4, retaining competing sources.
    fn disable_source4(&mut self) -> impl Future<Output = Result<(), Self::Error>>;
}

/// Publishes within a radio lease already acquired by the caller.
///
/// The implementation owns advertisement stop, verification, and identity
/// restoration on success, error, and cancellation before returning.
pub(crate) trait AdvertisementBurst {
    /// An advertisement or identity-restoration failure.
    type Error: Error;

    /// Attempts one burst, returning its publication outcome or cleanup-aware failure.
    fn broadcast(
        &mut self,
        sample: BthomeSample,
    ) -> impl Future<Output = Result<Publication, Self::Error>>;
}

/// An ownership or burst failure, including source-release failures.
#[derive(Debug, Error)]
pub enum OwnedBroadcastError<RE: Error + 'static, AE: Error + 'static> {
    /// Initial ownership inspection failed before acquisition.
    /// Its error is the source.
    #[error("stock BLE source read failed: {0}")]
    Inspect(#[source] RE),
    /// Acquisition or its readback failed; the source-release result is retained.
    #[error("stock BLE source enable failed: {source}; cleanup error: {cleanup_error:?}")]
    Enable {
        /// The acquisition or ownership-readback failure.
        source: RE,
        /// A failure while attempting to release source 4, if any.
        cleanup_error: Option<RE>,
    },
    /// The burst failed; subsequent ownership inspection/release failures are retained.
    #[error("advertisement failed: {source}; radio cleanup errors: {cleanup_errors:?}")]
    Advertisement {
        /// The burst failure, including its advertisement cleanup errors.
        source: AE,
        /// Failures from ownership inspection and source release, in order.
        cleanup_errors: Vec<RE>,
    },
    /// Ownership inspection or source release failed after a non-error burst outcome.
    /// No single error source is selected from the ordered collection.
    #[error("stock BLE cleanup failed: {0:?}")]
    Cleanup(Vec<RE>),
}

/// A broadcaster borrowing the stock ownership and advertisement boundaries.
///
/// Defers when the backend already exists, including a zero source mask.
/// After acquisition it publishes only when the observed mask is exactly 4.
/// It then inspects ownership and attempts to release only source 4, even when
/// the burst or inspection fails. It cannot exclude races with other clients.
/// After acquisition is attempted, drive the publication future to completion;
/// dropping it does not release source 4 or finish advertisement cleanup.
pub(crate) struct OwnedBroadcaster<'a, R, A> {
    radio: &'a mut R,
    advertisement: &'a mut A,
}

impl<'a, R, A> OwnedBroadcaster<'a, R, A> {
    /// Borrows both boundaries without acquiring or configuring the radio.
    pub(crate) fn new(radio: &'a mut R, advertisement: &'a mut A) -> Self {
        Self {
            radio,
            advertisement,
        }
    }
}

impl<R: RadioPower, A: AdvertisementBurst> Broadcaster for OwnedBroadcaster<'_, R, A>
where
    R::Error: 'static,
    A::Error: 'static,
{
    type Error = OwnedBroadcastError<R::Error, A::Error>;

    async fn broadcast(&mut self, sample: BthomeSample) -> Result<Publication, Self::Error> {
        let initial = self
            .radio
            .state()
            .await
            .map_err(OwnedBroadcastError::Inspect)?;
        match initial {
            RadioState::DaemonAbsent => {}
            RadioState::EnabledSources(mask) => {
                tracing::info!(event = "radio_in_use", mask);
                return Ok(Publication::Deferred);
            }
        }
        if let Err(source) = self.radio.enable_source4().await {
            return Err(OwnedBroadcastError::Enable {
                source,
                cleanup_error: self.radio.disable_source4().await.err(),
            });
        }
        // Recheck after acquisition: never configure identity on a shared radio.
        let acquired = self.radio.state().await;
        let advertisement = match acquired {
            Ok(RadioState::EnabledSources(4)) => self.advertisement.broadcast(sample).await,
            Ok(RadioState::DaemonAbsent | RadioState::EnabledSources(_)) => {
                Ok(Publication::Deferred)
            }
            Err(source) => {
                return Err(OwnedBroadcastError::Enable {
                    source,
                    cleanup_error: self.radio.disable_source4().await.err(),
                });
            }
        };
        let mut cleanup_errors = Vec::new();
        // Observe competing owners, but release only the source we acquired.
        // Inspection failure must not skip the bounded source-release attempt.
        match self.radio.state().await {
            Ok(state) => tracing::info!(event = "radio_cleanup_ownership", ?state),
            Err(error) => cleanup_errors.push(error),
        }
        if let Err(error) = self.radio.disable_source4().await {
            cleanup_errors.push(error);
        }
        match advertisement {
            Err(source) => Err(OwnedBroadcastError::Advertisement {
                source,
                cleanup_errors,
            }),
            Ok(outcome) => {
                if cleanup_errors.is_empty() {
                    Ok(outcome)
                } else {
                    Err(OwnedBroadcastError::Cleanup(cleanup_errors))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
