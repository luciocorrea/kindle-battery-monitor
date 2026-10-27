//! Read-only powerd property probes with required and optional observations.
//!
//! These are diagnostic observations, not device qualification or battery-life
//! measurements. Reports retain returned values and error strings; callers must
//! review them before sharing.

use crate::client::LipcDbusClient;
use crate::transport::LipcMethodTransport;

const POWERD_SERVICE: &str = "com.lab126.powerd";

/// The aggregate outcome of required property reads.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SmokeExitCode {
    /// No required read failed; optional failures do not affect this outcome.
    Pass,
    /// At least one required read failed.
    Fail,
}

impl SmokeExitCode {
    /// Returns zero for pass and one for failure.
    pub const fn as_i32(self) -> i32 {
        match self {
            Self::Pass => 0,
            Self::Fail => 1,
        }
    }
}

/// Collected property observations and required-read counters.
///
/// A default empty report passes because it contains no failed required reads;
/// use [`run_smoke_probe`] to perform the actual observations.
#[derive(Debug, Default)]
pub struct SmokeReport {
    lines: Vec<String>,
    required_passed: usize,
    required_failed: usize,
}

impl SmokeReport {
    /// Returns failure only when a required read failed.
    pub fn exit_code(&self) -> SmokeExitCode {
        if self.required_failed == 0 {
            SmokeExitCode::Pass
        } else {
            SmokeExitCode::Fail
        }
    }

    /// Serializes observation lines followed by a summary and final newline.
    ///
    /// Includes returned values or error strings and performs no IO.
    pub fn to_jsonl(&self) -> String {
        let mut jsonl = String::new();
        for line in &self.lines {
            jsonl.push_str(line);
            jsonl.push('\n');
        }
        jsonl.push_str("{\"kind\":\"summary\",\"status\":\"");
        jsonl.push_str(match self.exit_code() {
            SmokeExitCode::Pass => "pass",
            SmokeExitCode::Fail => "fail",
        });
        jsonl.push_str("\",\"required_passed\":");
        jsonl.push_str(&self.required_passed.to_string());
        jsonl.push_str(",\"required_failed\":");
        jsonl.push_str(&self.required_failed.to_string());
        jsonl.push_str("}\n");
        jsonl
    }
}

/// Reads six powerd properties synchronously without writing device state.
///
/// `battLevel`, `isCharging`, and `state` are required; `flIntensity`,
/// `currentAmberLevel`, and `_properties` are optional. All reads are attempted,
/// with failures recorded in the report instead of returned early. The probe
/// adds no cancellation, retry, or overall deadline to the transport's blocking
/// calls. Success records readable properties, not domain-range validation.
pub fn run_smoke_probe<T: LipcMethodTransport>(client: &LipcDbusClient<T>) -> SmokeReport {
    let mut report = SmokeReport::default();

    record_required_int(&mut report, client, POWERD_SERVICE, "battLevel");
    record_required_int(&mut report, client, POWERD_SERVICE, "isCharging");
    record_required_string(&mut report, client, POWERD_SERVICE, "state");
    record_optional_int(&mut report, client, POWERD_SERVICE, "flIntensity");
    record_optional_int(&mut report, client, POWERD_SERVICE, "currentAmberLevel");
    record_optional_string(&mut report, client, POWERD_SERVICE, "_properties");

    report
}

fn record_required_int<T: LipcMethodTransport>(
    report: &mut SmokeReport,
    client: &LipcDbusClient<T>,
    service: &'static str,
    property: &'static str,
) {
    match client.get_int_property(service, property) {
        Ok(value) => {
            report.required_passed += 1;
            report
                .lines
                .push(int_line("required-int", service, property, "ok", value));
        }
        Err(error) => {
            report.required_failed += 1;
            report.lines.push(error_line(
                "required-int",
                service,
                property,
                "error",
                &error.to_string(),
            ));
        }
    }
}

fn record_required_string<T: LipcMethodTransport>(
    report: &mut SmokeReport,
    client: &LipcDbusClient<T>,
    service: &'static str,
    property: &'static str,
) {
    match client.get_string_property(service, property) {
        Ok(value) => {
            report.required_passed += 1;
            report.lines.push(string_line(
                "required-string",
                service,
                property,
                "ok",
                &value,
            ));
        }
        Err(error) => {
            report.required_failed += 1;
            report.lines.push(error_line(
                "required-string",
                service,
                property,
                "error",
                &error.to_string(),
            ));
        }
    }
}

fn record_optional_int<T: LipcMethodTransport>(
    report: &mut SmokeReport,
    client: &LipcDbusClient<T>,
    service: &'static str,
    property: &'static str,
) {
    match client.get_int_property(service, property) {
        Ok(value) => {
            report
                .lines
                .push(int_line("optional-int", service, property, "ok", value));
        }
        Err(error) => {
            report.lines.push(error_line(
                "optional-int",
                service,
                property,
                "error",
                &error.to_string(),
            ));
        }
    }
}

fn record_optional_string<T: LipcMethodTransport>(
    report: &mut SmokeReport,
    client: &LipcDbusClient<T>,
    service: &'static str,
    property: &'static str,
) {
    match client.get_string_property(service, property) {
        Ok(value) => {
            report.lines.push(string_line(
                "optional-string",
                service,
                property,
                "ok",
                &value,
            ));
        }
        Err(error) => {
            report.lines.push(error_line(
                "optional-string",
                service,
                property,
                "error",
                &error.to_string(),
            ));
        }
    }
}

fn int_line(kind: &str, service: &str, property: &str, status: &str, value: i32) -> String {
    let mut line = line_prefix(kind, service, property, status);
    line.push_str(",\"value\":");
    line.push_str(&value.to_string());
    line.push('}');
    line
}

fn string_line(kind: &str, service: &str, property: &str, status: &str, value: &str) -> String {
    let mut line = line_prefix(kind, service, property, status);
    line.push_str(",\"value\":\"");
    push_json_string(&mut line, value);
    line.push_str("\"}");
    line
}

fn error_line(kind: &str, service: &str, property: &str, status: &str, error: &str) -> String {
    let mut line = line_prefix(kind, service, property, status);
    line.push_str(",\"error\":\"");
    push_json_string(&mut line, error);
    line.push_str("\"}");
    line
}

fn line_prefix(kind: &str, service: &str, property: &str, status: &str) -> String {
    let mut line = String::from("{\"kind\":\"");
    push_json_string(&mut line, kind);
    line.push_str("\",\"service\":\"");
    push_json_string(&mut line, service);
    line.push_str("\",\"property\":\"");
    push_json_string(&mut line, property);
    line.push_str("\",\"status\":\"");
    push_json_string(&mut line, status);
    line.push('"');
    line
}

pub(crate) fn push_json_string(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            value => output.push(value),
        }
    }
}

pub(crate) fn push_optional_json_string(output: &mut String, field: &str, value: Option<&str>) {
    output.push_str(",\"");
    output.push_str(field);
    output.push_str("\":");
    match value {
        Some(value) => {
            output.push('"');
            push_json_string(output, value);
            output.push('"');
        }
        None => output.push_str("null"),
    }
}
