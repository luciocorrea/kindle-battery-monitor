//! Recoverable LIPC failures separated by transport and service-status origin.

use thiserror::Error;

use crate::status::LipcStatus;

/// A DBus failure or a nonzero status returned by the LIPC service.
///
/// Equality compares status variants only. Transport failures compare unequal,
/// including when compared to themselves.
#[derive(Debug, Error)]
pub enum LipcDbusError {
    /// Connecting, calling, subscribing, or decoding through DBus failed.
    #[error("DBus transport failed: {0}")]
    Transport(#[from] zbus::Error),
    /// The service returned a nonzero status, preserving unknown wire codes.
    #[error("LIPC returned status {0:?}")]
    Status(LipcStatus),
}

impl PartialEq for LipcDbusError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Status(left), Self::Status(right)) => left == right,
            (Self::Transport(_), Self::Transport(_)) => false,
            (Self::Transport(_), Self::Status(_)) | (Self::Status(_), Self::Transport(_)) => false,
        }
    }
}
