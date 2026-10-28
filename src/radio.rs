//! Stock DBus BLE source commands with event acknowledgement and readback.
//!
//! Transactions subscribe before issuing commands and target an existing frontend's
//! unique owner, avoiding service autoactivation. Deadlines cover subscription,
//! command, completion, and readback; ownership policy belongs to
//! [`crate::ownership`].

use std::future::Future;
use std::time::{Duration, Instant};

use futures_lite::StreamExt;
use zbus::names::OwnedUniqueName;

/// A change to the monitor's BLE-only stock source 4 requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioCommand {
    /// Request `1:4`, adding the BLE-only source requirement.
    EnableBle,
    /// Request `0:4`, releasing only the caller-owned BLE source requirement.
    DisableBle,
}

impl RadioCommand {
    fn event(self) -> RadioEvent {
        match self {
            Self::EnableBle => RadioEvent::BleEnabled,
            Self::DisableBle => RadioEvent::BleDisabled,
        }
    }

    fn argument(self) -> &'static str {
        match self {
            Self::EnableBle => "1:4",
            Self::DisableBle => "0:4",
        }
    }
}

/// The stock frontend completion signal expected after a source command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RadioEvent {
    /// Completion of a BLE enable request.
    BleEnabled,
    /// Completion of a BLE disable request.
    BleDisabled,
}

impl RadioEvent {
    fn member(self) -> &'static str {
        match self {
            Self::BleEnabled => "BleEnabled",
            Self::BleDisabled => "BleDisabled",
        }
    }
}

/// An ownership snapshot, not an atomic lease or reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioState {
    /// No backend currently owns `com.lab126.btmd` on the bus.
    DaemonAbsent,
    /// The backend exists and reports this source mask, including a possible zero mask.
    EnabledSources(u32),
}

/// A stock radio connection, command, completion, or readback failure.
#[derive(Debug, thiserror::Error)]
pub enum RadioError {
    /// The complete stock ownership inspection exceeded its five-second budget.
    #[error("stock Bluetooth ownership inspection exceeded its overall deadline")]
    OwnershipTimeout,
    /// The five-second system-bus connection deadline expired.
    #[error("system DBus connection exceeded its deadline")]
    ConnectionTimeout,
    /// A DBus operation failed with its operation context retained.
    #[error("DBus operation failed: {operation}")]
    Dbus {
        /// The DBus operation that failed.
        operation: &'static str,
        /// The underlying DBus error.
        #[source]
        source: zbus::Error,
    },
    /// The existing frontend required to send a command was absent.
    #[error("the stock Bluetooth frontend is not running")]
    FrontendAbsent,
    /// Source-mask readback returned a nonzero LIPC status.
    #[error("stock Bluetooth enable readback returned LIPC status {0}")]
    ReadStatus(u32),
    /// The complete command transaction exceeded its deadline.
    #[error("stock Bluetooth command {command:?} exceeded its overall deadline")]
    Timeout {
        /// The command whose deadline expired.
        command: RadioCommand,
    },
    /// The frontend rejected the command with a nonzero LIPC status.
    #[error("stock Bluetooth command {command:?} returned LIPC status {status}")]
    Status {
        /// The rejected command.
        command: RadioCommand,
        /// The raw nonzero LIPC status.
        status: u32,
    },
    /// The source-4 bit did not match the requested command after completion.
    #[error("stock Bluetooth command {command:?} completed with unexpected state {state:?}")]
    Readback {
        /// The command whose readback failed validation.
        command: RadioCommand,
        /// The observed state after the completion event.
        state: RadioState,
    },
    /// A required subscription, stream, mask, or deadline invariant was invalid.
    #[error("Bluetooth protocol failed: {0}")]
    Protocol(&'static str),
}

/// A connection to the system bus. Construction does not activate Bluetooth.
pub(crate) struct DbusRadio {
    connection: zbus::Connection,
    subscription: Option<CompletionSubscription>,
}

struct CompletionSubscription {
    frontend_owner: OwnedUniqueName,
    stream: zbus::MessageStream,
}

impl DbusRadio {
    /// Connects to the system bus within five seconds without activating Bluetooth.
    ///
    /// # Errors
    ///
    /// Returns a connection timeout or DBus connection error.
    pub(crate) async fn system() -> Result<Self, RadioError> {
        futures_lite::future::or(
            async {
                async_io::Timer::after(Duration::from_secs(5)).await;
                Err(RadioError::ConnectionTimeout)
            },
            async {
                let connection =
                    zbus::Connection::system()
                        .await
                        .map_err(|source| RadioError::Dbus {
                            operation: "connect to system bus",
                            source,
                        })?;
                Ok(Self::from_connection(connection))
            },
        )
        .await
    }

    /// Wraps an existing bus connection without subscribing or changing radio state.
    fn from_connection(connection: zbus::Connection) -> Self {
        Self {
            connection,
            subscription: None,
        }
    }

    async fn owner(&self, service: &'static str) -> Result<Option<OwnedUniqueName>, RadioError> {
        tracing::info!(event = "radio_owner_lookup", service);
        let reply = self
            .connection
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetNameOwner",
                &service,
            )
            .await;
        match reply {
            Ok(reply) => {
                tracing::info!(event = "radio_owner_reply", service);
                let (owner,): (OwnedUniqueName,) =
                    reply
                        .body()
                        .deserialize()
                        .map_err(|source| RadioError::Dbus {
                            operation: "decode existing service owner",
                            source,
                        })?;
                Ok(Some(owner))
            }
            Err(source) => {
                if let zbus::Error::MethodError(name, ..) = &source {
                    if name.as_str() == "org.freedesktop.DBus.Error.NameHasNoOwner" {
                        return Ok(None);
                    }
                }
                Err(RadioError::Dbus {
                    operation: "resolve existing service owner",
                    source,
                })
            }
        }
    }
}

fn completion_rule(
    owner: &OwnedUniqueName,
    event: RadioEvent,
) -> zbus::Result<zbus::OwnedMatchRule> {
    Ok(zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(owner.as_str())?
        .path("/default")?
        .interface("com.lab126.btfd")?
        .member(event.member())?
        .build()
        .into())
}

impl RadioProtocol for DbusRadio {
    async fn subscribe(&mut self, event: RadioEvent) -> Result<(), RadioError> {
        self.subscription = None;
        let owner = self
            .owner("com.lab126.btfd")
            .await?
            .ok_or(RadioError::FrontendAbsent)?;
        tracing::info!(event = "radio_frontend_resolved");
        let rule = completion_rule(&owner, event).map_err(|source| RadioError::Dbus {
            operation: "build completion subscription",
            source,
        })?;
        // for_match_rule awaits AddMatch acknowledgment before returning.
        let stream = zbus::MessageStream::for_match_rule(rule, &self.connection, Some(8))
            .await
            .map_err(|source| RadioError::Dbus {
                operation: "subscribe to stock BLE completion",
                source,
            })?;
        tracing::info!(event = "radio_match_acknowledged");
        self.subscription = Some(CompletionSubscription {
            frontend_owner: owner,
            stream,
        });
        Ok(())
    }

    async fn set_source(&mut self, value: &str) -> Result<u32, RadioError> {
        let subscription = self
            .subscription
            .as_ref()
            .ok_or(RadioError::Protocol("completion subscription is missing"))?;
        // A unique destination cannot trigger service autoactivation or target a replacement owner.
        let reply = self
            .connection
            .call_method(
                Some(subscription.frontend_owner.as_str()),
                "/default",
                Some("com.lab126.btfd"),
                "setBTenableStr",
                &value,
            )
            .await
            .map_err(|source| RadioError::Dbus {
                operation: "set stock BLE source requirement",
                source,
            })?;
        let (status,): (u32,) = reply
            .body()
            .deserialize()
            .map_err(|source| RadioError::Dbus {
                operation: "decode stock BLE request status",
                source,
            })?;
        Ok(status)
    }

    async fn wait_completion(&mut self) -> Result<(), RadioError> {
        let subscription = self
            .subscription
            .as_mut()
            .ok_or(RadioError::Protocol("completion subscription is missing"))?;
        let message = subscription
            .stream
            .next()
            .await
            .ok_or(RadioError::Protocol("completion stream closed"))?;
        message.map_err(|source| RadioError::Dbus {
            operation: "receive stock BLE completion",
            source,
        })?;
        Ok(())
    }

    async fn enabled_sources(&mut self) -> Result<RadioState, RadioError> {
        let Some(owner) = self.owner("com.lab126.btmd").await? else {
            return Ok(RadioState::DaemonAbsent);
        };
        let reply = self
            .connection
            .call_method(
                Some(owner.as_str()),
                "/default",
                Some("com.lab126.btmd"),
                "getEnableInt",
                &(),
            )
            .await
            .map_err(|source| RadioError::Dbus {
                operation: "read existing stock BLE source mask",
                source,
            })?;
        let (status, mask): (u32, i32) =
            reply
                .body()
                .deserialize()
                .map_err(|source| RadioError::Dbus {
                    operation: "decode stock BLE source mask",
                    source,
                })?;
        if status != 0 {
            return Err(RadioError::ReadStatus(status));
        }
        let mask = u32::try_from(mask)
            .map_err(|_| RadioError::Protocol("stock source mask is negative"))?;
        Ok(RadioState::EnabledSources(mask))
    }
}

/// The acknowledged subscription, stock LIPC command, event, and readback boundary.
pub(crate) trait RadioProtocol {
    /// Acknowledges a completion subscription bound to the existing frontend owner
    /// before any command is sent. Replaces any previous subscription.
    fn subscribe(&mut self, event: RadioEvent) -> impl Future<Output = Result<(), RadioError>>;
    /// Sends the stock source argument to the subscribed owner and returns its raw status.
    ///
    /// Requires a successful subscription; it must not activate or target a replacement owner.
    fn set_source(&mut self, value: &str) -> impl Future<Output = Result<u32, RadioError>>;
    /// Waits for the subscribed completion signal without polling.
    ///
    /// The controller supplies the overall deadline; stream closure is an error.
    fn wait_completion(&mut self) -> impl Future<Output = Result<(), RadioError>>;
    /// Reads the existing backend's source mask without activating an absent backend.
    ///
    /// A nonzero LIPC status or negative mask is an error.
    fn enabled_sources(&mut self) -> impl Future<Output = Result<RadioState, RadioError>>;
}

/// Sequences acknowledged stock commands under one transaction deadline.
///
/// It verifies source 4, not exclusive ownership, and performs no compensating
/// release on timeout, error, or future cancellation. The caller owns cleanup.
pub(crate) struct RadioController<P> {
    protocol: P,
}

impl<P> RadioController<P> {
    /// Takes ownership of the protocol adapter without performing a transaction.
    pub(crate) fn new(protocol: P) -> Self {
        Self { protocol }
    }

    /// Returns the protocol adapter without performing radio cleanup.
    #[cfg(test)]
    fn into_protocol(self) -> P {
        self.protocol
    }
}

impl<P: RadioProtocol> RadioController<P> {
    /// Executes a source command with a 30-second enable or 10-second disable deadline.
    ///
    /// The caller must own the temporary BLE lease before requesting disable.
    /// See [`Self::execute_with_timeout`] for sequencing, cleanup obligations,
    /// and errors. This method does not provide automatic rollback.
    pub(crate) async fn execute(
        &mut self,
        command: RadioCommand,
    ) -> Result<RadioState, RadioError> {
        let timeout = match command {
            RadioCommand::EnableBle => Duration::from_secs(30),
            RadioCommand::DisableBle => Duration::from_secs(10),
        };
        self.execute_with_timeout(command, timeout).await
    }

    /// Executes a source command under a caller-selected overall deadline.
    ///
    /// Subscribes, sends the command, waits for completion, then verifies source 4
    /// while retaining other source bits. A zero timeout fails before protocol IO.
    /// Dropping the future or timing out can leave a command applied; neither rolls
    /// back device state. The caller must arrange source release after attempted
    /// acquisition and own source 4 before requesting disable.
    ///
    /// # Errors
    ///
    /// Returns deadline, DBus, command-status, completion, or readback failures.
    /// An unrepresentable deadline is a protocol error.
    async fn execute_with_timeout(
        &mut self,
        command: RadioCommand,
        timeout: Duration,
    ) -> Result<RadioState, RadioError> {
        if timeout.is_zero() {
            return Err(RadioError::Timeout { command });
        }
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(RadioError::Protocol("deadline is outside the clock range"))?;
        futures_lite::future::or(
            async {
                async_io::Timer::at(deadline).await;
                Err(RadioError::Timeout { command })
            },
            self.transaction(command),
        )
        .await
    }

    async fn transaction(&mut self, command: RadioCommand) -> Result<RadioState, RadioError> {
        self.protocol.subscribe(command.event()).await?;
        tracing::info!(event = "radio_subscription_ready", command = ?command);
        let status = self.protocol.set_source(command.argument()).await?;
        tracing::info!(event = "radio_request_acknowledged", command = ?command, status);
        if status != 0 {
            return Err(RadioError::Status { command, status });
        }
        self.protocol.wait_completion().await?;
        tracing::info!(event = "radio_completion_observed", command = ?command);
        let state = self.protocol.enabled_sources().await?;
        tracing::info!(event = "radio_state_observed", command = ?command, state = ?state);
        let ble_enabled = match state {
            RadioState::DaemonAbsent => false,
            RadioState::EnabledSources(mask) => mask & 4 != 0,
        };
        let matches_command = match command {
            RadioCommand::EnableBle => ble_enabled,
            RadioCommand::DisableBle => !ble_enabled,
        };
        if !matches_command {
            return Err(RadioError::Readback { command, state });
        }
        Ok(state)
    }
}

#[cfg(test)]
mod tests;
