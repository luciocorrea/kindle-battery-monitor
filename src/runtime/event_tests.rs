//! Refresh validation and event-priority behavior through a controlled timer.

use super::*;

struct RecordingRefreshTimer {
    calls: Vec<Duration>,
    sender: async_channel::Sender<Instant>,
    receiver: Receiver<Instant>,
}

impl RecordingRefreshTimer {
    fn new() -> Self {
        let (sender, receiver) = bounded(1);
        Self {
            calls: Vec::new(),
            sender,
            receiver,
        }
    }

    fn create(&mut self, interval: Duration) -> RefreshStream {
        self.calls.push(interval);
        Box::pin(self.receiver.clone())
    }
}

#[test]
fn invalid_intervals_never_reach_the_timer_factory() {
    async_io::block_on(async {
        for interval in [Duration::ZERO, Duration::MAX] {
            let (_battery, battery_rx) = bounded(1);
            let (_shutdown, shutdown_rx) = bounded(1);
            let mut timer = RecordingRefreshTimer::new();
            let error = RuntimeEvents::with_timer(battery_rx, shutdown_rx, interval, |value| {
                timer.create(value)
            })
            .err()
            .expect("invalid interval rejected");
            assert_eq!(error, InvalidRefreshInterval { interval });
            assert!(timer.calls.is_empty());
        }
    });
}

#[test]
fn valid_intervals_create_exactly_one_unchanged_timer() {
    async_io::block_on(async {
        for interval in [
            Duration::from_nanos(1),
            Duration::from_secs(1),
            Duration::from_secs(600),
        ] {
            let (_battery, battery_rx) = bounded(1);
            let (_shutdown, shutdown_rx) = bounded(1);
            let mut timer = RecordingRefreshTimer::new();
            let result = RuntimeEvents::with_timer(battery_rx, shutdown_rx, interval, |value| {
                timer.create(value)
            });
            assert!(result.is_ok());
            assert_eq!(timer.calls, [interval]);
        }
    });
}

#[test]
fn controlled_ready_events_preserve_shutdown_battery_refresh_priority() {
    async_io::block_on(async {
        for (shutdown_ready, battery_ready, expected) in [
            (true, true, Trigger::Shutdown),
            (false, true, Trigger::BatteryChanged),
            (false, false, Trigger::Refresh),
        ] {
            let (battery, battery_rx) = bounded(1);
            let (shutdown, shutdown_rx) = bounded(1);
            let mut timer = RecordingRefreshTimer::new();
            let mut events = RuntimeEvents::with_timer(
                battery_rx,
                shutdown_rx,
                Duration::from_secs(600),
                |value| timer.create(value),
            )
            .expect("valid interval");
            timer
                .sender
                .try_send(Instant::now())
                .expect("refresh queued");
            if battery_ready {
                battery.try_send(()).expect("battery queued");
            }
            if shutdown_ready {
                shutdown.try_send(()).expect("shutdown queued");
            }
            assert_eq!(events.next().await.expect("event"), expected);
        }
    });
}

#[test]
fn disconnection_keeps_shutdown_priority_and_reports_a_lost_battery_listener() {
    async_io::block_on(async {
        for shutdown_disconnected in [false, true] {
            let (battery, battery_rx) = bounded(1);
            let (shutdown, shutdown_rx) = bounded(1);
            let mut timer = RecordingRefreshTimer::new();
            let mut events = RuntimeEvents::with_timer(
                battery_rx,
                shutdown_rx,
                Duration::from_secs(600),
                |value| timer.create(value),
            )
            .expect("valid interval");
            drop(battery);
            if shutdown_disconnected {
                drop(shutdown);
                assert_eq!(events.next().await.expect("shutdown"), Trigger::Shutdown);
            } else {
                assert!(matches!(
                    events.next().await,
                    Err(RuntimeEventError::BatteryListenerStopped)
                ));
            }
        }
    });
}

#[test]
fn construction_waits_for_a_controlled_refresh_then_shutdown() {
    async_io::block_on(async {
        let (_battery, battery_rx) = bounded(1);
        let (shutdown, shutdown_rx) = bounded(1);
        let mut timer = RecordingRefreshTimer::new();
        let mut events =
            RuntimeEvents::with_timer(battery_rx, shutdown_rx, Duration::from_secs(600), |value| {
                timer.create(value)
            })
            .expect("valid interval");
        assert!(futures_lite::future::poll_once(events.next())
            .await
            .is_none());
        timer
            .sender
            .try_send(Instant::now())
            .expect("release timer");
        assert_eq!(events.next().await.expect("refresh"), Trigger::Refresh);
        assert!(futures_lite::future::poll_once(events.next())
            .await
            .is_none());
        shutdown.try_send(()).expect("cancel wait");
        assert_eq!(events.next().await.expect("shutdown"), Trigger::Shutdown);
    });
}

#[test]
fn listener_failure_prevents_a_buffered_trigger_starting_another_publication() {
    async_io::block_on(async {
        let (battery, battery_rx) = bounded(1);
        let (shutdown, shutdown_rx) = bounded(1);
        let mut timer = RecordingRefreshTimer::new();
        let mut events =
            RuntimeEvents::with_timer(battery_rx, shutdown_rx, Duration::from_secs(600), |value| {
                timer.create(value)
            })
            .expect("interval");
        battery
            .try_send(())
            .expect("already queued battery trigger");
        timer
            .sender
            .try_send(Instant::now())
            .expect("refresh ready");
        drop(battery);
        assert!(matches!(
            events.next().await,
            Err(RuntimeEventError::BatteryListenerStopped)
        ));
        shutdown.try_send(()).expect("shutdown still wins");
        assert_eq!(events.next().await.expect("shutdown"), Trigger::Shutdown);
    });
}
