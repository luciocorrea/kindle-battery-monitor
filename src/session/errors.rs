//! Typed session diagnostics without exposing device-operation interfaces.
//!
//! Concrete aliases name the production error combinations. Generic error types
//! remain available for matching structured causes and ordered cleanup fields.

pub use crate::adapter::BsaRadioError;
pub use crate::broadcast::BroadcastError;
pub use crate::bsa::BsaError;
pub use crate::config::ConfigError;
pub use crate::monitor::{MonitorError, RunError};
pub use crate::ownership::OwnedBroadcastError;
pub use crate::powerd::PowerdReadError;
pub use crate::powerd_events::{PowerdCleanupError, PowerdEventError};
pub use crate::radio::{RadioCommand, RadioError, RadioState};
pub use crate::readiness::ReadinessError;
pub use crate::runtime::{
    BsaBurstError, EventListenerError, InvalidRefreshInterval, RuntimeEventError,
};

/// A publication or source-ownership failure, including all cleanup failures.
pub type PublicationError = OwnedBroadcastError<RadioError, BsaBurstError>;
/// A single sample's read, validation or publication failure.
pub type SampleError = MonitorError<PowerdReadError, PublicationError>;
/// A terminal continuous-monitor event or publication failure.
pub type ContinuousError = RunError<RuntimeEventError, PublicationError>;
/// Readiness cancellation, timeout or underlying DBus failure.
pub type StartupError = ReadinessError<zbus::Error>;
