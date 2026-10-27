//! Lossless mapping between LIPC wire status codes and named outcomes.

/// A known LIPC status or the original unrecognized unsigned wire code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LipcStatus {
    /// Successful operation, code zero.
    Ok,
    /// The service reported an unspecified error.
    Unknown,
    /// The service reported an internal error.
    Internal,
    /// The requested LIPC source was not found.
    NoSuchSource,
    /// The requested operation is unsupported.
    OperationNotSupported,
    /// The service reported memory allocation failure.
    OutOfMemory,
    /// The service reported subscription failure.
    SubscriptionFailed,
    /// The requested parameter was not found.
    NoSuchParam,
    /// The requested property was not found.
    NoSuchProperty,
    /// The service denied access.
    AccessNotAllowed,
    /// The service reported an insufficient buffer.
    BufferTooSmall,
    /// The service rejected a handle.
    InvalidHandle,
    /// The service rejected an argument.
    InvalidArg,
    /// The service denied the operation.
    OperationNotAllowed,
    /// The service reported excessive parameter size.
    ParamsSizeExceeded,
    /// The service reported a timeout, distinct from a local DBus transport failure.
    TimedOut,
    /// The service rejected an overlong service name.
    ServiceNameTooLong,
    /// The service reported a duplicate service name.
    DuplicateServiceName,
    /// The service reported DBus initialization failure.
    InitDbus,
    /// The property reported an invalid state.
    PropertyInvalidState,
    /// The property has not been initialized.
    PropertyNotInitialized,
    /// The property reported an internal error.
    PropertyInternal,
    /// An unrecognized code retained without truncation.
    UnknownCode(u32),
}

impl LipcStatus {
    /// Maps known unsigned codes and preserves all other values in [`Self::UnknownCode`].
    pub const fn from_wire_code(code: u32) -> Self {
        match code {
            0 => Self::Ok,
            1 => Self::Unknown,
            2 => Self::Internal,
            3 => Self::NoSuchSource,
            4 => Self::OperationNotSupported,
            5 => Self::OutOfMemory,
            6 => Self::SubscriptionFailed,
            7 => Self::NoSuchParam,
            8 => Self::NoSuchProperty,
            9 => Self::AccessNotAllowed,
            10 => Self::BufferTooSmall,
            11 => Self::InvalidHandle,
            12 => Self::InvalidArg,
            13 => Self::OperationNotAllowed,
            14 => Self::ParamsSizeExceeded,
            15 => Self::TimedOut,
            16 => Self::ServiceNameTooLong,
            17 => Self::DuplicateServiceName,
            18 => Self::InitDbus,
            0x100 => Self::PropertyInvalidState,
            0x101 => Self::PropertyNotInitialized,
            0x102 => Self::PropertyInternal,
            value => Self::UnknownCode(value),
        }
    }

    /// Maps nonnegative signed codes; negative values retain their bits as unknown `u32` codes.
    pub const fn from_code(code: i32) -> Self {
        if code < 0 {
            Self::UnknownCode(code as u32)
        } else {
            Self::from_wire_code(code as u32)
        }
    }

    /// Returns the unsigned wire code, preserving the value of [`Self::UnknownCode`].
    pub const fn code(self) -> u32 {
        match self {
            Self::Ok => 0,
            Self::Unknown => 1,
            Self::Internal => 2,
            Self::NoSuchSource => 3,
            Self::OperationNotSupported => 4,
            Self::OutOfMemory => 5,
            Self::SubscriptionFailed => 6,
            Self::NoSuchParam => 7,
            Self::NoSuchProperty => 8,
            Self::AccessNotAllowed => 9,
            Self::BufferTooSmall => 10,
            Self::InvalidHandle => 11,
            Self::InvalidArg => 12,
            Self::OperationNotAllowed => 13,
            Self::ParamsSizeExceeded => 14,
            Self::TimedOut => 15,
            Self::ServiceNameTooLong => 16,
            Self::DuplicateServiceName => 17,
            Self::InitDbus => 18,
            Self::PropertyInvalidState => 0x100,
            Self::PropertyNotInitialized => 0x101,
            Self::PropertyInternal => 0x102,
            Self::UnknownCode(value) => value,
        }
    }
}
