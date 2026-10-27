//! Blocking DBus transport and injectable raw LIPC method boundary.
//!
//! Transport methods return raw status codes. [`crate::LipcDbusClient`] checks
//! those codes before exposing values to consumers.

use zbus::blocking::{Connection, Proxy};

use crate::protocol::{service_interface, LIPC_OBJECT_PATH};
use crate::Result;

/// Borrowed addressing for one LIPC method destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LipcEndpoint<'a> {
    /// The destination bus name.
    pub service: &'a str,
    /// The object path, normally [`LIPC_OBJECT_PATH`].
    pub path: &'static str,
    /// The LIPC interface, normally the same spelling as the service name.
    pub interface: &'a str,
}

impl<'a> LipcEndpoint<'a> {
    /// Selects `/default` and uses `service` as both destination and interface.
    ///
    /// Construction borrows the name and performs no validation or IO.
    pub fn for_service(service: &'a str) -> Self {
        Self {
            service,
            path: LIPC_OBJECT_PATH,
            interface: service_interface(service),
        }
    }
}

/// Injectable synchronous raw-status method calls.
///
/// Implementations perform the named call at the supplied endpoint and decode
/// the expected reply shape. Nonzero LIPC statuses are returned as data so the
/// client can classify them. Connection, call, and decoding failures are errors.
/// Implementations own their IO timing; this trait provides no cancellation.
pub trait LipcMethodTransport {
    /// Reads an integer reply as `(status, value)` without interpreting the status.
    fn call_get_int(&self, endpoint: &LipcEndpoint<'_>, method: &str) -> Result<(u32, i32)>;

    /// Reads a string reply as `(status, value)` without interpreting the status.
    fn call_get_string(&self, endpoint: &LipcEndpoint<'_>, method: &str) -> Result<(u32, String)>;

    /// Sends an integer and decodes its raw status reply.
    fn call_set_int(&self, endpoint: &LipcEndpoint<'_>, method: &str, value: i32) -> Result<u32>;

    /// Sends a string and decodes its raw status reply.
    fn call_set_string(
        &self,
        endpoint: &LipcEndpoint<'_>,
        method: &str,
        value: &str,
    ) -> Result<u32>;
}

/// A cloneable blocking DBus connection used for LIPC method calls.
///
/// Each call builds a proxy and decodes its expected reply. This adapter adds
/// no retry, overall deadline, or cancellation input.
#[derive(Clone, Debug)]
pub struct ZbusTransport {
    connection: Connection,
}

impl ZbusTransport {
    /// Connects to the system bus or returns a DBus connection error.
    pub fn system() -> Result<Self> {
        Ok(Self {
            connection: Connection::system()?,
        })
    }

    /// Connects to the session bus or returns a DBus connection error.
    pub fn session() -> Result<Self> {
        Ok(Self {
            connection: Connection::session()?,
        })
    }

    /// Wraps an existing blocking connection without performing a method call.
    pub fn from_connection(connection: Connection) -> Self {
        Self { connection }
    }

    fn proxy<'a>(&self, endpoint: &LipcEndpoint<'a>) -> Result<Proxy<'a>> {
        Ok(Proxy::new(
            &self.connection,
            endpoint.service,
            endpoint.path,
            endpoint.interface,
        )?)
    }
}

impl LipcMethodTransport for ZbusTransport {
    fn call_get_int(&self, endpoint: &LipcEndpoint<'_>, method: &str) -> Result<(u32, i32)> {
        Ok(self.proxy(endpoint)?.call(method, &())?)
    }

    fn call_get_string(&self, endpoint: &LipcEndpoint<'_>, method: &str) -> Result<(u32, String)> {
        Ok(self.proxy(endpoint)?.call(method, &())?)
    }

    fn call_set_int(&self, endpoint: &LipcEndpoint<'_>, method: &str, value: i32) -> Result<u32> {
        let (status,) = self.proxy(endpoint)?.call(method, &value)?;
        Ok(status)
    }

    fn call_set_string(
        &self,
        endpoint: &LipcEndpoint<'_>,
        method: &str,
        value: &str,
    ) -> Result<u32> {
        let (status,) = self.proxy(endpoint)?.call(method, &value)?;
        Ok(status)
    }
}
