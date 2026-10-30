//! Sampling triggers and packet sequencing through injected battery and broadcaster fakes.

use crate::bthome::BthomeSample;
use crate::monitor::Publication;
use crate::monitor::{BatterySource, Broadcaster, Monitor, Outcome, Trigger};
use pretty_assertions::assert_eq;
use std::collections::VecDeque;
use std::io;

struct FakeBattery {
    readings: VecDeque<i32>,
    reads: usize,
}

impl BatterySource for FakeBattery {
    type Error = io::Error;

    async fn read_charging(&mut self) -> Result<Option<bool>, Self::Error> {
        Ok(None)
    }

    async fn read_percentage(&mut self) -> Result<i32, Self::Error> {
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

    async fn broadcast(&mut self, sample: BthomeSample) -> Result<Publication, Self::Error> {
        if std::mem::take(&mut self.defer_next) {
            return Ok(Publication::Deferred);
        }
        self.samples.push(sample);
        Ok(Publication::Published)
    }
}

struct ScriptedChargingBattery {
    charging: VecDeque<Result<Option<bool>, io::Error>>,
    percentage_reads: usize,
    charging_reads: usize,
}

impl BatterySource for ScriptedChargingBattery {
    type Error = io::Error;

    async fn read_percentage(&mut self) -> Result<i32, Self::Error> {
        self.percentage_reads += 1;
        Ok(52)
    }

    async fn read_charging(&mut self) -> Result<Option<bool>, Self::Error> {
        self.charging_reads += 1;
        self.charging
            .pop_front()
            .expect("queued charging observation")
    }
}

#[test]
fn existing_triggers_include_fresh_charging_and_shutdown_reads_neither_property() {
    async_io::block_on(async {
        let mut monitor = Monitor::new(7);
        let mut battery = ScriptedChargingBattery {
            charging: VecDeque::from([Ok(Some(true)), Ok(Some(false)), Ok(None)]),
            percentage_reads: 0,
            charging_reads: 0,
        };
        let mut broadcaster = FakeBroadcaster::default();
        for trigger in [Trigger::Started, Trigger::BatteryChanged, Trigger::Refresh] {
            assert_eq!(
                monitor
                    .handle(trigger, &mut battery, &mut broadcaster)
                    .await
                    .expect("sample"),
                Outcome::Published
            );
        }
        assert_eq!(
            monitor
                .handle(Trigger::Shutdown, &mut battery, &mut broadcaster)
                .await
                .expect("shutdown"),
            Outcome::Stopped
        );
        assert_eq!((battery.percentage_reads, battery.charging_reads), (3, 3));
        assert_eq!(
            broadcaster
                .samples
                .iter()
                .map(|sample| sample.service_data())
                .collect::<Vec<_>>(),
            vec![
                vec![0xd2, 0xfc, 0x44, 0, 7, 1, 52, 0x16, 1],
                vec![0xd2, 0xfc, 0x44, 0, 8, 1, 52, 0x16, 0],
                vec![0xd2, 0xfc, 0x44, 0, 9, 1, 52],
            ]
        );
    });
}

#[test]
fn cancelled_charging_read_does_not_start_publication_or_advance_packet_id() {
    async_io::block_on(async {
        let mut monitor = Monitor::new(7);
        let mut battery = ScriptedChargingBattery {
            charging: VecDeque::from([Err(io::Error::other("cancelled")), Ok(Some(true))]),
            percentage_reads: 0,
            charging_reads: 0,
        };
        let mut broadcaster = FakeBroadcaster::default();
        assert!(monitor
            .handle(Trigger::Started, &mut battery, &mut broadcaster)
            .await
            .is_err());
        assert!(broadcaster.samples.is_empty());
        monitor
            .handle(Trigger::Refresh, &mut battery, &mut broadcaster)
            .await
            .expect("next sample");
        assert_eq!(
            broadcaster.samples[0].service_data(),
            [0xd2, 0xfc, 0x44, 0, 7, 1, 52, 0x16, 1]
        );
    });
}

#[test]
fn deferred_sample_is_not_counted_as_publication_and_next_event_recovers() {
    async_io::block_on(async {
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
                .await
                .expect("deferral"),
            Outcome::Deferred
        );
        assert_eq!(
            monitor
                .handle(Trigger::Refresh, &mut battery, &mut broadcaster)
                .await
                .expect("retry on event"),
            Outcome::Published
        );
        assert_eq!(
            broadcaster.samples[0].service_data(),
            [0xd2, 0xfc, 0x44, 0, 7, 1, 93]
        );
    });
}

#[test]
fn monitor_samples_only_for_start_change_and_refresh() {
    async_io::block_on(async {
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
                    .await
                    .expect("publish"),
                Outcome::Published
            );
        }
        assert_eq!(
            monitor
                .handle(Trigger::Shutdown, &mut battery, &mut broadcaster)
                .await
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
    });
}
