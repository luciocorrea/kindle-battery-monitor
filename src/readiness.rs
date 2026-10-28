//! Event-driven startup readiness for powerd and the stock Bluetooth frontend.
//!
//! Subscribe before taking owner snapshots, then recheck only after relevant
//! owner-change events. The caller supplies a finite deadline and cancellation;
//! readiness adds no polling, service activation, or suspend inhibitor.

use futures_lite::StreamExt;
use std::{error::Error, future::Future, time::Duration};
use thiserror::Error;
use zbus::{Connection, MessageStream};

const SERVICES: [&str; 2] = ["com.lab126.powerd", "com.lab126.btfd"];

/// Injectable owner-subscription boundary used by [`wait_for_services`].
///
/// Owner snapshots are ordered as powerd, then `com.lab126.btfd`. Implementations
/// must allow in-flight futures to be dropped for timeout or cancellation.
pub(crate) trait ReadinessProtocol {
    /// A subscription, owner lookup, or owner-change stream failure.
    type Error: Error;
    /// Acknowledges owner-change monitoring before the initial owner snapshot.
    fn subscribe(&mut self) -> impl Future<Output = Result<(), Self::Error>>;
    /// Returns current powerd/frontend owner presence without starting either service.
    fn owners(&mut self) -> impl Future<Output = Result<[bool; 2], Self::Error>>;
    /// Waits for a relevant owner change; the caller takes a fresh snapshot afterward.
    fn changed(&mut self) -> impl Future<Output = Result<(), Self::Error>>;
}

/// Why startup failed to observe both required service owners.
#[derive(Debug, Error)]
pub enum ReadinessError<E: Error> {
    /// The overall readiness deadline expired.
    #[error("startup service readiness exceeded its deadline")]
    Timeout,
    /// The caller's cancellation future completed.
    #[error("startup cancelled")]
    Cancelled,
    /// The bus subscription, owner lookup, or event stream failed.
    #[error("startup service readiness failed: {0}")]
    Protocol(#[source] E),
}

async fn ready<P: ReadinessProtocol>(protocol: &mut P) -> Result<(), ReadinessError<P::Error>> {
    protocol
        .subscribe()
        .await
        .map_err(ReadinessError::Protocol)?;
    loop {
        if protocol
            .owners()
            .await
            .map_err(ReadinessError::Protocol)?
            .into_iter()
            .all(|present| present)
        {
            return Ok(());
        }
        // A new snapshot follows an owner-change event, never a polling timer.
        protocol.changed().await.map_err(ReadinessError::Protocol)?;
    }
}

async fn bounded<E: Error>(
    work: impl Future<Output = Result<(), ReadinessError<E>>>,
    cancelled: impl Future<Output = ()>,
    timeout: Duration,
) -> Result<(), ReadinessError<E>> {
    futures_lite::future::or(
        async {
            cancelled.await;
            Err(ReadinessError::Cancelled)
        },
        futures_lite::future::or(
            async {
                async_io::Timer::after(timeout).await;
                Err(ReadinessError::Timeout)
            },
            work,
        ),
    )
    .await
}

/// Waits for both service owners under one timeout and cancellation future.
///
/// Subscribes before the first snapshot to avoid losing an intervening change.
/// Rechecks after events rather than on a polling timer. Completion proves only
/// owner presence at the time of observation, not continued service health.
/// Timeout or cancellation drops pending protocol work; no device state is changed.
///
/// # Errors
///
/// Returns timeout, cancellation, or protocol errors.
#[cfg(test)]
pub(crate) async fn wait_for_services<P: ReadinessProtocol>(
    protocol: &mut P,
    cancelled: impl Future<Output = ()>,
    timeout: Duration,
) -> Result<(), ReadinessError<P::Error>> {
    bounded(ready(protocol), cancelled, timeout).await
}

/// Connects to the system bus and waits for powerd/frontend owners.
///
/// The supplied timeout and cancellation cover connection setup as well as the
/// readiness sequence described by [`wait_for_services`].
///
/// # Errors
///
/// Returns timeout, cancellation, or underlying DBus errors.
pub(crate) async fn system_readiness(
    cancelled: impl Future<Output = ()>,
    timeout: Duration,
) -> Result<(), ReadinessError<zbus::Error>> {
    bounded(
        async {
            let connection = Connection::system()
                .await
                .map_err(ReadinessError::Protocol)?;
            ready(&mut DbusReadiness::new(connection)).await
        },
        cancelled,
        timeout,
    )
    .await
}

/// A DBus implementation of owner-change readiness, retaining its subscription.
struct DbusReadiness {
    connection: Connection,
    stream: Option<MessageStream>,
}

impl DbusReadiness {
    /// Stores an existing connection without subscribing or activating services.
    fn new(connection: Connection) -> Self {
        Self {
            connection,
            stream: None,
        }
    }
}

impl ReadinessProtocol for DbusReadiness {
    type Error = zbus::Error;

    async fn subscribe(&mut self) -> Result<(), Self::Error> {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender("org.freedesktop.DBus")?
            .path("/org/freedesktop/DBus")?
            .interface("org.freedesktop.DBus")?
            .member("NameOwnerChanged")?
            .build();
        self.stream = Some(MessageStream::for_match_rule(rule, &self.connection, Some(64)).await?);
        Ok(())
    }

    async fn owners(&mut self) -> Result<[bool; 2], Self::Error> {
        let mut present = [false; 2];
        for (index, name) in SERVICES.into_iter().enumerate() {
            let reply = self
                .connection
                .call_method(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    Some("org.freedesktop.DBus"),
                    "NameHasOwner",
                    &name,
                )
                .await?;
            present[index] = reply.body().deserialize()?;
        }
        Ok(present)
    }

    async fn changed(&mut self) -> Result<(), Self::Error> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| zbus::Error::Failure("startup subscription missing".into()))?;
        loop {
            let message = stream
                .next()
                .await
                .ok_or_else(|| zbus::Error::Failure("startup owner stream closed".into()))??;
            let (name, _, _): (String, String, String) = message.body().deserialize()?;
            if SERVICES.contains(&name.as_str()) {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests;
