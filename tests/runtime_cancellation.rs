//! Cancellation before a burst avoids opening the BSA socket.

use kindle_battery_monitor::bthome::{BatteryLevel, BthomeSample};
use kindle_battery_monitor::monitor::Publication;
use kindle_battery_monitor::ownership::AdvertisementBurst;
use kindle_battery_monitor::runtime::BsaAdvertisement;
use pretty_assertions::assert_eq;

#[test]
fn cancellation_before_configuration_never_opens_the_bsa_socket() {
    let (cancel, cancelled) = crossbeam_channel::bounded(1);
    cancel.send(()).expect("cancel queued");
    let mut advertisement = BsaAdvertisement::new("/nonexistent/bsa-socket".into(), cancelled);
    let sample = BthomeSample::new(BatteryLevel::try_from(99).expect("percentage"), 1);
    assert_eq!(
        advertisement.broadcast(sample).expect("clean cancellation"),
        Publication::Cancelled
    );
}
