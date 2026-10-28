//! Continuous monitor recovery and shutdown behavior through injected event sources.

use crate::bthome::BthomeSample;
use crate::monitor::Publication;
use crate::monitor::{run_monitor, BatterySource, Broadcaster, EventSource, Monitor, Trigger};
use pretty_assertions::assert_eq;
use std::collections::VecDeque;
use std::io;

struct FakeBattery {
    readings: VecDeque<Result<i32, io::Error>>,
    reads: usize,
}

impl BatterySource for FakeBattery {
    type Error = io::Error;

    async fn read_percentage(&mut self) -> Result<i32, Self::Error> {
        self.reads += 1;
        self.readings.pop_front().expect("queued reading")
    }
}

#[derive(Default)]
struct FakeBroadcaster {
    samples: Vec<BthomeSample>,
    fail: bool,
    cancelled: bool,
}

impl Broadcaster for FakeBroadcaster {
    type Error = io::Error;

    async fn broadcast(&mut self, sample: BthomeSample) -> Result<Publication, Self::Error> {
        if self.fail {
            return Err(io::Error::other("radio cleanup unverified"));
        }
        if self.cancelled {
            return Ok(Publication::Cancelled);
        }
        self.samples.push(sample);
        Ok(Publication::Published)
    }
}

#[test]
fn publication_failure_stops_before_another_burst() {
    async_io::block_on(async {
        let mut monitor = Monitor::new(7);
        let mut battery = FakeBattery {
            readings: VecDeque::from([Ok(83)]),
            reads: 0,
        };
        let mut broadcaster = FakeBroadcaster {
            fail: true,
            ..FakeBroadcaster::default()
        };
        let mut events = FakeEvents {
            events: VecDeque::from([Trigger::Refresh, Trigger::Shutdown]),
            waits: 0,
        };

        assert!(
            run_monitor(&mut monitor, &mut battery, &mut broadcaster, &mut events)
                .await
                .is_err()
        );
        assert_eq!(events.waits, 0);
        assert_eq!(broadcaster.samples.len(), 0);
    });
}

struct FakeEvents {
    events: VecDeque<Trigger>,
    waits: usize,
}

impl EventSource for FakeEvents {
    type Error = io::Error;

    async fn next(&mut self) -> Result<Trigger, Self::Error> {
        self.waits += 1;
        self.events
            .pop_front()
            .ok_or_else(|| io::Error::other("no event queued"))
    }
}

#[test]
fn failed_sample_is_not_broadcast_and_next_scheduled_event_recovers() {
    async_io::block_on(async {
        let mut monitor = Monitor::new(7);
        let mut battery = FakeBattery {
            readings: VecDeque::from([
                Ok(83),
                Err(io::Error::other("powerd temporarily unavailable")),
                Ok(82),
            ]),
            reads: 0,
        };
        let mut broadcaster = FakeBroadcaster::default();
        let mut events = FakeEvents {
            events: VecDeque::from([Trigger::BatteryChanged, Trigger::Refresh, Trigger::Shutdown]),
            waits: 0,
        };

        run_monitor(&mut monitor, &mut battery, &mut broadcaster, &mut events)
            .await
            .expect("graceful shutdown");

        assert_eq!(battery.reads, 3);
        assert_eq!(events.waits, 3);
        assert_eq!(
            broadcaster
                .samples
                .iter()
                .map(|sample| sample.service_data())
                .collect::<Vec<_>>(),
            vec![
                [0xd2, 0xfc, 0x44, 0x00, 7, 0x01, 83],
                [0xd2, 0xfc, 0x44, 0x00, 8, 0x01, 82],
            ]
        );
    });
}

#[test]
fn a_clean_cancelled_burst_finishes_the_monitor_without_waiting_again() {
    async_io::block_on(async {
        let mut monitor = Monitor::new(7);
        let mut battery = FakeBattery {
            readings: VecDeque::from([Ok(83)]),
            reads: 0,
        };
        let mut broadcaster = FakeBroadcaster {
            cancelled: true,
            ..FakeBroadcaster::default()
        };
        let mut events = FakeEvents {
            events: VecDeque::new(),
            waits: 0,
        };
        run_monitor(&mut monitor, &mut battery, &mut broadcaster, &mut events)
            .await
            .expect("clean cancellation");
        assert_eq!(events.waits, 0);
    });
}
