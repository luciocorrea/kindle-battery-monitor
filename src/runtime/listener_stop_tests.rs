//! Stop outcome caching and cancellation through the foreground cleanup seam.

use super::*;
use futures_lite::future;
use std::cell::Cell;
use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::rc::Rc;

struct PrivateBus(Child);

impl PrivateBus {
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
            .expect("private bus");
        let mut address = String::new();
        std::io::BufReader::new(child.stdout.take().expect("bus stdout"))
            .read_line(&mut address)
            .expect("bus address");
        (Self(child), address.trim().to_owned())
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct DropTrackedCleanup(Rc<Cell<usize>>);
impl Drop for DropTrackedCleanup {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn repeated_stop_reuses_success_or_failure_without_more_cleanup() {
    for fails in [false, true] {
        let (_bus, address) = PrivateBus::start();
        async_io::block_on(async {
            let connection = zbus::connection::Builder::address(address.as_str())
                .expect("address")
                .build()
                .await
                .expect("connection");
            let events = PowerdEvents::dedicated(connection)
                .await
                .expect("subscription");
            let (mut listener, changed) = EventListener::from_events(events);
            let calls = Cell::new(0);
            let first = listener
                .stop_with_cleanup(async |events, failures| {
                    calls.set(calls.get() + 1);
                    if fails {
                        drop(events);
                        failures.extend([
                            PowerdCleanupError::UnsubscribeTimeout,
                            PowerdCleanupError::CloseTimeout,
                        ]);
                    } else {
                        events
                            .shutdown_recording(
                                std::future::pending(),
                                std::future::pending(),
                                failures,
                            )
                            .await
                    }
                })
                .await;
            assert_eq!(first.is_err(), fails);
            let second = listener
                .stop_with_cleanup(async |_, _| panic!("repeated cleanup"))
                .await;
            assert_eq!(format!("{first:?}"), format!("{second:?}"));
            assert_eq!(calls.get(), 1);
            assert!(changed.is_closed());
            assert!(listener.events.is_none());
        });
    }
}

#[test]
fn cancelling_stop_releases_owned_state_and_records_interrupted_cleanup() {
    let (_bus, address) = PrivateBus::start();
    async_io::block_on(async {
        let connection = zbus::connection::Builder::address(address.as_str())
            .expect("address")
            .build()
            .await
            .expect("connection");
        let mut released = Box::pin(connection.clone().graceful_shutdown());
        assert!(future::poll_once(released.as_mut()).await.is_none());
        let events = PowerdEvents::dedicated(connection)
            .await
            .expect("subscription");
        let (mut listener, changed) = EventListener::from_events(events);
        listener.receive_error = Some(PowerdEventError::Closed);
        let drops = Rc::new(Cell::new(0));
        let tracked = DropTrackedCleanup(drops.clone());
        let mut stop = Box::pin(listener.stop_with_cleanup(async move |events, failures| {
            let _events = events;
            let _tracked = tracked;
            failures.push(PowerdCleanupError::Unsubscribe(zbus::Error::Failure(
                "known unsubscribe failure".into(),
            )));
            std::future::pending::<()>().await
        }));
        assert!(future::poll_once(stop.as_mut()).await.is_none());
        assert_eq!(drops.get(), 0);
        drop(stop);
        assert_eq!(drops.get(), 1);
        future::or(released, async {
            async_io::Timer::after(Duration::from_secs(3)).await;
            panic!("foreground connection retained after cancellation");
        })
        .await;
        assert!(changed.is_closed());
        let repeated = listener
            .stop_with_cleanup(async |_, _| panic!("cancelled stop retried IO"))
            .await;
        assert!(
            matches!(repeated, Err(EventListenerError::Shutdown(ref errors))
            if matches!(errors.as_slice(), [PowerdCleanupError::Receive(PowerdEventError::Closed), PowerdCleanupError::Unsubscribe(zbus::Error::Failure(message)), PowerdCleanupError::Interrupted] if message == "known unsubscribe failure"))
        );
        assert!(listener.events.is_none());
    });
}
