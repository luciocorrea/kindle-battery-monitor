//! Bounded, cancellable setup before any listener worker is created.

use super::*;
use std::cell::Cell;
use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::rc::Rc;

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
            .expect("private test bus");
        let stdout = child.stdout.take().expect("address output");
        let mut address = String::new();
        std::io::BufReader::new(stdout)
            .read_line(&mut address)
            .expect("private address");
        (Self(child), address.trim().to_owned())
    }
}

impl Drop for PrivateTestBus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct DropRecordingSubscription(Rc<Cell<usize>>);

impl Drop for DropRecordingSubscription {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

struct PendingConnect(DropRecordingSubscription);

impl PendingConnect {
    async fn setup(self) -> Result<DropRecordingSubscription, PowerdEventError> {
        let _connection = self.0;
        std::future::pending().await
    }
}

struct PendingSubscribe {
    owned: DropRecordingSubscription,
    connected: async_channel::Receiver<()>,
    subscribing: Rc<Cell<bool>>,
}

impl PendingSubscribe {
    async fn setup(self) -> Result<DropRecordingSubscription, PowerdEventError> {
        let _connection = self.owned;
        self.connected.recv().await.expect("connect gate");
        self.subscribing.set(true);
        std::future::pending().await
    }
}

#[test]
fn one_setup_budget_and_cancellation_cover_a_pending_subscription() {
    for cancel in [false, true] {
        async_io::block_on(async {
            let drops = Rc::new(Cell::new(0));
            let spawned = Cell::new(false);
            let subscribing = Rc::new(Cell::new(false));
            let (connect, connected) = async_channel::bounded::<()>(1);
            let (expire, expired) = async_channel::bounded::<()>(1);
            let (cancel_setup, cancelled) = async_channel::bounded::<()>(1);
            let setup = PendingSubscribe {
                owned: DropRecordingSubscription(drops.clone()),
                connected,
                subscribing: subscribing.clone(),
            };
            let mut start = Box::pin(finish_listener_setup(
                setup.setup(),
                async {
                    let _ = cancelled.recv().await;
                },
                async {
                    let _ = expired.recv().await;
                },
                |_| spawned.set(true),
            ));
            assert!(futures_lite::future::poll_once(start.as_mut())
                .await
                .is_none());
            assert!(!subscribing.get());
            connect.send(()).await.expect("complete connection");
            assert!(futures_lite::future::poll_once(start.as_mut())
                .await
                .is_none());
            assert!(subscribing.get());
            assert_eq!(drops.get(), 0);
            if cancel {
                cancel_setup.send(()).await.expect("cancel");
            } else {
                expire
                    .send(())
                    .await
                    .expect("expire original setup deadline");
            }
            let result = start.await;
            if cancel {
                assert!(matches!(result, Err(EventListenerError::SetupCancelled)));
            } else {
                assert!(matches!(result, Err(EventListenerError::SetupTimeout)));
            }
            assert_eq!(drops.get(), 1);
            assert!(!spawned.get());
        });
    }
}

#[test]
fn already_ready_cancellation_or_expiry_starts_no_setup_io() {
    for cancel in [false, true] {
        let started = Cell::new(false);
        let spawned = Cell::new(false);
        let result = async_io::block_on(finish_listener_setup(
            async {
                started.set(true);
                Ok::<_, PowerdEventError>(())
            },
            async {
                if !cancel {
                    std::future::pending::<()>().await;
                }
            },
            std::future::ready(()),
            |_| spawned.set(true),
        ));
        if cancel {
            assert!(matches!(result, Err(EventListenerError::SetupCancelled)));
        } else {
            assert!(matches!(result, Err(EventListenerError::SetupTimeout)));
        }
        assert!(!started.get());
        assert!(!spawned.get());
    }
    // The actual public entry point also prioritizes cancellation before opening
    // the system connection. This test supplies no bus or device configuration.
    assert!(matches!(
        async_io::block_on(EventListener::start_with_cancellation(std::future::ready(
            ()
        ))),
        Err(EventListenerError::SetupCancelled)
    ));
}

#[test]
fn cancellation_drops_a_pending_connection_before_returning() {
    async_io::block_on(async {
        let drops = Rc::new(Cell::new(0));
        let spawned = Cell::new(false);
        let (cancel, cancelled) = async_channel::bounded::<()>(1);
        let setup = PendingConnect(DropRecordingSubscription(drops.clone()));
        let mut start = Box::pin(finish_listener_setup(
            setup.setup(),
            async {
                let _ = cancelled.recv().await;
            },
            std::future::pending(),
            |_| spawned.set(true),
        ));
        assert!(futures_lite::future::poll_once(start.as_mut())
            .await
            .is_none());
        assert_eq!(drops.get(), 0);
        cancel.send(()).await.expect("cancel connection");
        assert!(matches!(
            start.await,
            Err(EventListenerError::SetupCancelled)
        ));
        assert_eq!(drops.get(), 1);
        assert!(!spawned.get());
    });
}

#[test]
fn successful_setup_spawns_once_after_acknowledgement_and_errors_keep_their_source() {
    async_io::block_on(async {
        let drops = Rc::new(Cell::new(0));
        let spawned = Cell::new(0);
        let (ack, acknowledged) = async_channel::bounded::<()>(1);
        let owned = DropRecordingSubscription(drops.clone());
        let mut start = Box::pin(finish_listener_setup(
            async {
                acknowledged
                    .recv()
                    .await
                    .expect("subscription acknowledgement");
                Ok(owned)
            },
            std::future::pending(),
            std::future::pending(),
            |subscription| {
                spawned.set(spawned.get() + 1);
                subscription
            },
        ));
        assert!(futures_lite::future::poll_once(start.as_mut())
            .await
            .is_none());
        assert_eq!(spawned.get(), 0);
        ack.send(()).await.expect("acknowledge subscription");
        let subscription = start.await.expect("setup succeeds");
        assert_eq!(spawned.get(), 1);
        assert_eq!(drops.get(), 0);
        drop(subscription);
        assert_eq!(drops.get(), 1);
    });
    let result = async_io::block_on(finish_listener_setup(
        std::future::ready(Err::<(), _>(PowerdEventError::Subscribe(
            zbus::Error::Failure("subscription denied".into()),
        ))),
        std::future::pending(),
        std::future::pending(),
        |_| panic!("failed setup must not spawn"),
    ));
    match result {
        Err(EventListenerError::Powerd(PowerdEventError::Subscribe(zbus::Error::Failure(
            message,
        )))) => assert_eq!(message, "subscription denied"),
        other => panic!("unexpected setup failure: {other:?}"),
    }
}

#[test]
fn actual_setup_timer_expires_without_a_device_event() {
    let result = async_io::block_on(futures_lite::future::or(
        finish_listener_setup(
            std::future::pending::<Result<(), PowerdEventError>>(),
            std::future::pending(),
            async {
                async_io::Timer::after(Duration::from_millis(10)).await;
            },
            |_| panic!("no completed setup"),
        ),
        async {
            async_io::Timer::after(Duration::from_secs(3)).await;
            Err(EventListenerError::ThreadPanicked)
        },
    ));
    assert!(matches!(result, Err(EventListenerError::SetupTimeout)));
}

#[test]
fn acknowledged_private_bus_setup_drives_notifications_in_the_caller_scope() {
    let (_bus, address) = PrivateTestBus::start();
    async_io::block_on(async {
        let publisher = zbus::connection::Builder::address(address.as_str())
            .expect("private address")
            .name("com.lab126.powerd")
            .expect("test publisher name")
            .build()
            .await
            .expect("publisher");
        let connection = zbus::connection::Builder::address(address.as_str())
            .expect("private address")
            .build()
            .await
            .expect("subscriber");
        let (mut listener, changed) = finish_listener_setup(
            PowerdEvents::dedicated(connection),
            std::future::pending(),
            async {
                async_io::Timer::after(Duration::from_secs(3)).await;
            },
            EventListener::from_events,
        )
        .await
        .expect("acknowledged setup before worker");
        let delivered = listener
            .run_scoped(
                async {
                    publisher
                        .emit_signal(
                            None::<&str>,
                            "/default",
                            "com.lab126.powerd",
                            "battLevelChanged",
                            &83_i32,
                        )
                        .await
                        .expect("private signal");
                    futures_lite::future::or(
                        async {
                            changed.recv().await.expect("notification");
                        },
                        async {
                            async_io::Timer::after(Duration::from_secs(3)).await;
                            panic!("notification watchdog");
                        },
                    )
                    .await;
                },
                || {},
            )
            .await;
        let stopped = listener.stop().await;
        delivered.expect("scoped listener");
        stopped.expect("normal listener stop");
    });
}

#[test]
fn setup_timeout_drops_partial_state_without_spawning_a_worker() {
    async_io::block_on(async {
        let drops = Rc::new(Cell::new(0));
        let spawned = Cell::new(false);
        let (expire, expired) = async_channel::bounded::<()>(1);
        let setup = PendingConnect(DropRecordingSubscription(drops.clone()));
        let mut start = Box::pin(finish_listener_setup(
            setup.setup(),
            std::future::pending(),
            async {
                let _ = expired.recv().await;
            },
            |_| spawned.set(true),
        ));
        assert!(futures_lite::future::poll_once(start.as_mut())
            .await
            .is_none());
        assert_eq!(drops.get(), 0);
        expire.send(()).await.expect("expire setup");
        assert!(matches!(start.await, Err(EventListenerError::SetupTimeout)));
        assert_eq!(drops.get(), 1);
        assert!(!spawned.get());
    });
}
