//! Asynchronous powerd battery-change subscription without battery polling.
//!
//! A notification is a sampling trigger, not a battery percentage. The monitor
//! reads the current percentage separately after receiving it.

use futures_lite::StreamExt;
use thiserror::Error;
use zbus::{message::Type, Connection, MessageStream};

/// A battery notification subscription or receive failure.
#[derive(Debug, Error)]
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

/// Owns a connection and acknowledged `battLevelChanged` subscription.
///
/// The match rule filters signal type, path, interface, and member, but does not
/// pin a unique sender. Notifications therefore trigger a fresh property read
/// rather than being used as authoritative battery values.
pub struct PowerdEvents {
    _connection: Connection,
    stream: MessageStream,
}

impl PowerdEvents {
    /// Connects to the system bus and subscribes before returning.
    ///
    /// # Errors
    ///
    /// Returns connection or subscription errors. No overall timeout or cancellation
    /// input is supplied by this constructor.
    pub async fn system() -> Result<Self, PowerdEventError> {
        let connection = Connection::system()
            .await
            .map_err(PowerdEventError::Subscribe)?;
        Self::from_connection(connection).await
    }

    /// Acknowledges a battery-change match rule on an existing connection.
    ///
    /// # Errors
    ///
    /// Returns match-rule or subscription failures.
    pub async fn from_connection(connection: Connection) -> Result<Self, PowerdEventError> {
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
            _connection: connection,
            stream,
        })
    }

    /// Waits for one notification without decoding its body.
    ///
    /// The caller can cancel the wait by dropping its future. The subscription remains
    /// owned by this value until dropped.
    ///
    /// # Errors
    ///
    /// Returns stream closure or a receive error.
    pub async fn next(&mut self) -> Result<(), PowerdEventError> {
        match self.stream.next().await {
            Some(Ok(_)) => Ok(()),
            Some(Err(error)) => Err(PowerdEventError::Receive(error)),
            None => Err(PowerdEventError::Closed),
        }
    }
}
