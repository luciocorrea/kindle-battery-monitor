//! Async powerd battery reads, with an injected blocking LIPC adapter in tests.
//!
//! Reads integer `battLevel` and optional `isCharging` properties. Percentage
//! validation belongs to [`crate::bthome::BatteryLevel`]. Charging accepts only
//! zero or one; other values and read failures are logged and omitted. Pending
//! reads cooperate with cancellation, which remains an error rather than an
//! unknown charging observation. See `docs/protocol.md` for sampling policy.

use crate::monitor::BatterySource;
use kindle_lipc_dbus::{
    property_method_name, LipcDbusError, LipcEndpoint, LipcStatus, PropertyAccess, PropertyType,
};
#[cfg(test)]
use kindle_lipc_dbus::{LipcDbusClient, LipcMethodTransport};
use thiserror::Error;

const SERVICE: &str = "com.lab126.powerd";
const PROPERTY: &str = "battLevel";

/// An asynchronous powerd connection, property-call, decoding, or LIPC failure.
#[derive(Debug, Error)]
pub enum PowerdReadError {
    /// A queued cancellation or disconnected cancellation channel ended the read.
    #[error("powerd battery read cancelled")]
    Cancelled,
    /// DBus failed while performing the named operation.
    #[error("powerd DBus operation failed: {operation}: {source}")]
    Dbus {
        /// The connection, call, or decoding operation that failed.
        operation: &'static str,
        /// The underlying DBus transport or decoding cause.
        #[source]
        source: zbus::Error,
    },
    /// The property service returned a nonzero LIPC status, including unknown codes.
    #[error(transparent)]
    Lipc(#[from] LipcDbusError),
}

/// An asynchronous production battery source owning its DBus connection.
///
/// Pending connection and read operations yield to other futures without a
/// blocking transport or offload worker. The caller owns any overall deadline
/// and may cancel a pending read by dropping its future. Cancellation does not
/// require a compensating write because property reads do not mutate powerd.
pub(crate) struct SystemPowerdBattery {
    connection: zbus::Connection,
    cancelled: Option<async_channel::Receiver<()>>,
}

impl SystemPowerdBattery {
    /// Connects asynchronously to the system bus without reading the battery.
    ///
    /// No application-level overall deadline is added; the caller owns startup
    /// timeout and cancellation policy.
    ///
    /// # Errors
    ///
    /// Returns a connection failure with its DBus cause retained.
    pub(crate) async fn system() -> Result<Self, PowerdReadError> {
        let connection =
            zbus::Connection::system()
                .await
                .map_err(|source| PowerdReadError::Dbus {
                    operation: "connect to system bus for battery reads",
                    source,
                })?;
        Ok(Self::from_connection(connection))
    }

    /// Owns an existing async connection without issuing a property call.
    ///
    /// This supports private-bus tests as well as callers that already own a
    /// connection. Dropping this adapter releases its connection handle.
    fn from_connection(connection: zbus::Connection) -> Self {
        Self {
            connection,
            cancelled: None,
        }
    }

    /// Enables cancellation of subsequent reads using the supplied receiver.
    ///
    /// A queued message or channel disconnection takes priority over the DBus
    /// call. Cancellation drops the pending read future and returns
    /// [`PowerdReadError::Cancelled`] without fabricating a value or performing
    /// a compensating write. A call already sent may still finish remotely.
    /// Constructors leave cancellation disabled until this builder is used.
    pub(crate) fn with_cancellation(mut self, cancelled: async_channel::Receiver<()>) -> Self {
        self.cancelled = Some(cancelled);
        self
    }
}

impl SystemPowerdBattery {
    async fn read_uncancelled(&self, property: &str) -> Result<i32, PowerdReadError> {
        let endpoint = LipcEndpoint::for_service(SERVICE);
        let method = property_method_name(PropertyAccess::Get, property, PropertyType::Int);
        let (read_context, decode_context) = if property == PROPERTY {
            (
                "read powerd battery integer",
                "decode powerd battery integer",
            )
        } else {
            (
                "read powerd charging integer",
                "decode powerd charging integer",
            )
        };
        let reply = self
            .connection
            .call_method(
                Some(endpoint.service),
                endpoint.path,
                Some(endpoint.interface),
                method.as_str(),
                &(),
            )
            .await
            .map_err(|source| PowerdReadError::Dbus {
                operation: read_context,
                source,
            })?;
        let (status, value): (u32, i32) =
            reply
                .body()
                .deserialize()
                .map_err(|source| PowerdReadError::Dbus {
                    operation: decode_context,
                    source,
                })?;
        match LipcStatus::from_wire_code(status) {
            LipcStatus::Ok => Ok(value),
            status => Err(LipcDbusError::Status(status).into()),
        }
    }

    async fn read_property(&self, property: &str) -> Result<i32, PowerdReadError> {
        match &self.cancelled {
            Some(cancelled) => {
                futures_lite::future::or(
                    async {
                        let _ = cancelled.recv().await;
                        Err(PowerdReadError::Cancelled)
                    },
                    self.read_uncancelled(property),
                )
                .await
            }
            None => self.read_uncancelled(property).await,
        }
    }
}

fn charging_observation(result: Result<i32, impl std::fmt::Display>) -> Option<bool> {
    match result {
        Ok(0) => Some(false),
        Ok(1) => Some(true),
        Ok(value) => {
            tracing::warn!(event = "charging_value_invalid", value);
            None
        }
        Err(error) => {
            tracing::warn!(event = "charging_read_failed", error = %error);
            None
        }
    }
}

impl BatterySource for SystemPowerdBattery {
    type Error = PowerdReadError;

    async fn read_percentage(&mut self) -> Result<i32, Self::Error> {
        self.read_property(PROPERTY).await
    }

    async fn read_charging(&mut self) -> Result<Option<bool>, Self::Error> {
        match self.read_property("isCharging").await {
            Err(PowerdReadError::Cancelled) => Err(PowerdReadError::Cancelled),
            result => Ok(charging_observation(result)),
        }
    }
}

/// A battery source owning a blocking LIPC transport.
///
/// Read failures retain LIPC/DBus errors and do not fabricate a percentage.
/// Polling this compatibility adapter's read future can block; use
/// [`SystemPowerdBattery`] for production asynchronous session IO.
#[cfg(test)]
pub(crate) struct PowerdBattery<T> {
    client: LipcDbusClient<T>,
}

#[cfg(test)]
impl<T> PowerdBattery<T> {
    /// Takes ownership of an injected LIPC client without reading any property.
    fn new(client: LipcDbusClient<T>) -> Self {
        Self { client }
    }
}

#[cfg(test)]
impl<T: LipcMethodTransport> BatterySource for PowerdBattery<T> {
    type Error = LipcDbusError;

    async fn read_percentage(&mut self) -> Result<i32, Self::Error> {
        self.client.get_int_property(SERVICE, PROPERTY)
    }

    async fn read_charging(&mut self) -> Result<Option<bool>, Self::Error> {
        Ok(charging_observation(
            self.client.get_int_property(SERVICE, "isCharging"),
        ))
    }
}

#[cfg(test)]
mod tests;
