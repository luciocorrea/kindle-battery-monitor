//! Blocking runtime selection between battery events and shutdown.

use crossbeam_channel::bounded;
use kindle_battery_monitor::monitor::{EventSource, Trigger};
use kindle_battery_monitor::runtime::RuntimeEvents;
use pretty_assertions::assert_eq;
use std::time::Duration;

#[test]
fn runtime_waits_for_battery_event_or_shutdown() {
    let (battery_tx, battery_rx) = bounded(1);
    let (shutdown_tx, shutdown_rx) = bounded(1);
    let mut events = RuntimeEvents::new(battery_rx, shutdown_rx, Duration::from_secs(600));

    battery_tx.send(()).expect("battery event queued");
    assert_eq!(
        events.next().expect("battery event"),
        Trigger::BatteryChanged
    );

    shutdown_tx.send(()).expect("shutdown queued");
    assert_eq!(events.next().expect("shutdown"), Trigger::Shutdown);
}
