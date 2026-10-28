//! Session composition through real sampling, ownership and advertisement policy.

use super::*;
use crate::broadcast::{run_broadcast, BroadcastRadio, EndReason, WaitForEnd};
use crate::bthome::BthomeSample;
use crate::monitor::Publication;
use crate::readiness::{wait_for_services, ReadinessProtocol};
use errors::*;
use futures_lite::future;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

#[derive(Clone)]
struct Observations {
    trace: Rc<RefCell<Vec<&'static str>>>,
    sent: Sender<&'static str>,
    actions: Receiver<&'static str>,
    samples: Rc<RefCell<Vec<BthomeSample>>>,
}
impl Observations {
    fn new() -> Self {
        let (sent, actions) = bounded(128);
        Self {
            trace: Rc::default(),
            sent,
            actions,
            samples: Rc::default(),
        }
    }
    fn record(&self, action: &'static str) {
        self.trace.borrow_mut().push(action);
        self.sent.try_send(action).expect("bounded test trace");
    }
    async fn until(&self, expected: &str) {
        while self.actions.recv().await.expect("action stream") != expected {}
    }
}

struct ScriptedReadiness {
    observed: Observations,
    owners: [bool; 2],
}
impl ReadinessProtocol for ScriptedReadiness {
    type Error = zbus::Error;
    async fn subscribe(&mut self) -> Result<(), Self::Error> {
        self.observed.record("subscribe-ready");
        Ok(())
    }
    async fn owners(&mut self) -> Result<[bool; 2], Self::Error> {
        self.observed.record("owners-ready");
        Ok(self.owners)
    }
    async fn changed(&mut self) -> Result<(), Self::Error> {
        std::future::pending().await
    }
}
struct GatedBattery {
    observed: Observations,
    values: Rc<RefCell<VecDeque<Result<i32, PowerdReadError>>>>,
    gate: Option<Receiver<()>>,
    cancelled: Receiver<()>,
}
impl BatterySource for GatedBattery {
    type Error = PowerdReadError;
    async fn read_percentage(&mut self) -> Result<i32, Self::Error> {
        self.observed.record("read-battery");
        future::or(
            async {
                let _ = self.cancelled.recv().await;
                Err(PowerdReadError::Cancelled)
            },
            async {
                if let Some(gate) = self.gate.take() {
                    let _ = gate.recv().await;
                }
                self.values.borrow_mut().pop_front().unwrap_or(Ok(97))
            },
        )
        .await
    }
}
struct RecordingRadio {
    observed: Observations,
    state: RadioState,
}
impl RadioPower for RecordingRadio {
    type Error = RadioError;
    async fn state(&mut self) -> Result<RadioState, Self::Error> {
        self.observed.record("state");
        Ok(self.state)
    }
    async fn enable_source4(&mut self) -> Result<(), Self::Error> {
        self.observed.record("enable");
        self.state = RadioState::EnabledSources(4);
        Ok(())
    }
    async fn disable_source4(&mut self) -> Result<(), Self::Error> {
        self.observed.record("disable");
        self.state = RadioState::DaemonAbsent;
        Ok(())
    }
}
struct RecordingAdvertisement {
    observed: Observations,
    fail_configure: bool,
}
impl BroadcastRadio for RecordingAdvertisement {
    type Error = BsaRadioError;
    async fn ensure_available(&mut self) -> Result<(), Self::Error> {
        self.observed.record("preflight");
        Ok(())
    }
    async fn configure(&mut self, sample: BthomeSample) -> Result<(), Self::Error> {
        self.observed.record("configure");
        self.observed.samples.borrow_mut().push(sample);
        if self.fail_configure {
            Err(BsaRadioError::Config(ConfigError::Status(9)))
        } else {
            Ok(())
        }
    }
    async fn start(&mut self) -> Result<(), Self::Error> {
        self.observed.record("start");
        Ok(())
    }
    async fn stop(&mut self) -> Result<(), Self::Error> {
        self.observed.record("stop");
        Ok(())
    }
    async fn verify_stopped(&mut self) -> Result<(), Self::Error> {
        self.observed.record("verify");
        Ok(())
    }
    async fn restore_identity(&mut self) -> Result<(), Self::Error> {
        self.observed.record("restore");
        Ok(())
    }
}
struct ManualDeadline {
    cancelled: Receiver<()>,
    elapsed: Receiver<()>,
}
impl WaitForEnd for ManualDeadline {
    async fn wait(self) -> EndReason {
        future::or(
            async {
                let _ = self.cancelled.recv().await;
                EndReason::Cancelled
            },
            async {
                self.elapsed.recv().await.expect("manual deadline");
                EndReason::Deadline
            },
        )
        .await
    }
}
struct GatedAdvertisement {
    radio: RecordingAdvertisement,
    cancelled: Receiver<()>,
    elapsed: Receiver<()>,
}
impl AdvertisementBurst for GatedAdvertisement {
    type Error = BsaBurstError;
    async fn broadcast(&mut self, sample: BthomeSample) -> Result<Publication, Self::Error> {
        if self.cancelled.try_recv().is_ok() {
            return Ok(Publication::Cancelled);
        }
        crate::runtime::publication_result(
            run_broadcast(
                &mut self.radio,
                sample,
                ManualDeadline {
                    cancelled: self.cancelled.clone(),
                    elapsed: self.elapsed.clone(),
                },
            )
            .await,
        )
    }
}
type Notifications = std::pin::Pin<Box<Receiver<Result<(), PowerdEventError>>>>;

struct ScriptedListener {
    observed: Observations,
    notifications: Option<Notifications>,
    changed: Option<Sender<()>>,
    received: Option<PowerdEventError>,
    cleanup: Vec<PowerdCleanupError>,
}
impl SessionListener for ScriptedListener {
    async fn drive<T>(
        &mut self,
        operation: impl Future<Output = T>,
        cancel: impl FnOnce(),
    ) -> Result<T, EventListenerError> {
        self.observed.record("drive-listener");
        let observed = self.observed.clone();
        let notifications = futures_lite::StreamExt::map(
            self.notifications.take().expect("one driver"),
            move |event| {
                observed.record("notification");
                event
            },
        );
        Ok(crate::runtime::drive_listener(
            notifications,
            self.changed.take().expect("trigger sender"),
            &mut self.received,
            operation,
            cancel,
        )
        .await)
    }
    async fn stop(&mut self) -> Result<(), EventListenerError> {
        self.observed.record("listener-stop");
        let mut failures: Vec<_> = self
            .received
            .take()
            .map(PowerdCleanupError::Receive)
            .into_iter()
            .collect();
        failures.extend(self.cleanup.clone());
        if failures.is_empty() {
            Ok(())
        } else {
            Err(EventListenerError::Shutdown(failures))
        }
    }
}
impl Drop for ScriptedListener {
    fn drop(&mut self) {
        self.observed.record("listener-drop");
    }
}
struct ScriptedIo {
    observed: Observations,
    values: Rc<RefCell<VecDeque<Result<i32, PowerdReadError>>>>,
    battery_gate: Option<Receiver<()>>,
    initial: RadioState,
    fail_configure: bool,
    cleanup: Vec<PowerdCleanupError>,
    listener_error: Option<EventListenerError>,
    listener_gate: Option<Receiver<()>>,
    owners: [bool; 2],
    notify: Sender<Result<(), PowerdEventError>>,
    notifications: Receiver<Result<(), PowerdEventError>>,
    expire: Sender<()>,
    expired: Receiver<()>,
    refresh: Sender<std::time::Instant>,
    refreshed: Receiver<std::time::Instant>,
}
impl ScriptedIo {
    fn new() -> Self {
        let (notify, notifications) = bounded(16);
        let (expire, expired) = bounded(16);
        let (refresh, refreshed) = bounded(1);
        Self {
            observed: Observations::new(),
            values: Rc::default(),
            battery_gate: None,
            initial: RadioState::DaemonAbsent,
            fail_configure: false,
            cleanup: Vec::new(),
            listener_error: None,
            listener_gate: None,
            owners: [true, true],
            notify,
            notifications,
            expire,
            expired,
            refresh,
            refreshed,
        }
    }
}
impl SessionIo for ScriptedIo {
    type Battery = GatedBattery;
    type Radio = RecordingRadio;
    type Advertisement = GatedAdvertisement;
    type Listener = ScriptedListener;
    async fn ready(
        &mut self,
        cancelled: Receiver<()>,
        timeout: Duration,
    ) -> Result<(), StartupError> {
        assert_eq!(timeout, Duration::from_secs(30));
        wait_for_services(
            &mut ScriptedReadiness {
                observed: self.observed.clone(),
                owners: self.owners,
            },
            async {
                let _ = cancelled.recv().await;
            },
            timeout,
        )
        .await
    }
    async fn battery(&mut self, cancelled: Receiver<()>) -> Result<Self::Battery, PowerdReadError> {
        self.observed.record("connect-battery");
        Ok(GatedBattery {
            observed: self.observed.clone(),
            values: self.values.clone(),
            gate: self.battery_gate.take(),
            cancelled,
        })
    }
    fn radio(&mut self) -> Self::Radio {
        self.observed.record("construct-radio");
        RecordingRadio {
            observed: self.observed.clone(),
            state: self.initial,
        }
    }
    fn advertisement(&mut self, cancelled: Receiver<()>) -> Self::Advertisement {
        self.observed.record("construct-advertisement");
        GatedAdvertisement {
            radio: RecordingAdvertisement {
                observed: self.observed.clone(),
                fail_configure: self.fail_configure,
            },
            cancelled,
            elapsed: self.expired.clone(),
        }
    }
    fn now(&mut self) -> SystemTime {
        self.observed.record("clock");
        UNIX_EPOCH + Duration::from_secs(258)
    }
    async fn listener(
        &mut self,
        cancelled: Receiver<()>,
    ) -> Result<(Self::Listener, Receiver<()>), EventListenerError> {
        self.observed.record("listen");
        if let Some(gate) = self.listener_gate.take() {
            future::or(
                async {
                    let _ = cancelled.recv().await;
                    Err(EventListenerError::SetupCancelled)
                },
                async {
                    gate.recv().await.expect("listener setup gate");
                    Ok(())
                },
            )
            .await?;
        }
        if let Some(error) = self.listener_error.take() {
            return Err(error);
        }
        let (changed, receiver) = bounded(1);
        Ok((
            ScriptedListener {
                observed: self.observed.clone(),
                notifications: Some(Box::pin(self.notifications.clone())),
                changed: Some(changed),
                received: None,
                cleanup: self.cleanup.clone(),
            },
            receiver,
        ))
    }
    fn events(
        &mut self,
        changed: Receiver<()>,
        shutdown: Receiver<()>,
        interval: Duration,
    ) -> Result<RuntimeEvents, InvalidRefreshInterval> {
        assert_eq!(interval, Duration::from_secs(600));
        self.observed.record("events");
        RuntimeEvents::with_timer(changed, shutdown, interval, |_| {
            Box::pin(self.refreshed.clone())
        })
    }
}
async fn watchdog<T>(work: impl Future<Output = T>) -> T {
    future::or(work, async {
        async_io::Timer::after(Duration::from_secs(3)).await;
        panic!("session test watchdog");
    })
    .await
}
async fn alongside<T>(work: impl Future<Output = T>, actions: impl Future<Output = ()>) -> T {
    watchdog(future::or(work, async {
        actions.await;
        std::future::pending().await
    }))
    .await
}

#[test]
fn once_session_owns_the_transaction_and_preserves_packet_zero_without_listener_or_clock() {
    async_io::block_on(async {
        let mut io = ScriptedIo::new();
        io.expire.try_send(()).expect("elapsed burst");
        let observed = io.observed.clone();
        let session = Session::new(RunMode::Once);
        drop(session.cancellation());
        assert!(observed.trace.borrow().is_empty());
        assert_eq!(
            session.run_with(&mut io).await.expect("once publication"),
            SessionOutcome::Published
        );
        assert_eq!(
            &*observed.trace.borrow(),
            &[
                "subscribe-ready",
                "owners-ready",
                "connect-battery",
                "construct-radio",
                "construct-advertisement",
                "read-battery",
                "state",
                "enable",
                "state",
                "preflight",
                "configure",
                "start",
                "stop",
                "verify",
                "restore",
                "state",
                "disable"
            ]
        );
        assert_eq!(
            observed.samples.borrow()[0].service_data(),
            [0xd2, 0xfc, 0x44, 0, 0, 1, 97]
        );
    });
}

#[test]
fn once_defers_for_an_existing_owner_without_acquiring_or_constructing_a_listener() {
    async_io::block_on(async {
        let mut io = ScriptedIo::new();
        io.initial = RadioState::EnabledSources(4);
        let observed = io.observed.clone();
        assert_eq!(
            Session::new(RunMode::Once)
                .run_with(&mut io)
                .await
                .expect("deferred"),
            SessionOutcome::Deferred
        );
        assert_eq!(observed.trace.borrow().last(), Some(&"state"));
        assert!(!observed.trace.borrow().contains(&"enable"));
        assert!(observed.samples.borrow().is_empty());
    });
}

#[test]
fn once_read_failure_or_invalid_percentage_does_not_touch_the_radio() {
    for invalid in [false, true] {
        async_io::block_on(async {
            let mut io = ScriptedIo::new();
            io.values.borrow_mut().push_back(if invalid {
                Ok(101)
            } else {
                Err(PowerdReadError::Dbus {
                    operation: "scripted read",
                    source: zbus::Error::Failure("battery unavailable".into()),
                })
            });
            let observed = io.observed.clone();
            let error = Session::new(RunMode::Once)
                .run_with(&mut io)
                .await
                .expect_err("sample rejected");
            if invalid {
                assert!(
                    matches!(error, SessionError::Once(error) if matches!(*error, MonitorError::InvalidBattery(_)))
                );
            } else {
                assert!(
                    matches!(error, SessionError::Once(error) if matches!(*error, MonitorError::Battery(_)))
                );
            }
            assert!(!observed.trace.borrow().contains(&"state"));
            assert!(!observed.trace.borrow().contains(&"listen"));
        });
    }
}

#[test]
fn once_cancellation_during_read_or_publication_returns_stopped_after_needed_cleanup() {
    for during_read in [true, false] {
        async_io::block_on(async {
            let mut io = ScriptedIo::new();
            let (_release, gate) = bounded(1);
            if during_read {
                io.battery_gate = Some(gate);
            }
            let observed = io.observed.clone();
            let session = Session::new(RunMode::Once);
            let cancellation = session.cancellation();
            let result = alongside(session.run_with(&mut io), async {
                observed
                    .until(if during_read { "read-battery" } else { "start" })
                    .await;
                cancellation.request();
                cancellation.request();
            })
            .await
            .expect("cancelled once");
            assert_eq!(result, SessionOutcome::Stopped);
            if during_read {
                assert!(!observed.trace.borrow().contains(&"enable"));
            } else {
                assert!(observed
                    .trace
                    .borrow()
                    .ends_with(&["stop", "verify", "restore", "state", "disable"]));
            }
            assert!(!observed.trace.borrow().contains(&"listen"));
        });
    }
}

#[test]
fn continuous_coalesces_notifications_during_owned_publication_and_advances_packet_ids() {
    async_io::block_on(async {
        let mut io = ScriptedIo::new();
        let observed = io.observed.clone();
        let expire = io.expire.clone();
        let notify = io.notify.clone();
        let session = Session::new(RunMode::Continuous);
        let cancellation = session.cancellation();
        let result = alongside(session.run_with(&mut io), async {
            observed.until("start").await;
            for _ in 0..4 {
                notify.send(Ok(())).await.expect("battery notification");
            }
            for _ in 0..4 {
                observed.until("notification").await;
            }
            expire.send(()).await.expect("first burst elapsed");
            observed.until("start").await;
            cancellation.request();
        })
        .await
        .expect("continuous cancellation");
        assert_eq!(result, SessionOutcome::Stopped);
        let packets: Vec<_> = observed
            .samples
            .borrow()
            .iter()
            .map(|sample| sample.service_data()[4])
            .collect();
        assert_eq!(packets, [2, 3]);
        let trace = observed.trace.borrow();
        assert!(trace.starts_with(&[
            "subscribe-ready",
            "owners-ready",
            "connect-battery",
            "construct-radio",
            "construct-advertisement",
            "clock",
            "listen",
            "events",
            "drive-listener",
            "read-battery"
        ]));
        assert_eq!(
            trace
                .iter()
                .filter(|action| **action == "read-battery")
                .count(),
            2
        );
        assert!(trace.ends_with(&[
            "stop",
            "verify",
            "restore",
            "state",
            "disable",
            "listener-stop",
            "listener-drop"
        ]));
    });
}

#[test]
fn continuous_read_errors_and_invalid_levels_wait_for_the_next_trigger_without_consuming_packet_ids(
) {
    async_io::block_on(async {
        let mut io = ScriptedIo::new();
        io.values.borrow_mut().extend([
            Err(PowerdReadError::Dbus {
                operation: "scripted read",
                source: zbus::Error::Failure("battery unavailable".into()),
            }),
            Ok(101),
            Ok(97),
        ]);
        let observed = io.observed.clone();
        let refresh = io.refresh.clone();
        let notify = io.notify.clone();
        let expire = io.expire.clone();
        let session = Session::new(RunMode::Continuous);
        let cancellation = session.cancellation();
        let result = alongside(session.run_with(&mut io), async {
            observed.until("read-battery").await;
            refresh
                .send(std::time::Instant::now())
                .await
                .expect("controlled refresh");
            observed.until("read-battery").await;
            notify.send(Ok(())).await.expect("controlled notification");
            observed.until("start").await;
            expire.send(()).await.expect("burst elapsed");
            observed.until("disable").await;
            cancellation.request();
        })
        .await
        .expect("normal termination");
        assert_eq!(result, SessionOutcome::Stopped);
        assert_eq!(observed.samples.borrow().len(), 1);
        assert_eq!(observed.samples.borrow()[0].service_data()[4], 2);
        assert_eq!(
            observed
                .trace
                .borrow()
                .iter()
                .filter(|action| **action == "read-battery")
                .count(),
            3
        );
    });
}

#[test]
fn listener_failure_cancels_pending_read_or_finishes_owned_publication_before_teardown() {
    for during_read in [true, false] {
        async_io::block_on(async {
            let mut io = ScriptedIo::new();
            let (_release, gate) = bounded(1);
            if during_read {
                io.battery_gate = Some(gate);
            }
            let observed = io.observed.clone();
            let notify = io.notify.clone();
            let result = alongside(Session::new(RunMode::Continuous).run_with(&mut io), async {
                observed
                    .until(if during_read { "read-battery" } else { "start" })
                    .await;
                notify
                    .send(Err(PowerdEventError::Receive(zbus::Error::Failure(
                        "listener failed".into(),
                    ))))
                    .await
                    .expect("receive failure");
            })
            .await
            .expect_err("listener failure is terminal");
            if during_read {
                assert!(
                    matches!(result, SessionError::Combined { primary, listener }
                    if matches!(*primary, RunError::Event(RuntimeEventError::BatteryListenerStopped))
                    && matches!(*listener, EventListenerError::Shutdown(ref errors) if matches!(errors.as_slice(), [PowerdCleanupError::Receive(_)])))
                );
                assert!(!observed.trace.borrow().contains(&"enable"));
            } else {
                assert!(
                    matches!(result, SessionError::Listener(EventListenerError::Shutdown(ref errors)) if matches!(errors.as_slice(), [PowerdCleanupError::Receive(_)]))
                );
                assert!(observed.trace.borrow().ends_with(&[
                    "stop",
                    "verify",
                    "restore",
                    "state",
                    "disable",
                    "listener-stop",
                    "listener-drop"
                ]));
            }
        });
    }
}

#[test]
fn session_retains_monitor_and_listener_failures_together() {
    async_io::block_on(async {
        use std::error::Error;
        let mut io = ScriptedIo::new();
        io.fail_configure = true;
        io.cleanup = vec![
            PowerdCleanupError::UnsubscribeTimeout,
            PowerdCleanupError::CloseTimeout,
        ];
        let observed = io.observed.clone();
        let error = Session::new(RunMode::Continuous)
            .run_with(&mut io)
            .await
            .expect_err("both failures");
        assert!(error.source().is_some(), "primary source retained");
        match error {
            SessionError::Combined { primary, listener } => {
                assert!(matches!(
                    *primary,
                    RunError::Publication(OwnedBroadcastError::Advertisement {
                        source: BsaBurstError::Broadcast(
                            crate::broadcast::BroadcastError::Configure {
                                source: BsaRadioError::Config(ConfigError::Status(9)),
                                ..
                            }
                        ),
                        ..
                    })
                ));
                assert!(
                    matches!(*listener, EventListenerError::Shutdown(ref failures) if matches!(failures.as_slice(), [PowerdCleanupError::UnsubscribeTimeout, PowerdCleanupError::CloseTimeout]))
                );
            }
            other => panic!("lost session failures: {other:?}"),
        }
        assert!(observed.trace.borrow().ends_with(&[
            "stop",
            "verify",
            "restore",
            "state",
            "disable",
            "listener-stop",
            "listener-drop"
        ]));
    });
}

#[test]
fn listener_setup_failure_starts_no_sample_or_radio_transaction() {
    async_io::block_on(async {
        let mut io = ScriptedIo::new();
        io.listener_error = Some(EventListenerError::SetupTimeout);
        let observed = io.observed.clone();
        assert!(matches!(
            Session::new(RunMode::Continuous).run_with(&mut io).await,
            Err(SessionError::Listener(EventListenerError::SetupTimeout))
        ));
        assert_eq!(observed.trace.borrow().last(), Some(&"listen"));
        assert!(!observed.trace.borrow().contains(&"read-battery"));
        assert!(!observed.trace.borrow().contains(&"state"));
    });
}

#[test]
fn construction_cancellation_and_handle_drop_never_start_adapter_work() {
    let mut io = ScriptedIo::new();
    let observed = io.observed.clone();
    let unpolled = Session::new(RunMode::Continuous);
    drop(unpolled.cancellation());
    drop(unpolled);
    assert!(observed.trace.borrow().is_empty());
    let session = Session::new(RunMode::Continuous);
    let cancel = session.cancellation();
    cancel.request();
    cancel.request();
    assert!(matches!(
        async_io::block_on(session.run_with(&mut io)),
        Err(SessionError::Readiness(ReadinessError::Cancelled))
    ));
    cancel.request();
    assert!(observed.trace.borrow().is_empty());
}

#[test]
fn bounded_smoke_uses_a_known_packet_id() {
    async_io::block_on(async {
        let mut io = ScriptedIo::new();
        io.expire.try_send(()).expect("elapsed");
        Session::new(RunMode::Once)
            .run_with(&mut io)
            .await
            .expect("once");
        assert_eq!(io.observed.samples.borrow()[0].service_data()[4], 0);
    });
}

#[test]
fn cancellation_reaches_pending_readiness_and_listener_setup_before_sampling() {
    for during_readiness in [true, false] {
        async_io::block_on(async {
            let mut io = ScriptedIo::new();
            let (_release, gate) = bounded(1);
            if during_readiness {
                io.owners = [false, false];
            } else {
                io.listener_gate = Some(gate);
            }
            let observed = io.observed.clone();
            let session = Session::new(RunMode::Continuous);
            let cancellation = session.cancellation();
            let error = alongside(session.run_with(&mut io), async {
                observed
                    .until(if during_readiness {
                        "owners-ready"
                    } else {
                        "listen"
                    })
                    .await;
                cancellation.request();
            })
            .await
            .expect_err("startup cancellation retains its existing stage result");
            if during_readiness {
                assert!(matches!(
                    error,
                    SessionError::Readiness(ReadinessError::Cancelled)
                ));
            } else {
                assert!(matches!(
                    error,
                    SessionError::Listener(EventListenerError::SetupCancelled)
                ));
            }
            let trace = observed.trace.borrow();
            assert!(!trace.contains(&"read-battery"));
            assert!(!trace.contains(&"enable"));
            assert!(!trace.contains(&"listener-stop"));
        });
    }
}
