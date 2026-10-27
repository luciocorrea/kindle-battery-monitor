//! Source-4 acquisition and release while preserving competing radio ownership.

use kindle_battery_monitor::bthome::{BatteryLevel, BthomeSample};
use kindle_battery_monitor::monitor::{Broadcaster, Publication};
use kindle_battery_monitor::ownership::{AdvertisementBurst, OwnedBroadcaster, RadioPower};
use kindle_battery_monitor::radio::RadioState;
use pretty_assertions::assert_eq;
use std::io;

#[derive(Default)]
struct FakeRadio {
    mask: u32,
    backend_present: bool,
    competing_source_on_enable: bool,
    state_calls: usize,
    competing_source_on_cleanup: bool,
    fail_cleanup_inspection: bool,
    enable_calls: usize,
    disable_calls: usize,
}

impl RadioPower for FakeRadio {
    type Error = io::Error;

    fn state(&mut self) -> Result<RadioState, Self::Error> {
        self.state_calls += 1;
        if self.state_calls == 3 {
            if self.competing_source_on_cleanup {
                self.mask |= 1;
            }
            if self.fail_cleanup_inspection {
                return Err(io::Error::other("cleanup inspection failed"));
            }
        }
        Ok(if self.backend_present || self.mask != 0 {
            RadioState::EnabledSources(self.mask)
        } else {
            RadioState::DaemonAbsent
        })
    }

    fn enable_source4(&mut self) -> Result<(), Self::Error> {
        self.enable_calls += 1;
        self.mask |= if self.competing_source_on_enable {
            5
        } else {
            4
        };
        Ok(())
    }

    fn disable_source4(&mut self) -> Result<(), Self::Error> {
        self.disable_calls += 1;
        self.mask &= !4;
        Ok(())
    }
}

#[derive(Default)]
struct FakeAdvertisement {
    samples: Vec<BthomeSample>,
    fail: bool,
}

impl AdvertisementBurst for FakeAdvertisement {
    type Error = io::Error;

    fn broadcast(&mut self, sample: BthomeSample) -> Result<Publication, Self::Error> {
        if self.fail {
            return Err(io::Error::other("advertisement rejected"));
        }
        self.samples.push(sample);
        Ok(Publication::Published)
    }
}

#[test]
fn an_existing_radio_owner_defers_without_mutation() {
    let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
    for mask in [0, 1, 4, 5] {
        let mut radio = FakeRadio {
            mask,
            backend_present: true,
            ..FakeRadio::default()
        };
        let mut advertisement = FakeAdvertisement::default();
        let outcome = OwnedBroadcaster::new(&mut radio, &mut advertisement)
            .broadcast(sample)
            .expect("deferred");
        assert_eq!(outcome, Publication::Deferred);
        assert_eq!(
            (radio.enable_calls, radio.disable_calls, radio.mask),
            (0, 0, mask)
        );
        assert!(advertisement.samples.is_empty());
    }
}

#[test]
fn a_competing_source_after_acquisition_defers_and_releases_only_our_source() {
    let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
    let mut radio = FakeRadio {
        competing_source_on_enable: true,
        ..FakeRadio::default()
    };
    let mut advertisement = FakeAdvertisement::default();
    let outcome = OwnedBroadcaster::new(&mut radio, &mut advertisement)
        .broadcast(sample)
        .expect("deferred");
    assert_eq!(outcome, Publication::Deferred);
    assert_eq!(radio.mask, 1);
    assert_eq!(radio.disable_calls, 1);
    assert!(advertisement.samples.is_empty());
}

#[test]
fn rejected_advertisement_still_releases_owned_source() {
    let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
    let mut radio = FakeRadio::default();
    let mut advertisement = FakeAdvertisement {
        fail: true,
        ..FakeAdvertisement::default()
    };

    assert!(OwnedBroadcaster::new(&mut radio, &mut advertisement)
        .broadcast(sample)
        .is_err());
    assert_eq!(radio.enable_calls, 1);
    assert_eq!(radio.disable_calls, 1);
    assert_eq!(radio.mask, 0);
}

#[test]
fn burst_releases_only_the_stock_radio_source_it_acquired() {
    let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
    let mut radio = FakeRadio::default();
    let mut advertisement = FakeAdvertisement::default();

    OwnedBroadcaster::new(&mut radio, &mut advertisement)
        .broadcast(sample)
        .expect("owned burst");

    assert_eq!(radio.enable_calls, 1);
    assert_eq!(radio.disable_calls, 1);
    assert_eq!(radio.mask, 0);
    assert_eq!(advertisement.samples, vec![sample]);
}

#[test]
fn a_competing_owner_during_the_burst_survives_cleanup() {
    let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
    let mut radio = FakeRadio {
        competing_source_on_cleanup: true,
        ..FakeRadio::default()
    };
    let mut advertisement = FakeAdvertisement::default();
    assert_eq!(
        OwnedBroadcaster::new(&mut radio, &mut advertisement)
            .broadcast(sample)
            .expect("published"),
        Publication::Published
    );
    assert_eq!(radio.mask, 1);
    assert_eq!(radio.disable_calls, 1);
}

#[test]
fn cleanup_inspection_failure_is_reported_but_owned_source_is_released() {
    let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
    let mut radio = FakeRadio {
        fail_cleanup_inspection: true,
        ..FakeRadio::default()
    };
    let mut advertisement = FakeAdvertisement::default();
    let error = OwnedBroadcaster::new(&mut radio, &mut advertisement)
        .broadcast(sample)
        .expect_err("inspection failed");
    assert!(error.to_string().contains("cleanup inspection failed"));
    assert_eq!(radio.mask, 0);
    assert_eq!(radio.disable_calls, 1);
}
