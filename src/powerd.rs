//! Synchronous powerd battery reads through the bundled LIPC client.
//!
//! Reads the integer `battLevel` property. Range validation belongs to
//! [`crate::bthome::BatteryLevel`]; this adapter returns the reported integer unchanged.

use crate::monitor::BatterySource;
use kindle_lipc_dbus::{LipcDbusClient, LipcDbusError, LipcMethodTransport, ZbusTransport};

const SERVICE: &str = "com.lab126.powerd";
const PROPERTY: &str = "battLevel";

/// A battery source owning a blocking LIPC transport.
///
/// Read failures retain LIPC/DBus errors and do not fabricate a percentage.
pub struct PowerdBattery<T = ZbusTransport> {
    client: LipcDbusClient<T>,
}

impl PowerdBattery<ZbusTransport> {
    /// Connects a blocking system-bus client without reading the battery.
    ///
    /// # Errors
    ///
    /// Returns a DBus connection failure. No application-level cancellation or
    /// overall timeout is added here.
    pub fn system() -> Result<Self, LipcDbusError> {
        Ok(Self::new(LipcDbusClient::system()?))
    }
}

impl<T> PowerdBattery<T> {
    /// Takes ownership of an injected LIPC client without reading any property.
    pub fn new(client: LipcDbusClient<T>) -> Self {
        Self { client }
    }
}

impl<T: LipcMethodTransport> BatterySource for PowerdBattery<T> {
    type Error = LipcDbusError;

    fn read_percentage(&mut self) -> Result<i32, Self::Error> {
        self.client.get_int_property(SERVICE, PROPERTY)
    }
}
