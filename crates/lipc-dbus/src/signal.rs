//! Asynchronous signal observations and diagnostic reports without body decoding.
//!
//! Subscription filters select messages but do not by themselves authenticate a
//! service owner. Observations capture headers and body shape, not body values.

use std::time::Duration;

use futures_lite::{future, StreamExt};
use zbus::{MessageStream, OwnedMatchRule};

use crate::protocol::event_match_rule;
use crate::smoke::{push_json_string, push_optional_json_string};
use crate::Result;

/// An asynchronous bus connection for one-shot signal observation.
///
/// Each wait owns a fresh subscription and removes that local stream when it
/// returns or its future is dropped. Connection/subscription setup is outside
/// the observation timeout.
#[derive(Clone, Debug)]
pub struct ZbusSignalMonitor {
    connection: zbus::Connection,
}

impl ZbusSignalMonitor {
    /// Connects to the system bus without subscribing.
    ///
    /// # Errors
    ///
    /// Returns a DBus connection error. No overall connection deadline is added.
    pub async fn system() -> Result<Self> {
        Ok(Self {
            connection: zbus::Connection::system().await?,
        })
    }

    /// Connects to the session bus, useful for isolated signal tests.
    ///
    /// # Errors
    ///
    /// Returns a DBus connection error. No overall connection deadline is added.
    pub async fn session() -> Result<Self> {
        Ok(Self {
            connection: zbus::Connection::session().await?,
        })
    }

    /// Stores an existing asynchronous connection without subscribing.
    pub fn from_connection(connection: zbus::Connection) -> Self {
        Self { connection }
    }

    /// Waits for one signal matching an interface and optional member.
    ///
    /// Uses [`crate::event_match_rule`], which does not pin a sender or path.
    /// See [`Self::wait_for_signal_rule`] for timeout, cancellation, and errors.
    pub async fn wait_for_signal(
        &self,
        service: &str,
        event: Option<&str>,
        timeout: Duration,
    ) -> Result<Option<SignalObservation>> {
        let match_rule = event_match_rule(service, event);
        self.wait_for_signal_rule(&match_rule, timeout).await
    }

    /// Subscribes to a rule, then waits for one observation or the timeout.
    ///
    /// The timer starts after the subscription is acknowledged. Returns `None` on
    /// timeout or stream closure. Dropping the future cancels the local wait; this
    /// method does not issue any service command or install a wake alarm.
    ///
    /// # Errors
    ///
    /// Returns rule parsing, subscription, receive, or observation errors.
    pub async fn wait_for_signal_rule(
        &self,
        match_rule: &str,
        timeout: Duration,
    ) -> Result<Option<SignalObservation>> {
        let rule = OwnedMatchRule::try_from(match_rule)?;
        let mut stream = MessageStream::for_match_rule(rule, &self.connection, Some(1)).await?;

        match future::or(
            async { SignalWaitResult::Message(stream.next().await) },
            async {
                async_io::Timer::after(timeout).await;
                SignalWaitResult::TimedOut
            },
        )
        .await
        {
            SignalWaitResult::Message(Some(message)) => {
                Ok(Some(SignalObservation::from_message(&message?)?))
            }
            SignalWaitResult::Message(None) | SignalWaitResult::TimedOut => Ok(None),
        }
    }
}

enum SignalWaitResult {
    Message(Option<zbus::Result<zbus::Message>>),
    TimedOut,
}

/// Message metadata without decoded body values.
///
/// Headers can contain service or sender identities; callers control where
/// observations and reports are written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignalObservation {
    /// The interface header, if present.
    pub interface: Option<String>,
    /// The signal member header, if present.
    pub member: Option<String>,
    /// The object path header, if present.
    pub path: Option<String>,
    /// The sender header, if present; it is not independently authenticated here.
    pub sender: Option<String>,
    /// The DBus body type signature.
    pub signature: String,
    /// The encoded body length in bytes.
    pub body_len: usize,
}

impl SignalObservation {
    /// Copies headers, signature, and body length without deserializing body values.
    ///
    /// The current implementation always returns `Ok`; the result type matches the
    /// fallible observation interface used by callers.
    pub fn from_message(message: &zbus::Message) -> Result<Self> {
        let header = message.header();
        let body = message.body();
        Ok(Self {
            interface: header.interface().map(ToString::to_string),
            member: header.member().map(ToString::to_string),
            path: header.path().map(ToString::to_string),
            sender: header.sender().map(ToString::to_string),
            signature: body.signature().to_string(),
            body_len: body.len(),
        })
    }
}

/// Whether the signal probe recorded an observation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SignalSmokeExitCode {
    /// One matching message was observed.
    Pass,
    /// No message was observed, including when the stream closed.
    TimedOut,
}

impl SignalSmokeExitCode {
    /// Returns zero for an observation and two for no observation.
    pub const fn as_i32(self) -> i32 {
        match self {
            Self::Pass => 0,
            Self::TimedOut => 2,
        }
    }
}

/// A match rule and optional single observation for diagnostic serialization.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SignalSmokeReport {
    match_rule: String,
    observation: Option<SignalObservation>,
}

impl SignalSmokeReport {
    /// Builds a successful report from an already captured observation without IO.
    pub fn observed(match_rule: &str, observation: SignalObservation) -> Self {
        Self {
            match_rule: match_rule.to_owned(),
            observation: Some(observation),
        }
    }

    /// Builds a report without an observation, also used for stream closure.
    pub fn timed_out(match_rule: &str) -> Self {
        Self {
            match_rule: match_rule.to_owned(),
            observation: None,
        }
    }

    /// Reports success only when an observation is present.
    pub fn exit_code(&self) -> SignalSmokeExitCode {
        if self.observation.is_some() {
            SignalSmokeExitCode::Pass
        } else {
            SignalSmokeExitCode::TimedOut
        }
    }

    /// Serializes an observation or timeout record followed by a summary and final newline.
    ///
    /// Includes message headers and match rule; performs no IO or body decoding.
    pub fn to_jsonl(&self) -> String {
        let mut jsonl = String::new();
        match &self.observation {
            Some(observation) => {
                jsonl.push_str("{\"kind\":\"signal\",\"status\":\"ok\",\"match_rule\":\"");
                push_json_string(&mut jsonl, &self.match_rule);
                jsonl.push('"');
                push_optional_json_string(
                    &mut jsonl,
                    "interface",
                    observation.interface.as_deref(),
                );
                push_optional_json_string(&mut jsonl, "member", observation.member.as_deref());
                push_optional_json_string(&mut jsonl, "path", observation.path.as_deref());
                push_optional_json_string(&mut jsonl, "sender", observation.sender.as_deref());
                jsonl.push_str(",\"signature\":\"");
                push_json_string(&mut jsonl, &observation.signature);
                jsonl.push_str("\",\"body_len\":");
                jsonl.push_str(&observation.body_len.to_string());
                jsonl.push_str("}\n");
            }
            None => {
                jsonl.push_str(
                    "{\"kind\":\"signal-timeout\",\"status\":\"timeout\",\"match_rule\":\"",
                );
                push_json_string(&mut jsonl, &self.match_rule);
                jsonl.push_str("\"}\n");
            }
        }
        jsonl.push_str("{\"kind\":\"summary\",\"status\":\"");
        jsonl.push_str(match self.exit_code() {
            SignalSmokeExitCode::Pass => "pass",
            SignalSmokeExitCode::TimedOut => "timeout",
        });
        jsonl.push_str("\",\"signals_observed\":");
        jsonl.push_str(match self.observation {
            Some(_) => "1",
            None => "0",
        });
        jsonl.push_str(",\"timed_out\":");
        jsonl.push_str(match self.observation {
            Some(_) => "false",
            None => "true",
        });
        jsonl.push_str("}\n");
        jsonl
    }
}

/// Connects to the system bus and observes one matching signal.
///
/// No property writes or other service commands are sent. The supplied timeout
/// covers only the wait after subscription, not bus connection or subscription
/// setup. A missing observation, whether from timeout or stream closure,
/// produces a timeout report.
///
/// # Errors
///
/// Returns connection, match-rule, subscription, or receive errors.
pub async fn run_signal_smoke_probe(
    service: &str,
    event: Option<&str>,
    timeout: Duration,
) -> Result<SignalSmokeReport> {
    let match_rule = event_match_rule(service, event);
    let monitor = ZbusSignalMonitor::system().await?;
    match monitor.wait_for_signal_rule(&match_rule, timeout).await? {
        Some(observation) => Ok(SignalSmokeReport::observed(&match_rule, observation)),
        None => Ok(SignalSmokeReport::timed_out(&match_rule)),
    }
}
