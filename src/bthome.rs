//! Validated battery percentages and unencrypted, trigger-based BTHome v2 bytes.
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

/// One battery percentage and packet ID for a BTHome advertisement.
///
/// The caller assigns packet IDs; constructing a sample does not advance them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BthomeSample {
    level: BatteryLevel,
    packet_id: u8,
}

impl BthomeSample {
    /// Constructs a sample from a validated level and caller-owned packet ID.
    pub const fn new(level: BatteryLevel, packet_id: u8) -> Self {
        Self { level, packet_id }
    }

    /// BTHome service data, including the little-endian service UUID.
    ///
    /// Returns `D2 FC 44 00 <packet-id> 01 <battery-percent>`. The `0x44`
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
    /// # Ok(())
    /// # }
    /// ```
    pub const fn service_data(self) -> [u8; 7] {
        [0xd2, 0xfc, 0x44, 0x00, self.packet_id, 0x01, self.level.0]
    }
}
