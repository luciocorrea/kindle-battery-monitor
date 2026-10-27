//! Sampling triggers and packet sequencing through injected battery and broadcaster fakes.

use kindle_battery_monitor::bthome::BthomeSample;
use kindle_battery_monitor::monitor::Publication;
use kindle_battery_monitor::monitor::{BatterySource, Broadcaster, Monitor, Outcome, Trigger};
use pretty_assertions::assert_eq;
use std::collections::VecDeque;
use std::io;

struct FakeBattery {
    readings: VecDeque<i32>,
    reads: usize,
}

impl BatterySource for FakeBattery {
    type Error = io::Error;

    fn read_percentage(&mut self) -> Result<i32, Self::Error> {
        self.reads += 1;
        self.readings
            .pop_front()
            .ok_or_else(|| io::Error::other("no reading queued"))
    }
}

#[derive(Default)]
struct FakeBroadcaster {
    samples: Vec<BthomeSample>,
    defer_next: bool,
}

impl Broadcaster for FakeBroadcaster {
    type Error = io::Error;

    fn broadcast(&mut self, sample: BthomeSample) -> Result<Publication, Self::Error> {
        if std::mem::take(&mut self.defer_next) {
            return Ok(Publication::Deferred);
        }
        self.samples.push(sample);
        Ok(Publication::Published)
    }
}

#[test]
fn deferred_sample_is_not_counted_as_publication_and_next_event_recovers() {
    let mut monitor = Monitor::new(7);
    let mut battery = FakeBattery {
        readings: VecDeque::from([94, 93]),
        reads: 0,
    };
    let mut broadcaster = FakeBroadcaster {
        defer_next: true,
        ..FakeBroadcaster::default()
    };
    assert_eq!(
        monitor
            .handle(Trigger::Started, &mut battery, &mut broadcaster)
            .expect("deferral"),
        Outcome::Deferred
    );
    assert_eq!(
        monitor
            .handle(Trigger::Refresh, &mut battery, &mut broadcaster)
            .expect("retry on event"),
        Outcome::Published
    );
    assert_eq!(
        broadcaster.samples[0].service_data(),
        [0xd2, 0xfc, 0x44, 0, 7, 1, 93]
    );
}

#[test]
fn monitor_samples_only_for_start_change_and_refresh() {
    let mut monitor = Monitor::new(7);
    let mut battery = FakeBattery {
        readings: VecDeque::from([96, 83, 83]),
        reads: 0,
    };
    let mut broadcaster = FakeBroadcaster::default();

    for event in [Trigger::Started, Trigger::BatteryChanged, Trigger::Refresh] {
        assert_eq!(
            monitor
                .handle(event, &mut battery, &mut broadcaster)
                .expect("publish"),
            Outcome::Published
        );
    }
    assert_eq!(
        monitor
            .handle(Trigger::Shutdown, &mut battery, &mut broadcaster)
            .expect("stop"),
        Outcome::Stopped
    );
    assert_eq!(battery.reads, 3);
    assert_eq!(
        broadcaster
            .samples
            .iter()
            .map(|sample| sample.service_data())
            .collect::<Vec<_>>(),
        vec![
            [0xd2, 0xfc, 0x44, 0x00, 7, 0x01, 96],
            [0xd2, 0xfc, 0x44, 0x00, 8, 0x01, 83],
            [0xd2, 0xfc, 0x44, 0x00, 9, 0x01, 83],
        ]
    );
}
