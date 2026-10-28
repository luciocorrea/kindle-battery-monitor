//! Awake-only monitor sessions and pure BTHome battery encoding.
//!
//! [`session::Session`] owns startup, event-driven sampling, publication and
//! explicit cleanup. Callers own process signal handling, tracing setup and exit
//! status. Request cancellation through [`session::Cancellation`] and keep polling
//! the session until cleanup completes; dropping a run future is not cleanup.
//!
//! [`bthome`] encodes validated samples without IO; [`logging`] supplies bounded
//! tracing writers. [`session::errors`] preserves typed causes and cleanup fields
//! without exposing device-operation interfaces. Waiting adds no suspend inhibitor
//! or wake alarm. Listener cleanup has a cooperative budget after radio cleanup,
//! rather than a whole-process shutdown deadline.
//!
//! BSA layouts and stock services are qualified only for Paperwhite 4 firmware
//! 5.16.5 as recorded in `docs/compatibility.md`. `docs/protocol.md` owns the wire
//! format and radio sequence. Host checks do not extend physical qualification.

#![forbid(unsafe_code)]

mod adapter;
mod broadcast;
mod bsa;
pub mod bthome;
mod config;
pub mod logging;
mod monitor;
mod ownership;
mod powerd;
mod powerd_events;
mod radio;
mod readiness;
mod runtime;

/// Monitor session composition and cooperative cancellation.
pub mod session;

#[cfg(test)]
mod error_chain_tests;

#[cfg(doctest)]
mod visibility_contract;
