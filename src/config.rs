//! Pure BSA configuration encoding and readback for the qualified firmware.
//!
//! Offsets and masks are specific to Paperwhite 4 firmware 5.16.5. Encoders change
//! only selected fields in zeroed requests; encoding does not perform device IO.
//! See the repository's `docs/protocol.md` for the payload and identity contract.

use crate::bthome::BthomeSample;
use thiserror::Error;

/// The qualified firmware's 832-byte set-config payload size, excluding reply status.
const CONFIG_BYTES: usize = 0x340;

/// The stock local-privacy policy requested for the owned burst or its cleanup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AddressPolicy {
    /// Request the factory public Bluetooth address for receiver identity stability.
    Public,
    /// Request the stock rotating-address policy after the burst.
    Rotating,
}

/// Stock BluetoothHalDmSetBleLocalPrivacy request on the qualified firmware.
///
/// Selects only local privacy. Callers must own the burst before requesting
/// [`AddressPolicy::Public`] and request [`AddressPolicy::Rotating`] during cleanup.
pub(crate) fn privacy(policy: AddressPolicy) -> [u8; CONFIG_BYTES] {
    let mut request = [0; CONFIG_BYTES];
    request[..4].copy_from_slice(&0x20000_u32.to_le_bytes());
    request[4] = 1;
    request[0x203] = match policy {
        AddressPolicy::Public => 0,
        AddressPolicy::Rotating => 1,
    };
    request
}

/// Encode this Kindle's BSA advertising-data request, excluding unrelated fields.
///
/// Selects primary advertisement flags, BTHome service data, and the explicit
/// Complete Local Name `Kindle`. It leaves the global Bluetooth name untouched
/// and does not configure a scan response.
pub(crate) fn advertising_data(sample: BthomeSample) -> [u8; CONFIG_BYTES] {
    let mut request = [0; CONFIG_BYTES];
    request[..4].copy_from_slice(&0x8000_u32.to_le_bytes());
    request[4] = 1;
    request[0x219] = 0x06;
    request[0x23c..0x240].copy_from_slice(&0x8102_u32.to_le_bytes());
    request[0x254..0x256].copy_from_slice(&2_u16.to_le_bytes());
    let service_data = sample.service_data();
    // The pure sample encoder returns seven or nine bytes, including the UUID.
    let data_length = service_data.len() - 2;
    request[0x250] = data_length as u8;
    request[0x258..0x25a].copy_from_slice(&service_data[..2]);
    request[0x268..0x268 + data_length].copy_from_slice(&service_data[2..]);
    // One explicit Complete Local Name element, after the BTHome service data.
    request[0x2b0] = 1;
    request[0x2b1] = 0x09;
    request[0x2b2] = 6;
    request[0x2b3..0x2b9].copy_from_slice(b"Kindle");
    request
}

/// Whether legacy BLE advertising is inactive or active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AdvertisingState {
    /// Neither BLE discoverability nor BLE connectability is enabled.
    Inactive,
    /// At least one advertising-related BLE flag is enabled.
    Active,
}

/// Encodes the legacy BLE discoverability request for starting or stopping a burst.
///
/// This request selects the discoverability field; unrelated fields remain unselected.
pub(crate) fn visibility(state: AdvertisingState) -> [u8; CONFIG_BYTES] {
    let mut request = [0; CONFIG_BYTES];
    request[..4].copy_from_slice(&0x2000_u32.to_le_bytes());
    request[4] = 1;
    request[0x201] = match state {
        AdvertisingState::Inactive => 0,
        AdvertisingState::Active => 1,
    };
    request
}

/// Validated initialized radio flags from a get-config reply.
///
/// Identity data and uninitialized reply bytes are deliberately excluded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DeviceConfig {
    enabled: bool,
    discoverable: bool,
    connectable: bool,
    ble_discoverable: bool,
    ble_connectable: bool,
}

/// A malformed configuration reply or nonzero BSA operation status.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The reply length differs from the required wire size.
    #[error("BSA configuration reply length {actual}, expected {expected}")]
    Length {
        /// The received payload length.
        actual: usize,
        /// The required payload length.
        expected: usize,
    },
    /// The daemon returned a nonzero little-endian operation status.
    #[error("BSA operation failed with status {0}")]
    Status(u16),
    /// A flag contained a value other than zero or one.
    #[error("BSA configuration field {field} has unsupported value {value}")]
    Field {
        /// The decoded flag name.
        field: &'static str,
        /// The unsupported raw flag byte.
        value: u8,
    },
}

impl DeviceConfig {
    /// Decode only fields initialized by this target's get-config handler.
    /// The remaining reply contains identity data and uninitialized bytes.
    ///
    /// # Errors
    ///
    /// Rejects a length other than 836 bytes, nonzero status, or decoded flags
    /// whose values are not zero or one. Performs no IO and logs no reply bytes.
    pub(crate) fn from_reply(reply: &[u8]) -> Result<Self, ConfigError> {
        if reply.len() != CONFIG_BYTES + 4 {
            return Err(ConfigError::Length {
                actual: reply.len(),
                expected: CONFIG_BYTES + 4,
            });
        }
        let status = u16::from_le_bytes([reply[0], reply[1]]);
        if status != 0 {
            return Err(ConfigError::Status(status));
        }
        Ok(Self {
            enabled: flag("enabled", reply[8])?,
            discoverable: flag("discoverable", reply[9])?,
            connectable: flag("connectable", reply[10])?,
            ble_discoverable: flag("ble_discoverable", reply[4 + 0x201])?,
            ble_connectable: flag("ble_connectable", reply[4 + 0x202])?,
        })
    }

    /// Treats either BLE discoverability or BLE connectability as active advertising.
    pub(crate) const fn advertising_state(self) -> AdvertisingState {
        if self.ble_discoverable || self.ble_connectable {
            AdvertisingState::Active
        } else {
            AdvertisingState::Inactive
        }
    }

    /// Returns the decoded radio-enabled flag without performing IO.
    pub(crate) const fn radio_enabled(self) -> bool {
        self.enabled
    }
}

fn flag(field: &'static str, value: u8) -> Result<bool, ConfigError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        value => Err(ConfigError::Field { field, value }),
    }
}

#[cfg(test)]
mod tests;
