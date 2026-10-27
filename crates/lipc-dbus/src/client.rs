//! Typed blocking LIPC property access over an injected method transport.
//!
//! A property value is returned only after its raw LIPC status is checked.
//! Transport errors propagate unchanged; nonzero statuses become typed errors.

use crate::error::LipcDbusError;
use crate::protocol::{property_method_name, PropertyAccess, PropertyType};
use crate::status::LipcStatus;
use crate::transport::{LipcEndpoint, LipcMethodTransport, ZbusTransport};
use crate::Result;

/// A blocking property client owning its transport.
///
/// Calls use `/default` and the service name as interface. The client adds no
/// retry, overall deadline, or cancellation; transport policy controls blocking.
#[derive(Clone, Debug)]
pub struct LipcDbusClient<T = ZbusTransport> {
    pub(crate) transport: T,
}

impl LipcDbusClient<ZbusTransport> {
    /// Connects the default blocking transport to the system bus.
    ///
    /// # Errors
    ///
    /// Returns a DBus connection error.
    pub fn system() -> Result<Self> {
        Ok(Self::from_transport(ZbusTransport::system()?))
    }

    /// Connects the default blocking transport to the session bus, useful for isolated tests.
    ///
    /// # Errors
    ///
    /// Returns a DBus connection error.
    pub fn session() -> Result<Self> {
        Ok(Self::from_transport(ZbusTransport::session()?))
    }
}

impl<T> LipcDbusClient<T> {
    /// Takes ownership of an injected transport without making a property call.
    pub fn from_transport(transport: T) -> Self {
        Self { transport }
    }

    #[cfg(test)]
    pub(crate) fn transport_ref(&self) -> &T {
        &self.transport
    }
}

impl<T: LipcMethodTransport> LipcDbusClient<T> {
    /// Calls `get<Property>Int`, preserving the supplied property spelling.
    ///
    /// # Errors
    ///
    /// Returns transport/decoding failures or a nonzero LIPC status. The integer is
    /// otherwise returned unchanged; domain range validation belongs to the caller.
    pub fn get_int_property(&self, service: &str, property: &str) -> Result<i32> {
        let endpoint = LipcEndpoint::for_service(service);
        let method = property_method_name(PropertyAccess::Get, property, PropertyType::Int);
        let (status, value) = self.transport.call_get_int(&endpoint, &method)?;
        ensure_ok(status)?;
        Ok(value)
    }

    /// Calls `set<Property>Int` and checks its status without additional readback.
    ///
    /// # Errors
    ///
    /// Returns transport/decoding failures or a nonzero LIPC status.
    pub fn set_int_property(&self, service: &str, property: &str, value: i32) -> Result<()> {
        let endpoint = LipcEndpoint::for_service(service);
        let method = property_method_name(PropertyAccess::Set, property, PropertyType::Int);
        let status = self.transport.call_set_int(&endpoint, &method, value)?;
        ensure_ok(status)
    }

    /// Calls `get<Property>Str` and returns the string only on status zero.
    ///
    /// # Errors
    ///
    /// Returns transport/decoding failures or a nonzero LIPC status.
    pub fn get_string_property(&self, service: &str, property: &str) -> Result<String> {
        let endpoint = LipcEndpoint::for_service(service);
        let method = property_method_name(PropertyAccess::Get, property, PropertyType::Str);
        let (status, value) = self.transport.call_get_string(&endpoint, &method)?;
        ensure_ok(status)?;
        Ok(value)
    }

    /// Calls `set<Property>Str` and checks its status without additional readback.
    ///
    /// # Errors
    ///
    /// Returns transport/decoding failures or a nonzero LIPC status.
    pub fn set_string_property(&self, service: &str, property: &str, value: &str) -> Result<()> {
        let endpoint = LipcEndpoint::for_service(service);
        let method = property_method_name(PropertyAccess::Set, property, PropertyType::Str);
        let status = self.transport.call_set_string(&endpoint, &method, value)?;
        ensure_ok(status)
    }
}

pub(crate) fn ensure_ok(status_code: u32) -> Result<()> {
    let status = LipcStatus::from_wire_code(status_code);
    match status {
        LipcStatus::Ok => Ok(()),
        LipcStatus::Unknown
        | LipcStatus::Internal
        | LipcStatus::NoSuchSource
        | LipcStatus::OperationNotSupported
        | LipcStatus::OutOfMemory
        | LipcStatus::SubscriptionFailed
        | LipcStatus::NoSuchParam
        | LipcStatus::NoSuchProperty
        | LipcStatus::AccessNotAllowed
        | LipcStatus::BufferTooSmall
        | LipcStatus::InvalidHandle
        | LipcStatus::InvalidArg
        | LipcStatus::OperationNotAllowed
        | LipcStatus::ParamsSizeExceeded
        | LipcStatus::TimedOut
        | LipcStatus::ServiceNameTooLong
        | LipcStatus::DuplicateServiceName
        | LipcStatus::InitDbus
        | LipcStatus::PropertyInvalidState
        | LipcStatus::PropertyNotInitialized
        | LipcStatus::PropertyInternal
        | LipcStatus::UnknownCode(_) => Err(LipcDbusError::Status(status)),
    }
}
