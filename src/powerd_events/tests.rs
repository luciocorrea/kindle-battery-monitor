//! Event-driven powerd notifications on an isolated host DBus.

use crate::powerd_events::PowerdEvents;
use futures_lite::future::block_on;
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
fn charging_notifications_trigger_sampling_for_shared_and_dedicated_subscriptions() {
    for dedicated in [false, true] {
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
            let mut events = if dedicated {
                PowerdEvents::dedicated(subscriber).await
            } else {
                PowerdEvents::from_connection(subscriber).await
            }
            .expect("subscription acknowledged");
            for member in [
                "charging",
                "notCharging",
                "battStateInfoChanged",
                "battLevelChanged",
            ] {
                // Signal bodies are notifications, never authoritative samples.
                publisher
                    .emit_signal(None::<&str>, "/default", "com.lab126.powerd", member, &())
                    .await
                    .expect("notification sent");
                futures_lite::future::or(
                    async {
                        events.next().await.expect("sampling notification");
                    },
                    async {
                        async_io::Timer::after(std::time::Duration::from_secs(3)).await;
                        panic!("missing notification: {member}, dedicated={dedicated}");
                    },
                )
                .await;
            }
            if dedicated {
                assert!(events
                    .shutdown_with_deadlines(std::future::pending(), std::future::pending())
                    .await
                    .is_empty());
            }
        });
    }
}

#[test]
fn unrelated_signal_headers_and_method_calls_do_not_trigger_sampling() {
    use futures_lite::{future, StreamExt};
    let (_bus, address) = PrivateTestBus::start();
    block_on(future::or(
        async {
            let publisher = zbus::connection::Builder::address(address.as_str())
                .expect("private bus address")
                .build()
                .await
                .expect("publisher");
            let subscriber = zbus::connection::Builder::address(address.as_str())
                .expect("private bus address")
                .build()
                .await
                .expect("subscriber");
            let destination = subscriber
                .unique_name()
                .expect("subscriber name")
                .to_owned();
            let mut events = PowerdEvents::dedicated(subscriber)
                .await
                .expect("subscription");
            // This independent cursor witnesses delivery on the actual socket without
            // consuming the subscription's messages or depending on a fixed sleep.
            let mut observed = events.stream.clone();
            for (path, interface, member, method_call) in [
                ("/other", "com.lab126.powerd", "charging", false),
                ("/default", "com.example.Unrelated", "charging", false),
                ("/default", "com.lab126.powerd", "wakeupFromSuspend", false),
                ("/default", "com.lab126.powerd", "isCharging", false),
                ("/default", "com.lab126.powerd", "Charging", false),
                ("/default", "com.lab126.powerd", "charging", true),
            ] {
                if method_call {
                    let message = zbus::Message::method_call(path, member)
                        .expect("method header")
                        .interface(interface)
                        .expect("interface")
                        .destination(destination.as_str())
                        .expect("destination")
                        .build(&())
                        .expect("method body");
                    publisher
                        .send(&message)
                        .await
                        .expect("unrelated method sent");
                } else {
                    publisher
                        .emit_signal(Some(destination.as_str()), path, interface, member, &())
                        .await
                        .expect("unrelated signal sent");
                }
                publisher
                    .emit_signal(
                        Some(destination.as_str()),
                        "/default",
                        "com.lab126.powerd",
                        "battLevelChanged",
                        &(),
                    )
                    .await
                    .expect("matching marker sent");
                loop {
                    let message = observed
                        .next()
                        .await
                        .expect("observation stream")
                        .expect("message");
                    if message.header().member().map(|member| member.as_str())
                        == Some("battLevelChanged")
                    {
                        break;
                    }
                }
                events
                    .next()
                    .await
                    .expect("one sampling trigger for matching marker");
                assert!(future::poll_once(events.next()).await.is_none(),
                "unrelated message was accepted before marker: {path}, {interface}, {member}, method={method_call}");
            }
            drop(observed);
            assert!(events
                .shutdown_with_deadlines(future::pending(), future::pending())
                .await
                .is_empty());
        },
        async {
            async_io::Timer::after(std::time::Duration::from_secs(4)).await;
            panic!("notification filtering watchdog");
        },
    ));
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
