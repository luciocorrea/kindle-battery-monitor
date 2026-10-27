//! Exact trigger-based BTHome service bytes for a validated battery sample.

use kindle_battery_monitor::bthome::{BatteryLevel, BthomeSample};
use pretty_assertions::assert_eq;

#[test]
fn trigger_based_battery_sample_has_exact_bthome_service_data() {
    let level = BatteryLevel::try_from(52).expect("valid percentage");
    let sample = BthomeSample::new(level, 42);

    assert_eq!(
        sample.service_data(),
        [0xd2, 0xfc, 0x44, 0x00, 42, 0x01, 52]
    );
}
