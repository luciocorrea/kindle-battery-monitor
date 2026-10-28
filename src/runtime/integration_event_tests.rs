//! Async runtime selection, interval validation, and real timer delivery.
use crate::monitor::{EventSource, Trigger};
use crate::runtime::RuntimeEvents;
use async_channel::bounded;
use std::time::Duration;

#[test]
fn zero_refresh_interval_is_rejected_at_construction() {
    let (_battery_tx, battery_rx) = bounded(1);
    let (_shutdown_tx, shutdown_rx) = bounded(1);
    assert!(RuntimeEvents::new(battery_rx, shutdown_rx, Duration::ZERO).is_err());
}
#[test]
fn an_unrepresentable_refresh_interval_retains_the_rejected_value() {
    let (_battery_tx, battery_rx) = bounded(1);
    let (_shutdown_tx, shutdown_rx) = bounded(1);
    let error = RuntimeEvents::new(battery_rx, shutdown_rx, Duration::MAX)
        .err()
        .expect("unrepresentable interval rejected");
    assert_eq!(error.interval, Duration::MAX);
}
#[test]
fn a_positive_timer_delivers_a_refresh_without_device_io() {
    async_io::block_on(async {
        let (_battery_tx, battery_rx) = bounded(1);
        let (_shutdown, shutdown_rx) = bounded(1);
        let mut events = RuntimeEvents::new(battery_rx, shutdown_rx, Duration::from_millis(10))
            .expect("short interval");
        let result = futures_lite::future::or(events.next(), async {
            async_io::Timer::after(Duration::from_secs(3)).await;
            panic!("refresh watchdog");
        })
        .await;
        assert_eq!(result.expect("refresh"), Trigger::Refresh);
    });
}
#[test]
fn runtime_waits_for_battery_event_or_shutdown() {
    async_io::block_on(async {
        let (battery_tx, battery_rx) = bounded(1);
        let (shutdown_tx, shutdown_rx) = bounded(1);
        let mut events = RuntimeEvents::new(battery_rx, shutdown_rx, Duration::from_secs(600))
            .expect("valid production interval");
        battery_tx.try_send(()).expect("battery event queued");
        assert_eq!(
            events.next().await.expect("battery event"),
            Trigger::BatteryChanged
        );
        shutdown_tx.try_send(()).expect("shutdown queued");
        assert_eq!(events.next().await.expect("shutdown"), Trigger::Shutdown);
    });
}
