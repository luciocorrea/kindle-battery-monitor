//! Asynchronous powerd battery-change subscription without battery polling.
//!
//! A notification is a sampling trigger, not a battery percentage. The monitor
//! reads the current percentage separately after receiving it.

use futures_lite::{future, StreamExt};
use std::future::Future;
use thiserror::Error;
use zbus::{message::Type, Connection, MessageStream};

#[cfg(test)]
mod shutdown_tests;

/// A battery notification subscription or receive failure.
#[derive(Clone, Debug, Error)]
pub enum PowerdEventError {
    /// The bus connection or acknowledged match-rule subscription failed.
    #[error("powerd event subscription failed: {0}")]
    Subscribe(#[source] zbus::Error),
    /// The subscribed stream ended.
    #[error("powerd event stream closed")]
    Closed,
    /// Receiving a subscribed message failed.
    #[error("powerd event receive failed: {0}")]
    Receive(#[source] zbus::Error),
}

/// A listener cleanup failure; multiple failures are retained in operation order.
#[derive(Clone, Debug, Error)]
pub enum PowerdCleanupError {
    /// Explicit shutdown was cancelled before its cleanup outcome was recorded.
    #[error("listener shutdown was interrupted; remote cleanup is unconfirmed")]
    Interrupted,
    /// The notification driver failed before shutdown.
    #[error("battery event reception failed: {0}")]
    Receive(#[source] PowerdEventError),
    /// Explicit match removal used its allotted part of the shared deadline.
    #[error("battery match removal exceeded its deadline")]
    UnsubscribeTimeout,
    /// Explicit match removal failed.
    #[error("battery match removal failed: {0}")]
    Unsubscribe(#[source] zbus::Error),
    /// Dedicated connection close exceeded the remaining overall deadline.
    #[error("battery connection close exceeded its deadline")]
    CloseTimeout,
    /// Dedicated connection close failed.
    #[error("battery connection close failed: {0}")]
    Close(#[source] zbus::Error),
    /// Explicit connection close was requested for a shared subscription.
    #[error("listener cleanup requires a dedicated subscription")]
    SharedConnection,
    /// The monotonic clock cannot represent the cleanup deadline.
    #[error("listener shutdown deadline is outside the clock range")]
    InvalidDeadline,
}

/// Owns a connection and acknowledged `battLevelChanged` subscription.
///
/// The match rule filters signal type, path, interface, and member, but does not
/// pin a unique sender. Notifications therefore trigger a fresh property read
/// rather than being used as authoritative battery values.
/// Dedicated system subscriptions track explicit registration for scoped runtime
/// cleanup. Dropping them releases local state without queuing match removal;
/// [`crate::runtime::EventListener::stop`] supplies cleanup diagnostics.
pub(crate) struct PowerdEvents {
    connection: Connection,
    stream: MessageStream,
    owned_rule: Option<String>,
}

impl PowerdEvents {
    /// Connects to the system bus and subscribes before returning.
    ///
    /// # Errors
    ///
    /// Returns connection or subscription errors. No overall timeout or cancellation
    /// input is supplied by this constructor.
    pub(crate) async fn system() -> Result<Self, PowerdEventError> {
        let connection = Connection::system()
            .await
            .map_err(PowerdEventError::Subscribe)?;
        Self::dedicated(connection).await
    }

    /// Acknowledges a battery-change match rule on an existing connection.
    ///
    /// Retains automatic zbus rule removal on Drop and never forcibly closes the
    /// caller's shared connection. Use the runtime listener for dedicated,
    /// explicitly bounded subscription cleanup.
    ///
    /// # Errors
    ///
    /// Returns match-rule or subscription failures.
    #[cfg(test)]
    pub(crate) async fn from_connection(connection: Connection) -> Result<Self, PowerdEventError> {
        let rule = zbus::MatchRule::builder()
            .msg_type(Type::Signal)
            .path("/default")
            .map_err(PowerdEventError::Subscribe)?
            .interface("com.lab126.powerd")
            .map_err(PowerdEventError::Subscribe)?
            .member("battLevelChanged")
            .map_err(PowerdEventError::Subscribe)?
            .build();
        let stream = MessageStream::for_match_rule(rule, &connection, Some(8))
            .await
            .map_err(PowerdEventError::Subscribe)?;
        Ok(Self {
            connection,
            stream,
            owned_rule: None,
        })
    }

    // A dedicated listener tracks its bus rule itself. The unfiltered stream has
    // no automatic RemoveMatch task on Drop, including cancellation of cleanup.
    pub(crate) async fn dedicated(connection: Connection) -> Result<Self, PowerdEventError> {
        let rule =
            "type='signal',path='/default',interface='com.lab126.powerd',member='battLevelChanged'"
                .to_owned();
        let stream = MessageStream::from(&connection);
        connection
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "AddMatch",
                &rule,
            )
            .await
            .map_err(PowerdEventError::Subscribe)?;
        Ok(Self {
            connection,
            stream,
            owned_rule: Some(rule),
        })
    }

    #[cfg(test)]
    pub(crate) async fn shutdown_with_deadlines(
        self,
        unsubscribe_expired: impl Future<Output = ()>,
        total_expired: impl Future<Output = ()>,
    ) -> Vec<PowerdCleanupError> {
        let mut failures = Vec::new();
        self.shutdown_recording(unsubscribe_expired, total_expired, &mut failures)
            .await;
        failures
    }

    // Record each completed phase outside this future before starting the next
    // await, so cancelling close cannot erase an observed unsubscribe failure.
    pub(crate) async fn shutdown_recording(
        self,
        unsubscribe_expired: impl Future<Output = ()>,
        total_expired: impl Future<Output = ()>,
        failures: &mut Vec<PowerdCleanupError>,
    ) {
        let Self {
            connection,
            stream,
            owned_rule,
        } = self;
        let Some(rule) = owned_rule else {
            failures.push(PowerdCleanupError::SharedConnection);
            return;
        };
        let removed = future::or(
            async {
                unsubscribe_expired.await;
                Err(PowerdCleanupError::UnsubscribeTimeout)
            },
            async {
                connection
                    .call_method(
                        Some("org.freedesktop.DBus"),
                        "/org/freedesktop/DBus",
                        Some("org.freedesktop.DBus"),
                        "RemoveMatch",
                        &rule,
                    )
                    .await
                    .map(|_| ())
                    .map_err(PowerdCleanupError::Unsubscribe)
            },
        )
        .await;
        if let Err(error) = removed {
            failures.push(error);
        }
        drop(stream);
        let closed = future::or(
            async {
                total_expired.await;
                Err(PowerdCleanupError::CloseTimeout)
            },
            async { connection.close().await.map_err(PowerdCleanupError::Close) },
        )
        .await;
        if let Err(error) = closed {
            failures.push(error);
        }
    }

    /// Waits for one notification without decoding its body.
    ///
    /// The caller can cancel the wait by dropping its future. The subscription remains
    /// owned by this value until dropped.
    /// Filtering unrelated messages yields cooperatively so a ready unicast queue
    /// cannot monopolize the caller's foreground driver.
    ///
    /// # Errors
    ///
    /// Returns stream closure or a receive error.
    pub(crate) async fn next(&mut self) -> Result<(), PowerdEventError> {
        loop {
            match self.stream.next().await {
                Some(Ok(message)) => {
                    let header = message.header();
                    if message.message_type() == Type::Signal
                        && header.path().map(|x| x.as_str()) == Some("/default")
                        && header.interface().map(|x| x.as_str()) == Some("com.lab126.powerd")
                        && header.member().map(|x| x.as_str()) == Some("battLevelChanged")
                    {
                        return Ok(());
                    }
                    // The dedicated stream is unfiltered. Cooperate even when
                    // a ready queue contains only unrelated unicast messages.
                    future::yield_now().await;
                }
                Some(Err(error)) => return Err(PowerdEventError::Receive(error)),
                None => return Err(PowerdEventError::Closed),
            }
        }
    }
}

#[cfg(test)]
mod tests;
