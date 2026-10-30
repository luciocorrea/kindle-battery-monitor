//! Battery percentages, optional charging state and trigger-based BTHome v2 bytes.
//!
//! Encoding is pure and performs no radio IO. Advertisement identity and the fixed
//! local name are configured internally according to `docs/protocol.md`.

use thiserror::Error;

/// A percentage in `0..=100`, validated through `TryFrom<i32>`.
///
/// Invalid input returns [`InvalidBatteryLevel`] rather than clamping it.
///
/// # Examples
///
/// Both endpoints are valid; rejected values remain available for diagnostics.
///
/// ```
/// use kindle_battery_monitor::bthome::{BatteryLevel, InvalidBatteryLevel};
///
/// assert!(BatteryLevel::try_from(0).is_ok());
/// assert!(BatteryLevel::try_from(100).is_ok());
/// assert_eq!(
///     BatteryLevel::try_from(-1),
///     Err(InvalidBatteryLevel { value: -1 }),
/// );
/// assert_eq!(
///     BatteryLevel::try_from(101),
///     Err(InvalidBatteryLevel { value: 101 }),
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatteryLevel(u8);

/// A reported percentage outside `0..=100`.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("battery percentage must be 0..=100, received {value}")]
pub struct InvalidBatteryLevel {
    /// The original rejected value, retained for diagnostics.
    pub value: i32,
}

impl TryFrom<i32> for BatteryLevel {
    type Error = InvalidBatteryLevel;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        if value > 100 {
            return Err(InvalidBatteryLevel { value });
        }
        u8::try_from(value)
            .map(Self)
            .map_err(|_| InvalidBatteryLevel { value })
    }
}

/// One battery percentage, optional charging state and packet ID for an advertisement.
///
/// The caller assigns packet IDs; constructing a sample does not advance them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BthomeSample {
    level: BatteryLevel,
    packet_id: u8,
    charging: Option<bool>,
}

impl BthomeSample {
    /// Constructs a battery-only sample from a validated level and caller-owned packet ID.
    ///
    /// Add a known charging observation with [`Self::with_charging`].
    pub const fn new(level: BatteryLevel, packet_id: u8) -> Self {
        Self {
            level,
            packet_id,
            charging: None,
        }
    }

    /// Adds a freshly observed charging state as BTHome binary object `0x16`.
    ///
    /// Use a new sample without this field when charging is unknown. Omitting
    /// it does not clear a receiver's previously received charging state.
    pub const fn with_charging(mut self, charging: bool) -> Self {
        self.charging = Some(charging);
        self
    }

    /// BTHome service data, including the little-endian service UUID.
    ///
    /// Returns `D2 FC 44 00 <packet-id> 01 <battery-percent>`, optionally followed
    /// by `16 00` for not charging or `16 01` for charging. The `0x44`
    /// device byte selects unencrypted BTHome v2 with trigger-based behavior;
    /// the result excludes the BLE advertising element's length and type.
    ///
    /// # Examples
    ///
    /// Encoding a synthetic sample performs no radio IO. These bytes describe the
    /// service data only; they do not establish advertisement transmission or receipt.
    ///
    /// ```
    /// use kindle_battery_monitor::bthome::{BatteryLevel, BthomeSample};
    /// # fn main() -> Result<(), kindle_battery_monitor::bthome::InvalidBatteryLevel> {
    ///
    /// let sample = BthomeSample::new(BatteryLevel::try_from(52)?, 42);
    /// assert_eq!(sample.service_data(), [0xd2, 0xfc, 0x44, 0x00, 42, 0x01, 52]);
    /// assert_eq!(sample.with_charging(true).service_data(),
    ///     [0xd2, 0xfc, 0x44, 0x00, 42, 0x01, 52, 0x16, 1]);
    /// # Ok(())
    /// # }
    /// ```
    pub fn service_data(self) -> Vec<u8> {
        let mut bytes = vec![0xd2, 0xfc, 0x44, 0x00, self.packet_id, 0x01, self.level.0];
        if let Some(charging) = self.charging {
            bytes.extend_from_slice(&[0x16, u8::from(charging)]);
        }
        bytes
    }
}
