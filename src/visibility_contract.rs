//! Architecture invariant: implementation modules are inaccessible to external callers.
//!
//! `adapter` stays internal.
//! ```compile_fail,E0603
//! use kindle_battery_monitor::adapter;
//! ```
//!
//! `broadcast` stays internal.
//! ```compile_fail,E0603
//! use kindle_battery_monitor::broadcast;
//! ```
//!
//! `bsa` stays internal.
//! ```compile_fail,E0603
//! use kindle_battery_monitor::bsa;
//! ```
//!
//! `config` stays internal.
//! ```compile_fail,E0603
//! use kindle_battery_monitor::config;
//! ```
//!
//! `monitor` stays internal.
//! ```compile_fail,E0603
//! use kindle_battery_monitor::monitor;
//! ```
//!
//! `ownership` stays internal.
//! ```compile_fail,E0603
//! use kindle_battery_monitor::ownership;
//! ```
//!
//! `powerd` stays internal.
//! ```compile_fail,E0603
//! use kindle_battery_monitor::powerd;
//! ```
//!
//! `powerd_events` stays internal.
//! ```compile_fail,E0603
//! use kindle_battery_monitor::powerd_events;
//! ```
//!
//! `radio` stays internal.
//! ```compile_fail,E0603
//! use kindle_battery_monitor::radio;
//! ```
//!
//! `readiness` stays internal.
//! ```compile_fail,E0603
//! use kindle_battery_monitor::readiness;
//! ```
//!
//! `runtime` stays internal.
//! ```compile_fail,E0603
//! use kindle_battery_monitor::runtime;
//! ```
//!
