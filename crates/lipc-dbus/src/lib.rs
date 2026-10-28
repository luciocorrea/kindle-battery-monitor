//! Bundled safe LIPC property calls and signal observation over DBus.
//!
//! [`LipcDbusClient`] uses an injectable, blocking [`LipcMethodTransport`] and
//! converts nonzero wire status codes into [`LipcDbusError::Status`].
//! [`ZbusSignalMonitor`] provides asynchronous, event-driven observation without
//! polling. No native Amazon library is linked.
//!
//! Property calls add no application-level overall deadline or cancellation.
//! Signal wait timeouts begin after subscription setup, not during connection or
//! subscription. Callers own any broader startup or shutdown deadlines.
//! Smoke reports describe individual observations and do not establish firmware
//! compatibility, radio reception, or suspend behavior.

#![forbid(unsafe_code)]

mod client;
mod error;
mod protocol;
mod signal;
mod smoke;
mod status;
mod transport;

pub use client::LipcDbusClient;
pub use error::LipcDbusError;
pub use protocol::{
    event_match_rule, property_method_name, service_interface, PropertyAccess, PropertyType,
    LIPC_OBJECT_PATH,
};
pub use signal::{
    run_signal_smoke_probe, SignalObservation, SignalSmokeExitCode, SignalSmokeReport,
    ZbusSignalMonitor,
};
pub use smoke::{run_smoke_probe, SmokeExitCode, SmokeReport};
pub use status::LipcStatus;
pub use transport::{LipcEndpoint, LipcMethodTransport, ZbusTransport};

/// The result of a LIPC operation, preserving transport and service-status failures.
pub type Result<T> = std::result::Result<T, LipcDbusError>;

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::cell::RefCell;

    #[derive(Debug, Eq, PartialEq)]
    enum FakeCall {
        Get {
            service: String,
            path: String,
            interface: String,
            method: String,
        },
        SetInt {
            service: String,
            path: String,
            interface: String,
            method: String,
            value: i32,
        },
        SetString {
            service: String,
            path: String,
            interface: String,
            method: String,
            value: String,
        },
    }

    enum FakeReply {
        Int(u32, i32),
        String(u32, String),
        Status(u32),
    }

    struct FakeTransport {
        calls: RefCell<Vec<FakeCall>>,
        replies: RefCell<Vec<FakeReply>>,
    }

    impl FakeTransport {
        fn new(replies: Vec<FakeReply>) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                replies: RefCell::new(replies),
            }
        }

        fn next_reply(&self) -> FakeReply {
            self.replies.borrow_mut().remove(0)
        }
    }

    impl LipcMethodTransport for FakeTransport {
        fn call_get_int(&self, endpoint: &LipcEndpoint<'_>, method: &str) -> Result<(u32, i32)> {
            self.calls.borrow_mut().push(FakeCall::Get {
                service: endpoint.service.to_owned(),
                path: endpoint.path.to_owned(),
                interface: endpoint.interface.to_owned(),
                method: method.to_owned(),
            });

            match self.next_reply() {
                FakeReply::Int(status, value) => Ok((status, value)),
                FakeReply::String(_, _) | FakeReply::Status(_) => {
                    panic!("test configured an incompatible fake reply")
                }
            }
        }

        fn call_get_string(
            &self,
            endpoint: &LipcEndpoint<'_>,
            method: &str,
        ) -> Result<(u32, String)> {
            self.calls.borrow_mut().push(FakeCall::Get {
                service: endpoint.service.to_owned(),
                path: endpoint.path.to_owned(),
                interface: endpoint.interface.to_owned(),
                method: method.to_owned(),
            });

            match self.next_reply() {
                FakeReply::String(status, value) => Ok((status, value)),
                FakeReply::Int(_, _) | FakeReply::Status(_) => {
                    panic!("test configured an incompatible fake reply")
                }
            }
        }

        fn call_set_int(
            &self,
            endpoint: &LipcEndpoint<'_>,
            method: &str,
            value: i32,
        ) -> Result<u32> {
            self.calls.borrow_mut().push(FakeCall::SetInt {
                service: endpoint.service.to_owned(),
                path: endpoint.path.to_owned(),
                interface: endpoint.interface.to_owned(),
                method: method.to_owned(),
                value,
            });

            match self.next_reply() {
                FakeReply::Status(status) => Ok(status),
                FakeReply::Int(_, _) | FakeReply::String(_, _) => {
                    panic!("test configured an incompatible fake reply")
                }
            }
        }

        fn call_set_string(
            &self,
            endpoint: &LipcEndpoint<'_>,
            method: &str,
            value: &str,
        ) -> Result<u32> {
            self.calls.borrow_mut().push(FakeCall::SetString {
                service: endpoint.service.to_owned(),
                path: endpoint.path.to_owned(),
                interface: endpoint.interface.to_owned(),
                method: method.to_owned(),
                value: value.to_owned(),
            });

            match self.next_reply() {
                FakeReply::Status(status) => Ok(status),
                FakeReply::Int(_, _) | FakeReply::String(_, _) => {
                    panic!("test configured an incompatible fake reply")
                }
            }
        }
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "a fake successful reply must decode in this test"
    )]
    fn get_int_property_calls_default_lipc_endpoint_and_decodes_value() {
        let transport = FakeTransport::new(vec![FakeReply::Int(0, 87)]);
        let client = LipcDbusClient::from_transport(transport);

        let value = client
            .get_int_property("com.lab126.powerd", "battLevel")
            .expect("fake LIPC get int reply should decode");

        assert_eq!(value, 87);
        assert_eq!(
            client.transport_ref().calls.borrow().as_slice(),
            [FakeCall::Get {
                service: "com.lab126.powerd".to_owned(),
                path: "/default".to_owned(),
                interface: "com.lab126.powerd".to_owned(),
                method: "getbattLevelInt".to_owned(),
            }]
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "a fake successful reply must decode in this test"
    )]
    fn get_string_property_calls_default_lipc_endpoint_and_decodes_value() {
        let transport = FakeTransport::new(vec![FakeReply::String(0, "ready".to_owned())]);
        let client = LipcDbusClient::from_transport(transport);

        let value = client
            .get_string_property("com.lab126.powerd", "status")
            .expect("fake LIPC get string reply should decode");

        assert_eq!(value, "ready");
        assert_eq!(
            client.transport_ref().calls.borrow().as_slice(),
            [FakeCall::Get {
                service: "com.lab126.powerd".to_owned(),
                path: "/default".to_owned(),
                interface: "com.lab126.powerd".to_owned(),
                method: "getstatusStr".to_owned(),
            }]
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "a fake successful reply must decode in this test"
    )]
    fn set_int_property_calls_lipc_method_with_value() {
        let transport = FakeTransport::new(vec![FakeReply::Status(0)]);
        let client = LipcDbusClient::from_transport(transport);

        client
            .set_int_property("com.lab126.powerd", "preventScreenSaver", 1)
            .expect("fake LIPC set int reply should decode");

        assert_eq!(
            client.transport_ref().calls.borrow().as_slice(),
            [FakeCall::SetInt {
                service: "com.lab126.powerd".to_owned(),
                path: "/default".to_owned(),
                interface: "com.lab126.powerd".to_owned(),
                method: "setpreventScreenSaverInt".to_owned(),
                value: 1,
            }]
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "a fake successful reply must decode in this test"
    )]
    fn set_string_property_calls_lipc_method_with_value() {
        let transport = FakeTransport::new(vec![FakeReply::Status(0)]);
        let client = LipcDbusClient::from_transport(transport);

        client
            .set_string_property("com.lab126.winmgr", "orientationLock", "U")
            .expect("fake LIPC set string reply should decode");

        assert_eq!(
            client.transport_ref().calls.borrow().as_slice(),
            [FakeCall::SetString {
                service: "com.lab126.winmgr".to_owned(),
                path: "/default".to_owned(),
                interface: "com.lab126.winmgr".to_owned(),
                method: "setorientationLockStr".to_owned(),
                value: "U".to_owned(),
            }]
        );
    }

    #[test]
    #[expect(
        clippy::expect_used,
        reason = "a fake error reply must fail in this test"
    )]
    fn nonzero_lipc_status_is_returned_as_typed_error() {
        let transport = FakeTransport::new(vec![FakeReply::Int(8, 0)]);
        let client = LipcDbusClient::from_transport(transport);

        let error = client
            .get_int_property("com.lab126.powerd", "missing")
            .expect_err("nonzero LIPC status must fail");

        assert_eq!(error, LipcDbusError::Status(LipcStatus::NoSuchProperty));
    }

    #[test]
    fn smoke_probe_reports_required_powerd_properties() {
        let transport = FakeTransport::new(vec![
            FakeReply::Int(0, 89),
            FakeReply::Int(0, 0),
            FakeReply::String(0, "active".to_owned()),
            FakeReply::Int(8, 0),
            FakeReply::Int(8, 0),
            FakeReply::String(0, "battLevel:rw:i".to_owned()),
        ]);
        let client = LipcDbusClient::from_transport(transport);

        let report = run_smoke_probe(&client);

        assert_eq!(report.exit_code(), SmokeExitCode::Pass);
        assert_eq!(
            report.to_jsonl(),
            concat!(
                "{\"kind\":\"required-int\",\"service\":\"com.lab126.powerd\",\"property\":\"battLevel\",\"status\":\"ok\",\"value\":89}\n",
                "{\"kind\":\"required-int\",\"service\":\"com.lab126.powerd\",\"property\":\"isCharging\",\"status\":\"ok\",\"value\":0}\n",
                "{\"kind\":\"required-string\",\"service\":\"com.lab126.powerd\",\"property\":\"state\",\"status\":\"ok\",\"value\":\"active\"}\n",
                "{\"kind\":\"optional-int\",\"service\":\"com.lab126.powerd\",\"property\":\"flIntensity\",\"status\":\"error\",\"error\":\"LIPC returned status NoSuchProperty\"}\n",
                "{\"kind\":\"optional-int\",\"service\":\"com.lab126.powerd\",\"property\":\"currentAmberLevel\",\"status\":\"error\",\"error\":\"LIPC returned status NoSuchProperty\"}\n",
                "{\"kind\":\"optional-string\",\"service\":\"com.lab126.powerd\",\"property\":\"_properties\",\"status\":\"ok\",\"value\":\"battLevel:rw:i\"}\n",
                "{\"kind\":\"summary\",\"status\":\"pass\",\"required_passed\":3,\"required_failed\":0}\n"
            )
        );
    }

    #[test]
    fn smoke_probe_fails_when_required_property_fails() {
        let transport = FakeTransport::new(vec![
            FakeReply::Int(0, 89),
            FakeReply::Int(8, 0),
            FakeReply::String(0, "active".to_owned()),
            FakeReply::Int(8, 0),
            FakeReply::Int(8, 0),
            FakeReply::String(0, "battLevel:rw:i".to_owned()),
        ]);
        let client = LipcDbusClient::from_transport(transport);

        let report = run_smoke_probe(&client);

        assert_eq!(report.exit_code(), SmokeExitCode::Fail);
        assert!(report
            .to_jsonl()
            .contains("\"property\":\"isCharging\",\"status\":\"error\""));
    }

    #[test]
    fn signal_report_records_observed_signal_metadata() {
        let report = SignalSmokeReport::observed(
            "type='signal',interface='com.lab126.powerd'",
            SignalObservation {
                interface: Some("com.lab126.powerd".to_owned()),
                member: Some("battLevelChanged".to_owned()),
                path: Some("/default".to_owned()),
                sender: Some(":1.42".to_owned()),
                signature: "i".to_owned(),
                body_len: 4,
            },
        );

        assert_eq!(report.exit_code(), SignalSmokeExitCode::Pass);
        assert_eq!(
            report.to_jsonl(),
            concat!(
                "{\"kind\":\"signal\",\"status\":\"ok\",\"match_rule\":\"type='signal',interface='com.lab126.powerd'\",\"interface\":\"com.lab126.powerd\",\"member\":\"battLevelChanged\",\"path\":\"/default\",\"sender\":\":1.42\",\"signature\":\"i\",\"body_len\":4}\n",
                "{\"kind\":\"summary\",\"status\":\"pass\",\"signals_observed\":1,\"timed_out\":false}\n"
            )
        );
    }

    #[test]
    fn signal_report_records_timeout_without_signal() {
        let report = SignalSmokeReport::timed_out("type='signal',interface='com.lab126.powerd'");

        assert_eq!(report.exit_code(), SignalSmokeExitCode::TimedOut);
        assert_eq!(
            report.to_jsonl(),
            concat!(
                "{\"kind\":\"signal-timeout\",\"status\":\"timeout\",\"match_rule\":\"type='signal',interface='com.lab126.powerd'\"}\n",
                "{\"kind\":\"summary\",\"status\":\"timeout\",\"signals_observed\":0,\"timed_out\":true}\n"
            )
        );
    }
}
