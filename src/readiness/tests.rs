//! Event-driven startup readiness, timeout, and cancellation contracts.

use crate::readiness::{wait_for_services, ReadinessError, ReadinessProtocol};
use std::{collections::VecDeque, io, time::Duration};

struct FakeServices {
    snapshots: VecDeque<[bool; 2]>,
    subscribed: bool,
    changes: usize,
}

impl ReadinessProtocol for FakeServices {
    type Error = io::Error;
    async fn subscribe(&mut self) -> Result<(), Self::Error> {
        self.subscribed = true;
        Ok(())
    }
    async fn owners(&mut self) -> Result<[bool; 2], Self::Error> {
        assert!(
            self.subscribed,
            "registration must be observed before the snapshot"
        );
        Ok(self.snapshots.pop_front().expect("scripted snapshot"))
    }
    async fn changed(&mut self) -> Result<(), Self::Error> {
        self.changes += 1;
        if self.snapshots.is_empty() {
            std::future::pending().await
        } else {
            Ok(())
        }
    }
}

#[test]
fn waits_for_missing_service_and_handles_registration_before_snapshot() {
    for snapshots in [
        VecDeque::from([[true, true]]),
        VecDeque::from([[false, true], [true, true]]),
    ] {
        let expected_changes = snapshots.len() - 1;
        let mut services = FakeServices {
            snapshots,
            subscribed: false,
            changes: 0,
        };
        async_io::block_on(wait_for_services(
            &mut services,
            std::future::pending(),
            Duration::from_secs(1),
        ))
        .expect("services ready");
        assert_eq!(services.changes, expected_changes);
    }
}

#[test]
fn missing_service_times_out_and_cancellation_wins() {
    let mut services = FakeServices {
        snapshots: VecDeque::from([[false, false]]),
        subscribed: false,
        changes: 0,
    };
    let result = async_io::block_on(wait_for_services(
        &mut services,
        std::future::pending(),
        Duration::from_millis(10),
    ));
    assert!(matches!(result, Err(ReadinessError::Timeout)));
    let result = async_io::block_on(wait_for_services(
        &mut services,
        async {},
        Duration::from_secs(1),
    ));
    assert!(matches!(result, Err(ReadinessError::Cancelled)));
}

use std::io::BufRead;
use std::process::{Child, Command, Stdio};
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

#[test]
fn dbus_adapter_observes_late_registration_and_disconnect() {
    use crate::readiness::DbusReadiness;
    let (_bus, address) = PrivateTestBus::start();
    async_io::block_on(async {
        let subscriber = zbus::connection::Builder::address(address.as_str())
            .expect("address")
            .build()
            .await
            .expect("subscriber");
        let mut readiness = DbusReadiness::new(subscriber.clone());
        readiness.subscribe().await.expect("match acknowledged");
        assert_eq!(
            readiness.owners().await.expect("initial snapshot"),
            [false, false]
        );
        let publisher = zbus::connection::Builder::address(address.as_str())
            .expect("address")
            .build()
            .await
            .expect("publisher");
        publisher
            .request_name("com.lab126.powerd")
            .await
            .expect("register powerd");
        publisher
            .request_name("com.lab126.btfd")
            .await
            .expect("register btfd");
        readiness.changed().await.expect("owner event");
        assert_eq!(
            readiness.owners().await.expect("ready snapshot"),
            [true, true]
        );
        wait_for_services(
            &mut readiness,
            std::future::pending(),
            Duration::from_secs(1),
        )
        .await
        .expect("ready");
        subscriber.close().await.expect("disconnect");
        assert!(readiness.owners().await.is_err());
    });
}
