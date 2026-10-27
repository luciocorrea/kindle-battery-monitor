//! Safe radio operations over the firmware-specific BSA request boundary.
//!
//! [`SocketDaemon`] owns socket sessions; [`BsaRadio`] maps radio operations to
//! configuration requests. Neither acquires stock BLE source ownership, which
//! belongs to [`crate::ownership`].

use crate::broadcast::BroadcastRadio;
use crate::bsa::{BsaClient, BsaError};
use crate::bthome::BthomeSample;
use crate::config::{
    advertising_data, privacy, visibility, AddressPolicy, AdvertisingState, ConfigError,
    DeviceConfig,
};
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

/// Executes synchronous, bounded BSA request/reply exchanges.
///
/// Implementations must reject replies that do not match the requested ID and
/// size and avoid reusing a connection after a failed exchange.
pub trait RequestExecutor {
    /// Sends one request and returns exactly `reply_size` payload bytes or a transport error.
    fn execute(&mut self, id: u16, payload: &[u8], reply_size: usize) -> Result<Vec<u8>, BsaError>;
}

/// A lazily connected BSA session reused only after successful exchanges.
///
/// A failed exchange drops its client so cleanup can attempt a fresh connection.
pub struct SocketDaemon {
    path: PathBuf,
    timeout: Duration,
    client: Option<BsaClient>,
}

impl SocketDaemon {
    /// Stores the socket path and per-exchange timeout without connecting.
    ///
    /// Timeout validation is deferred to [`BsaClient::connect`] on the first request.
    pub fn new(path: PathBuf, timeout: Duration) -> Self {
        Self {
            path,
            timeout,
            client: None,
        }
    }
}

impl RequestExecutor for SocketDaemon {
    fn execute(&mut self, id: u16, payload: &[u8], reply_size: usize) -> Result<Vec<u8>, BsaError> {
        let mut client = match self.client.take() {
            Some(client) => client,
            None => BsaClient::connect(&self.path, self.timeout)?,
        };
        let result = client.request(id, payload, reply_size);
        // Preserve the vendor client's session lifecycle, but never reuse a
        // partial response for cleanup after a failed exchange.
        if result.is_ok() {
            self.client = Some(client);
        }
        result
    }
}

/// A transport, configuration, or radio-state failure.
#[derive(Debug, Error)]
pub enum BsaRadioError {
    /// The bounded socket exchange failed.
    #[error(transparent)]
    Transport(#[from] BsaError),
    /// The firmware reply was malformed or reported an operation failure.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// Preflight found the radio disabled.
    #[error("Bluetooth is not enabled")]
    Disabled,
    /// Preflight found advertising already active, so the burst must defer.
    #[error("an existing legacy advertisement prevents an exclusive test")]
    ExistingAdvertisement,
    /// Cleanup readback still showed active advertising.
    #[error("legacy advertising remains enabled after cleanup")]
    StillAdvertising,
}

/// A safe advertisement adapter for the qualified firmware.
///
/// Preflight refuses a disabled radio or existing advertisement. Configuration
/// requests the public identity and named BTHome data; cleanup requests stock
/// rotation. The adapter neither acquires source 4 nor provides automatic cleanup
/// on drop. Use [`crate::broadcast::run_broadcast`] within an owned radio lease.
pub struct BsaRadio<D> {
    daemon: D,
}

impl<D: RequestExecutor> BsaRadio<D> {
    /// Takes ownership of the request executor without performing IO.
    pub fn new(daemon: D) -> Self {
        Self { daemon }
    }

    /// Reads and validates initialized configuration flags via BSA request 100.
    ///
    /// # Errors
    ///
    /// Returns transport errors, nonzero status, or invalid reply lengths/flags.
    pub fn read_config(&mut self) -> Result<DeviceConfig, BsaRadioError> {
        Ok(DeviceConfig::from_reply(&self.daemon.execute(
            100,
            &[0],
            836,
        )?)?)
    }

    fn set_config(&mut self, config: &[u8]) -> Result<(), BsaRadioError> {
        let reply = self.daemon.execute(101, config, 2)?;
        let [low, high] = reply.as_slice() else {
            return Err(ConfigError::Length {
                actual: reply.len(),
                expected: 2,
            }
            .into());
        };
        let status = u16::from_le_bytes([*low, *high]);
        if status != 0 {
            return Err(ConfigError::Status(status).into());
        }
        Ok(())
    }
}

impl<D: RequestExecutor> BroadcastRadio for BsaRadio<D> {
    type Error = BsaRadioError;

    fn ensure_available(&mut self) -> Result<(), Self::Error> {
        let config = self.read_config()?;
        tracing::info!(event = "bsa_preflight", enabled = config.radio_enabled(), advertising = ?config.advertising_state());
        if !config.radio_enabled() {
            return Err(BsaRadioError::Disabled);
        }
        match config.advertising_state() {
            AdvertisingState::Inactive => Ok(()),
            AdvertisingState::Active => Err(BsaRadioError::ExistingAdvertisement),
        }
    }

    fn configure(&mut self, sample: BthomeSample) -> Result<(), Self::Error> {
        self.set_config(&privacy(AddressPolicy::Public))?;
        tracing::info!(event = "public_identity_requested");
        self.set_config(&advertising_data(sample))
    }

    fn start(&mut self) -> Result<(), Self::Error> {
        self.set_config(&visibility(AdvertisingState::Active))?;
        tracing::info!(event = "advertising_start_acknowledged");
        Ok(())
    }

    fn stop(&mut self) -> Result<(), Self::Error> {
        self.set_config(&visibility(AdvertisingState::Inactive))?;
        tracing::info!(event = "advertising_stop_acknowledged");
        Ok(())
    }

    fn verify_stopped(&mut self) -> Result<(), Self::Error> {
        match self.read_config()?.advertising_state() {
            AdvertisingState::Inactive => {
                tracing::info!(event = "advertising_stop_verified");
                Ok(())
            }
            AdvertisingState::Active => Err(BsaRadioError::StillAdvertising),
        }
    }

    fn restore_identity(&mut self) -> Result<(), Self::Error> {
        self.set_config(&privacy(AddressPolicy::Rotating))?;
        tracing::info!(event = "rotating_identity_restoration_requested");
        Ok(())
    }
}
