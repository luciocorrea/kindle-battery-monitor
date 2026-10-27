//! Event-driven powerd notifications on an isolated host DBus.

use futures_lite::future::block_on;
use kindle_battery_monitor::powerd_events::PowerdEvents;
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
fn powerd_change_signal_is_received_without_polling() {
    let (_bus, address) = PrivateTestBus::start();
    block_on(async {
        let publisher = zbus::connection::Builder::address(address.as_str())
            .expect("private bus address")
            .name("com.lab126.powerd")
            .expect("powerd name")
            .build()
            .await
            .expect("publisher connects");
        let subscriber = zbus::connection::Builder::address(address.as_str())
            .expect("private bus address")
            .build()
            .await
            .expect("subscriber connects");
        let mut events = PowerdEvents::from_connection(subscriber)
            .await
            .expect("subscription acknowledged");

        publisher
            .emit_signal(
                None::<&str>,
                "/default",
                "com.lab126.powerd",
                "battLevelChanged",
                &83_i32,
            )
            .await
            .expect("change event sent");

        events.next().await.expect("battery change received");
    });
}
