//! Battery property reads through injected LIPC and isolated async DBus transports.

use crate::monitor::BatterySource;
use crate::powerd::{PowerdBattery, PowerdReadError, SystemPowerdBattery};
use kindle_lipc_dbus::{
    LipcDbusClient, LipcDbusError, LipcEndpoint, LipcMethodTransport, LipcStatus,
};
use pretty_assertions::assert_eq;
use std::error::Error;
use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct PrivateTestBus(Child);

impl PrivateTestBus {
    fn start() -> (Self, String) {
        let mut child = Command::new("dbus-daemon")
            .args([
                "--session",
                "--nofork",
                "--print-address=1",
                "--address=unix:tmpdir=/tmp",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .expect("private test bus starts");
        let stdout = child.stdout.take().expect("bus address output is piped");
        let mut address = String::new();
        std::io::BufReader::new(stdout)
            .read_line(&mut address)
            .expect("test bus reports its address");
        (Self(child), address.trim().to_owned())
    }
}

impl Drop for PrivateTestBus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct GatedPowerd {
    entered: async_channel::Sender<()>,
    replies: async_channel::Receiver<(u32, i32)>,
}

#[zbus::interface(name = "com.lab126.powerd", spawn = false)]
impl GatedPowerd {
    #[zbus(name = "getisChargingInt")]
    async fn charging(&self) -> (u32, i32) {
        self.battery_level().await
    }

    #[zbus(name = "getbattLevelInt")]
    async fn battery_level(&self) -> (u32, i32) {
        self.entered.send(()).await.expect("record method entry");
        // The service executor's watchdog also bounds a regression that blocks
        // the client's executor before its independent gate future can run.
        futures_lite::future::or(
            async { self.replies.recv().await.expect("scripted battery reply") },
            async {
                async_io::Timer::after(Duration::from_secs(3)).await;
                (LipcStatus::TimedOut.code(), 0)
            },
        )
        .await
    }

    // Serial dispatch makes this a readback barrier for any previously issued
    // battery request on the same client connection, without fixed sleeps.
    fn barrier(&self) {}
}

struct RecordingBatteryBroadcast(Vec<Vec<u8>>);

impl crate::monitor::Broadcaster for RecordingBatteryBroadcast {
    type Error = std::convert::Infallible;

    async fn broadcast(
        &mut self,
        sample: crate::bthome::BthomeSample,
    ) -> Result<crate::monitor::Publication, Self::Error> {
        self.0.push(sample.service_data());
        Ok(crate::monitor::Publication::Published)
    }
}

#[test]
fn charging_notifications_read_current_properties_instead_of_trusting_event_bodies() {
    use crate::monitor::{Monitor, Trigger};
    use crate::powerd_events::PowerdEvents;
    let (_bus, address) = PrivateTestBus::start();
    futures_lite::future::block_on(futures_lite::future::or(
        async {
            let (entered, _observed) = async_channel::unbounded();
            let (reply, replies) = async_channel::unbounded();
            let publisher = zbus::connection::Builder::address(address.as_str())
                .expect("private bus address")
                .name("com.lab126.powerd")
                .expect("powerd name")
                .serve_at("/default", GatedPowerd { entered, replies })
                .expect("gated powerd")
                .build()
                .await
                .expect("publisher");
            let connection = zbus::connection::Builder::address(address.as_str())
                .expect("private bus address")
                .build()
                .await
                .expect("property client");
            let mut battery = SystemPowerdBattery::from_connection(connection);
            let connection = zbus::connection::Builder::address(address.as_str())
                .expect("private bus address")
                .build()
                .await
                .expect("event client");
            let mut events = PowerdEvents::dedicated(connection)
                .await
                .expect("subscription");
            let mut monitor = Monitor::new(7);
            let mut broadcaster = RecordingBatteryBroadcast(Vec::new());
            for (member, charging, stale_body) in [
                ("charging", 0, "charging is not confirmed yet"),
                (
                    "battStateInfoChanged",
                    1,
                    "{\"battInfo\":[{\"charging\":false}]}",
                ),
                ("notCharging", 0, "{\"charging\":true}"),
            ] {
                reply.send((0, 52)).await.expect("current percentage");
                reply
                    .send((0, charging))
                    .await
                    .expect("current charging state");
                publisher
                    .emit_signal(
                        None::<&str>,
                        "/default",
                        "com.lab126.powerd",
                        member,
                        &stale_body,
                    )
                    .await
                    .expect("state notification sent");
                events.next().await.expect("state notification received");
                monitor
                    .handle(Trigger::BatteryChanged, &mut battery, &mut broadcaster)
                    .await
                    .expect("fresh properties published");
            }
            assert_eq!(
                broadcaster.0,
                vec![
                    vec![0xd2, 0xfc, 0x44, 0, 7, 1, 52, 0x16, 0],
                    vec![0xd2, 0xfc, 0x44, 0, 8, 1, 52, 0x16, 1],
                    vec![0xd2, 0xfc, 0x44, 0, 9, 1, 52, 0x16, 0],
                ]
            );
            assert!(events
                .shutdown_with_deadlines(std::future::pending(), std::future::pending())
                .await
                .is_empty());
        },
        async {
            async_io::Timer::after(Duration::from_secs(5)).await;
            panic!("notification readback watchdog");
        },
    ));
}

#[test]
fn charging_samples_accept_only_binary_values_and_keep_battery_on_optional_failures() {
    let (_bus, address) = PrivateTestBus::start();
    futures_lite::future::block_on(async {
        let (entered, _observed) = async_channel::unbounded();
        let (reply, replies) = async_channel::unbounded();
        let _server = zbus::connection::Builder::address(address.as_str())
            .expect("private bus address")
            .name("com.lab126.powerd")
            .expect("powerd test name")
            .serve_at("/default", GatedPowerd { entered, replies })
            .expect("gated powerd")
            .build()
            .await
            .expect("powerd test service connects");
        let connection = zbus::connection::Builder::address(address.as_str())
            .expect("private bus address")
            .build()
            .await
            .expect("client connects");
        let mut battery = SystemPowerdBattery::from_connection(connection);
        let mut monitor = crate::monitor::Monitor::new(7);
        let mut broadcaster = RecordingBatteryBroadcast(Vec::new());
        let mut expected_packets = Vec::new();
        for (status, raw, expected) in [
            (0, 0, Some(false)),
            (0, 1, Some(true)),
            (0, -1, None),
            (0, 2, None),
            (LipcStatus::NoSuchProperty.code(), 1, None),
            (0x8000_0042, 1, None),
            (0, 1, Some(true)),
        ] {
            reply.send((0, 52)).await.expect("script percentage reply");
            reply
                .send((status, raw))
                .await
                .expect("script charging reply");
            monitor
                .handle(
                    crate::monitor::Trigger::Refresh,
                    &mut battery,
                    &mut broadcaster,
                )
                .await
                .expect("battery report survives optional failure");
            let mut expected_bytes =
                vec![0xd2, 0xfc, 0x44, 0, 7 + expected_packets.len() as u8, 1, 52];
            if let Some(charging) = expected {
                expected_bytes.extend_from_slice(&[0x16, u8::from(charging)]);
            }
            expected_packets.push(expected_bytes);
        }
        assert_eq!(broadcaster.0, expected_packets);
    });
}

#[test]
fn cancelling_a_held_charging_read_is_not_an_optional_observation_failure() {
    let (_bus, address) = PrivateTestBus::start();
    futures_lite::future::block_on(async {
        let (entered, observed) = async_channel::bounded(1);
        let (reply, replies) = async_channel::bounded(1);
        let _server = zbus::connection::Builder::address(address.as_str())
            .expect("private bus address")
            .name("com.lab126.powerd")
            .expect("powerd test name")
            .serve_at("/default", GatedPowerd { entered, replies })
            .expect("gated powerd")
            .build()
            .await
            .expect("powerd test service connects");
        let connection = zbus::connection::Builder::address(address.as_str())
            .expect("private bus address")
            .build()
            .await
            .expect("client connects");
        let (cancel, cancelled) = async_channel::bounded(1);
        let mut battery =
            SystemPowerdBattery::from_connection(connection).with_cancellation(cancelled);
        let (result, ()) = futures_lite::future::zip(battery.read_charging(), async {
            observed.recv().await.expect("charging call is pending");
            cancel.send(()).await.expect("cancel pending charging read");
        })
        .await;
        reply.send((0, 1)).await.expect("release remote reply");
        assert!(matches!(result, Err(PowerdReadError::Cancelled)));
    });
}

#[test]
fn cancelling_a_held_async_read_returns_no_value_and_drops_the_pending_operation() {
    let (_bus, address) = PrivateTestBus::start();
    futures_lite::future::block_on(async {
        let (entered, observed) = async_channel::bounded(1);
        let (reply, replies) = async_channel::bounded(1);
        let _server = zbus::connection::Builder::address(address.as_str())
            .expect("private bus address")
            .name("com.lab126.powerd")
            .expect("powerd test name")
            .serve_at("/default", GatedPowerd { entered, replies })
            .expect("gated powerd")
            .build()
            .await
            .expect("powerd test service connects");
        let connection = zbus::connection::Builder::address(address.as_str())
            .expect("private bus address")
            .build()
            .await
            .expect("client connects");
        let mut released = Box::pin(connection.clone().graceful_shutdown());
        assert!(futures_lite::future::poll_once(released.as_mut())
            .await
            .is_none());
        let (cancel, cancelled) = async_channel::bounded(1);
        let mut battery =
            SystemPowerdBattery::from_connection(connection).with_cancellation(cancelled);
        let (value, ()) = futures_lite::future::zip(battery.read_percentage(), async {
            observed
                .recv()
                .await
                .expect("battery method remains pending");
            cancel.send(()).await.expect("cancel the pending read");
        })
        .await;
        assert!(matches!(value, Err(PowerdReadError::Cancelled)));
        drop(battery);
        let released_before_reply = futures_lite::future::poll_once(released.as_mut())
            .await
            .is_some();
        // Release the fake server even if the local ownership assertion fails.
        reply
            .send((0, 83))
            .await
            .expect("release late remote reply");
        assert!(
            released_before_reply,
            "a cancelled operation retained the client connection"
        );
    });
}

#[test]
fn already_ready_cancellation_or_disconnection_starts_no_battery_read() {
    for disconnect in [false, true] {
        let (_bus, address) = PrivateTestBus::start();
        futures_lite::future::block_on(async {
            let (entered, observed) = async_channel::bounded(1);
            let (reply, replies) = async_channel::bounded(1);
            let _server = zbus::connection::Builder::address(address.as_str())
                .expect("private bus address")
                .name("com.lab126.powerd")
                .expect("powerd test name")
                .serve_at("/default", GatedPowerd { entered, replies })
                .expect("gated powerd")
                .build()
                .await
                .expect("powerd test service connects");
            let connection = zbus::connection::Builder::address(address.as_str())
                .expect("private bus address")
                .build()
                .await
                .expect("client connects");
            let barrier = connection.clone();
            let (cancel, cancelled) = async_channel::bounded(1);
            if disconnect {
                drop(cancel);
            } else {
                cancel.send(()).await.expect("queue cancellation");
            }
            let mut battery =
                SystemPowerdBattery::from_connection(connection).with_cancellation(cancelled);
            assert!(matches!(
                battery.read_percentage().await,
                Err(PowerdReadError::Cancelled)
            ));
            // A regressed read-first race may have sent a call before noticing
            // cancellation. Let it complete, then observe the serial barrier.
            reply
                .send((0, 83))
                .await
                .expect("script any unexpected read");
            barrier
                .call_method(
                    Some("com.lab126.powerd"),
                    "/default",
                    Some("com.lab126.powerd"),
                    "Barrier",
                    &(),
                )
                .await
                .expect("method ordering barrier");
            assert!(
                observed.try_recv().is_err(),
                "ready cancellation issued a battery request"
            );
        });
    }
}

#[test]
fn an_async_battery_read_allows_other_futures_to_progress_while_pending() {
    let (_bus, address) = PrivateTestBus::start();
    futures_lite::future::block_on(async {
        let (entered, observed) = async_channel::bounded(1);
        let (reply, replies) = async_channel::bounded(1);
        let _server = zbus::connection::Builder::address(address.as_str())
            .expect("private bus address")
            .name("com.lab126.powerd")
            .expect("powerd test name")
            .serve_at("/default", GatedPowerd { entered, replies })
            .expect("gated powerd")
            .build()
            .await
            .expect("powerd test service connects");
        let connection = zbus::connection::Builder::address(address.as_str())
            .expect("private bus address")
            .build()
            .await
            .expect("client connects");
        let mut battery = SystemPowerdBattery::from_connection(connection);
        let (value, ()) = futures_lite::future::zip(battery.read_percentage(), async {
            observed.recv().await.expect("battery method is pending");
            reply
                .send((0, 137))
                .await
                .expect("independent future releases method");
        })
        .await;
        assert_eq!(value.expect("asynchronous property reply"), 137);
    });
}

#[test]
fn async_reads_preserve_raw_percentages_and_known_and_unknown_lipc_statuses() {
    let (_bus, address) = PrivateTestBus::start();
    futures_lite::future::block_on(async {
        let (entered, _observed) = async_channel::unbounded();
        let (reply, replies) = async_channel::bounded(1);
        let _server = zbus::connection::Builder::address(address.as_str())
            .expect("private bus address")
            .name("com.lab126.powerd")
            .expect("powerd test name")
            .serve_at("/default", GatedPowerd { entered, replies })
            .expect("gated powerd")
            .build()
            .await
            .expect("powerd test service connects");
        let connection = zbus::connection::Builder::address(address.as_str())
            .expect("private bus address")
            .build()
            .await
            .expect("client connects");
        let mut battery = SystemPowerdBattery::from_connection(connection);
        for value in [-1, 83, 137] {
            reply.send((0, value)).await.expect("script raw value");
            assert_eq!(
                battery
                    .read_percentage()
                    .await
                    .expect("raw battery integer"),
                value
            );
        }
        for status in [
            LipcStatus::NoSuchProperty,
            LipcStatus::UnknownCode(0x8000_0042),
        ] {
            reply
                .send((status.code(), 83))
                .await
                .expect("script LIPC failure");
            match battery
                .read_percentage()
                .await
                .expect_err("nonzero status rejected")
            {
                PowerdReadError::Lipc(LipcDbusError::Status(actual)) => assert_eq!(actual, status),
                other => panic!("unexpected battery status error: {other:?}"),
            }
        }
    });
}

struct MalformedPowerd;

#[zbus::interface(name = "com.lab126.powerd")]
impl MalformedPowerd {
    #[zbus(name = "getisChargingInt")]
    fn charging(&self) -> &str {
        "not a status/integer tuple"
    }

    #[zbus(name = "getbattLevelInt")]
    fn battery_level(&self) -> &str {
        "not a status/integer tuple"
    }
}

#[test]
fn malformed_async_replies_retain_decode_context_and_the_dbus_source() {
    let (_bus, address) = PrivateTestBus::start();
    futures_lite::future::block_on(async {
        let _server = zbus::connection::Builder::address(address.as_str())
            .expect("private bus address")
            .name("com.lab126.powerd")
            .expect("powerd test name")
            .serve_at("/default", MalformedPowerd)
            .expect("malformed powerd")
            .build()
            .await
            .expect("powerd test service connects");
        let connection = zbus::connection::Builder::address(address.as_str())
            .expect("private bus address")
            .build()
            .await
            .expect("client connects");
        let mut battery = SystemPowerdBattery::from_connection(connection);
        let error = battery
            .read_percentage()
            .await
            .expect_err("malformed reply rejected");
        assert!(matches!(
            &error,
            PowerdReadError::Dbus {
                operation: "decode powerd battery integer",
                ..
            }
        ));
        assert!(error
            .source()
            .expect("decode source retained")
            .is::<zbus::Error>());
        assert_eq!(
            battery
                .read_charging()
                .await
                .expect("optional malformed reply"),
            None
        );
    });
}

#[test]
fn a_missing_async_service_retains_read_context_and_the_dbus_source() {
    let (_bus, address) = PrivateTestBus::start();
    futures_lite::future::block_on(async {
        let connection = zbus::connection::Builder::address(address.as_str())
            .expect("private bus address")
            .build()
            .await
            .expect("client connects");
        let mut battery = SystemPowerdBattery::from_connection(connection);
        let error = battery
            .read_percentage()
            .await
            .expect_err("missing powerd rejected");
        assert!(matches!(
            &error,
            PowerdReadError::Dbus {
                operation: "read powerd battery integer",
                ..
            }
        ));
        assert!(error
            .source()
            .expect("transport source retained")
            .is::<zbus::Error>());
        assert_eq!(
            battery
                .read_charging()
                .await
                .expect("optional unavailable service"),
            None
        );
    });
}

struct FakePowerd;

impl LipcMethodTransport for FakePowerd {
    fn call_get_int(
        &self,
        endpoint: &LipcEndpoint<'_>,
        method: &str,
    ) -> Result<(u32, i32), LipcDbusError> {
        assert_eq!(endpoint.service, "com.lab126.powerd");
        assert_eq!(method, "getbattLevelInt");
        Ok((0, 83))
    }

    fn call_get_string(
        &self,
        _: &LipcEndpoint<'_>,
        _: &str,
    ) -> Result<(u32, String), LipcDbusError> {
        unreachable!("monitor reads only the battery integer")
    }

    fn call_set_int(&self, _: &LipcEndpoint<'_>, _: &str, _: i32) -> Result<u32, LipcDbusError> {
        unreachable!("monitor never changes powerd properties")
    }

    fn call_set_string(
        &self,
        _: &LipcEndpoint<'_>,
        _: &str,
        _: &str,
    ) -> Result<u32, LipcDbusError> {
        unreachable!("monitor never changes powerd properties")
    }
}

#[test]
fn reads_kindles_current_battery_level_from_powerd() {
    let mut battery = PowerdBattery::new(LipcDbusClient::from_transport(FakePowerd));

    assert_eq!(
        futures_lite::future::block_on(battery.read_percentage()).expect("battery"),
        83
    );
}
