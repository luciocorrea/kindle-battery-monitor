//! Awake-only battery sampling and bounded BTHome radio transactions.
//!
//! [`monitor`] coordinates injected battery, event, and publication boundaries;
//! [`runtime`] supplies the blocking device adapters. [`ownership`] acquires and
//! releases stock BLE source 4, while [`broadcast`] handles advertisement cleanup.
//! Waiting uses events or ordinary timers, without suspend inhibitors or wake alarms.
//!
//! BSA layouts and stock service behavior are qualified only for the Paperwhite 4
//! firmware 5.16.5 described in the repository's `docs/compatibility.md`.
//! `docs/protocol.md` records the wire format and radio transaction sequence.
//! Host tests and these API contracts do not extend physical device qualification.

#![forbid(unsafe_code)]

pub mod adapter;
pub mod broadcast;
pub mod bsa;
pub mod bthome;
pub mod config;
pub mod logging;
pub mod monitor;
pub mod ownership;
pub mod powerd;
pub mod powerd_events;
pub mod radio;
pub mod readiness;
pub mod runtime;
