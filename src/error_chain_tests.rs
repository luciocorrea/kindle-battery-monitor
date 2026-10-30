//! Typed error-source chains through monitor, advertisement, and ownership boundaries.

use crate::broadcast::{run_broadcast, BroadcastError, BroadcastRadio, EndReason, WaitForEnd};
use crate::bthome::{BatteryLevel, BthomeSample};
use crate::monitor::{
    run_monitor, BatterySource, Broadcaster, EventSource, Monitor, MonitorError, Publication,
    RunError, Trigger,
};
use crate::ownership::{AdvertisementBurst, OwnedBroadcastError, OwnedBroadcaster, RadioPower};
use crate::radio::RadioState;
use std::error::Error;
use thiserror::Error;

#[derive(Debug, Eq, Error, PartialEq)]
#[error("{0}")]
struct FakeBatteryError(&'static str);

#[derive(Debug, Eq, Error, PartialEq)]
#[error("{0}")]
struct FakeTransportError(&'static str);

#[derive(Debug, Eq, Error, PartialEq)]
#[error("{0}")]
struct FakeEventError(&'static str);

#[derive(Default)]
struct FakeEvents {
    waits: usize,
}

impl EventSource for FakeEvents {
    type Error = FakeEventError;

    async fn next(&mut self) -> Result<Trigger, Self::Error> {
        self.waits += 1;
        Err(FakeEventError("event stream closed"))
    }
}

#[derive(Default)]
struct FakeBroadcastRadio {
    fail_at: Option<&'static str>,
    cleanup_fails: bool,
    calls: Vec<&'static str>,
}

impl FakeBroadcastRadio {
    fn operation(&mut self, name: &'static str) -> Result<(), FakeTransportError> {
        self.calls.push(name);
        if self.fail_at == Some(name)
            || (self.cleanup_fails && ["stop", "verify", "restore"].contains(&name))
        {
            Err(FakeTransportError(name))
        } else {
            Ok(())
        }
    }
}

impl BroadcastRadio for FakeBroadcastRadio {
    type Error = FakeTransportError;

    async fn ensure_available(&mut self) -> Result<(), Self::Error> {
        self.operation("preflight")
    }

    async fn configure(&mut self, _sample: BthomeSample) -> Result<(), Self::Error> {
        self.operation("configure")
    }

    async fn start(&mut self) -> Result<(), Self::Error> {
        self.operation("start")
    }

    async fn stop(&mut self) -> Result<(), Self::Error> {
        self.operation("stop")
    }

    async fn verify_stopped(&mut self) -> Result<(), Self::Error> {
        self.operation("verify")
    }

    async fn restore_identity(&mut self) -> Result<(), Self::Error> {
        self.operation("restore")
    }
}

struct FakeElapsedDeadline;

impl WaitForEnd for FakeElapsedDeadline {
    async fn wait(self) -> EndReason {
        EndReason::Deadline
    }
}

struct FakeAdvertisement<'a>(&'a mut FakeBroadcastRadio);

impl Broadcaster for FakeAdvertisement<'_> {
    type Error = BroadcastError<FakeTransportError>;

    async fn broadcast(&mut self, sample: BthomeSample) -> Result<Publication, Self::Error> {
        run_broadcast(self.0, sample, FakeElapsedDeadline)
            .await
            .map(|_| Publication::Published)
    }
}

impl AdvertisementBurst for FakeAdvertisement<'_> {
    type Error = BroadcastError<FakeTransportError>;

    async fn broadcast(&mut self, sample: BthomeSample) -> Result<Publication, Self::Error> {
        run_broadcast(self.0, sample, FakeElapsedDeadline)
            .await
            .map(|_| Publication::Published)
    }
}

#[derive(Default)]
struct FakeRadioPower {
    initial_inspection_fails: bool,
    cleanup_fails: bool,
    inspections: usize,
    calls: Vec<&'static str>,
}

impl RadioPower for FakeRadioPower {
    type Error = FakeTransportError;

    async fn state(&mut self) -> Result<RadioState, Self::Error> {
        self.calls.push("inspect");
        self.inspections += 1;
        match self.inspections {
            1 if self.initial_inspection_fails => Err(FakeTransportError("initial ownership")),
            1 => Ok(RadioState::DaemonAbsent),
            3 if self.cleanup_fails => Err(FakeTransportError("cleanup ownership")),
            _ => Ok(RadioState::EnabledSources(4)),
        }
    }

    async fn enable_source4(&mut self) -> Result<(), Self::Error> {
        self.calls.push("acquire");
        Ok(())
    }

    async fn disable_source4(&mut self) -> Result<(), Self::Error> {
        self.calls.push("release");
        if self.cleanup_fails {
            Err(FakeTransportError("source release"))
        } else {
            Ok(())
        }
    }
}

type FakeOwnedError = OwnedBroadcastError<FakeTransportError, BroadcastError<FakeTransportError>>;

#[derive(Default)]
struct FakeBattery {
    failure: Option<FakeBatteryError>,
    reads: usize,
}

impl BatterySource for FakeBattery {
    type Error = FakeBatteryError;

    async fn read_charging(&mut self) -> Result<Option<bool>, Self::Error> {
        Ok(None)
    }

    async fn read_percentage(&mut self) -> Result<i32, Self::Error> {
        self.reads += 1;
        self.failure.take().map_or(Ok(83), Err)
    }
}

#[derive(Default)]
struct FakeSuccessfulPublisher {
    calls: usize,
}

impl Broadcaster for FakeSuccessfulPublisher {
    type Error = FakeTransportError;

    async fn broadcast(&mut self, _sample: BthomeSample) -> Result<Publication, Self::Error> {
        self.calls += 1;
        Ok(Publication::Published)
    }
}

#[test]
fn monitor_battery_failure_exposes_the_original_typed_cause_before_publication() {
    async_io::block_on(async {
        let mut battery = FakeBattery {
            failure: Some(FakeBatteryError("battery channel closed")),
            ..FakeBattery::default()
        };
        let mut publisher = FakeSuccessfulPublisher::default();
        let error = Monitor::new(7)
            .handle(Trigger::Started, &mut battery, &mut publisher)
            .await
            .expect_err("battery read fails");

        assert!(matches!(error, MonitorError::Battery(_)));
        assert_eq!(
            error.to_string(),
            "battery read failed: battery channel closed"
        );
        let cause = error
            .source()
            .expect("battery error has an underlying source");
        assert_eq!(
            cause.downcast_ref::<FakeBatteryError>(),
            Some(&FakeBatteryError("battery channel closed"))
        );
        assert_eq!(battery.reads, 1);
        assert_eq!(publisher.calls, 0);
    });
}

#[test]
fn monitor_publication_chain_reaches_preflight_failure_without_mutating_the_radio() {
    async_io::block_on(async {
        let mut battery = FakeBattery::default();
        let mut radio = FakeBroadcastRadio {
            fail_at: Some("preflight"),
            ..FakeBroadcastRadio::default()
        };
        let error = Monitor::new(7)
            .handle(
                Trigger::Started,
                &mut battery,
                &mut FakeAdvertisement(&mut radio),
            )
            .await
            .expect_err("radio preflight fails");

        assert!(matches!(error, MonitorError::Broadcast(_)));
        assert_eq!(
            error.to_string(),
            "battery broadcast failed: radio preflight failed: preflight"
        );
        let publication = error.source().expect("monitor retains publication source");
        assert!(matches!(
            publication.downcast_ref::<BroadcastError<FakeTransportError>>(),
            Some(BroadcastError::Unavailable(_))
        ));
        let cause = publication
            .source()
            .expect("preflight retains transport source");
        assert_eq!(
            cause.downcast_ref::<FakeTransportError>(),
            Some(&FakeTransportError("preflight"))
        );
        assert_eq!(radio.calls, ["preflight"]);
        assert_eq!(battery.reads, 1);
    });
}

#[test]
fn session_event_failure_exposes_the_original_typed_cause_after_startup_publication() {
    async_io::block_on(async {
        let mut monitor = Monitor::new(7);
        let mut battery = FakeBattery::default();
        let mut publisher = FakeSuccessfulPublisher::default();
        let mut events = FakeEvents::default();
        let error = run_monitor(&mut monitor, &mut battery, &mut publisher, &mut events)
            .await
            .expect_err("event source fails");

        assert!(matches!(error, RunError::Event(_)));
        assert_eq!(
            error.to_string(),
            "monitor event source failed: event stream closed"
        );
        let cause = error.source().expect("session retains event-source error");
        assert_eq!(
            cause.downcast_ref::<FakeEventError>(),
            Some(&FakeEventError("event stream closed"))
        );
        assert_eq!((battery.reads, publisher.calls, events.waits), (1, 1, 1));
    });
}

#[test]
fn session_publication_chain_reaches_transport_cause_and_retains_all_owned_cleanup_errors() {
    async_io::block_on(async {
        let mut monitor = Monitor::new(7);
        let mut battery = FakeBattery::default();
        let mut events = FakeEvents::default();
        let mut power = FakeRadioPower {
            cleanup_fails: true,
            ..FakeRadioPower::default()
        };
        let mut radio = FakeBroadcastRadio {
            fail_at: Some("preflight"),
            ..FakeBroadcastRadio::default()
        };
        let error = run_monitor(
            &mut monitor,
            &mut battery,
            &mut OwnedBroadcaster::new(&mut power, &mut FakeAdvertisement(&mut radio)),
            &mut events,
        )
        .await
        .expect_err("publication and ownership cleanup fail");

        assert!(matches!(error, RunError::Publication(_)));
        assert_eq!(
            error.to_string(),
            "battery publication failed: advertisement failed: radio preflight failed: preflight; \
             radio cleanup errors: [FakeTransportError(\"cleanup ownership\"), \
             FakeTransportError(\"source release\")]"
        );
        let ownership = error.source().expect("session retains publication source");
        let owned = ownership
            .downcast_ref::<FakeOwnedError>()
            .expect("typed ownership error");
        match owned {
            OwnedBroadcastError::Advertisement { cleanup_errors, .. } => assert_eq!(
                cleanup_errors,
                &[
                    FakeTransportError("cleanup ownership"),
                    FakeTransportError("source release")
                ]
            ),
            other => panic!("expected advertisement failure, received {other:?}"),
        }
        let publication = ownership
            .source()
            .expect("ownership retains advertisement source");
        assert!(matches!(
            publication.downcast_ref::<BroadcastError<FakeTransportError>>(),
            Some(BroadcastError::Unavailable(_))
        ));
        let cause = publication
            .source()
            .expect("preflight retains transport source");
        assert_eq!(
            cause.downcast_ref::<FakeTransportError>(),
            Some(&FakeTransportError("preflight"))
        );
        assert_eq!(
            power.calls,
            ["inspect", "acquire", "inspect", "inspect", "release"]
        );
        assert_eq!(radio.calls, ["preflight"]);
        assert_eq!((battery.reads, events.waits), (1, 0));
    });
}

#[test]
fn monitor_inspection_chain_reaches_transport_source_without_acquiring_or_advertising() {
    async_io::block_on(async {
        let mut battery = FakeBattery::default();
        let mut power = FakeRadioPower {
            initial_inspection_fails: true,
            ..FakeRadioPower::default()
        };
        let mut radio = FakeBroadcastRadio::default();
        let error = Monitor::new(7)
            .handle(
                Trigger::Started,
                &mut battery,
                &mut OwnedBroadcaster::new(&mut power, &mut FakeAdvertisement(&mut radio)),
            )
            .await
            .expect_err("initial ownership inspection fails");

        assert!(matches!(error, MonitorError::Broadcast(_)));
        assert_eq!(
            error.to_string(),
            "battery broadcast failed: stock BLE source read failed: initial ownership"
        );
        let ownership = error.source().expect("monitor retains ownership failure");
        assert!(matches!(
            ownership.downcast_ref::<FakeOwnedError>(),
            Some(OwnedBroadcastError::Inspect(_))
        ));
        let cause = ownership
            .source()
            .expect("inspection retains transport source");
        assert_eq!(
            cause.downcast_ref::<FakeTransportError>(),
            Some(&FakeTransportError("initial ownership"))
        );
        assert_eq!(power.calls, ["inspect"]);
        assert!(radio.calls.is_empty());
    });
}

#[test]
fn configuration_and_start_failures_retain_primary_source_and_every_cleanup_failure_in_order() {
    async_io::block_on(async {
        let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("valid percentage"), 42);
        for operation in ["configure", "start"] {
            let mut radio = FakeBroadcastRadio {
                fail_at: Some(operation),
                cleanup_fails: true,
                ..FakeBroadcastRadio::default()
            };
            let error = run_broadcast(&mut radio, sample, FakeElapsedDeadline)
                .await
                .expect_err("primary operation and all cleanup operations fail");

            let cause = error
                .source()
                .expect("primary transport error remains the source");
            assert_eq!(
                cause.downcast_ref::<FakeTransportError>(),
                Some(&FakeTransportError(operation))
            );
            let cleanup_errors = match &error {
                BroadcastError::Configure { cleanup_errors, .. } if operation == "configure" => {
                    cleanup_errors
                }
                BroadcastError::Start { cleanup_errors, .. } if operation == "start" => {
                    cleanup_errors
                }
                other => panic!("expected primary operation failure, received {other:?}"),
            };
            assert_eq!(
                cleanup_errors,
                &[
                    FakeTransportError("stop"),
                    FakeTransportError("verify"),
                    FakeTransportError("restore")
                ]
            );
            let context = if operation == "configure" {
                "configuration"
            } else {
                "start"
            };
            assert_eq!(
                error.to_string(),
                format!(
                    "advertisement {context} failed: {operation}; cleanup errors: {cleanup_errors:?}"
                )
            );
            let expected = if operation == "configure" {
                vec!["preflight", "configure", "stop", "verify", "restore"]
            } else {
                vec![
                    "preflight",
                    "configure",
                    "start",
                    "stop",
                    "verify",
                    "restore",
                ]
            };
            assert_eq!(radio.calls, expected);
        }
    });
}

#[test]
fn advertisement_cleanup_only_failure_retains_collection_without_selecting_a_single_source() {
    async_io::block_on(async {
        let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("valid percentage"), 42);
        let mut radio = FakeBroadcastRadio {
            cleanup_fails: true,
            ..FakeBroadcastRadio::default()
        };
        let error = run_broadcast(&mut radio, sample, FakeElapsedDeadline)
            .await
            .expect_err("all advertisement cleanup operations fail");

        assert!(error.source().is_none());
        match error {
            BroadcastError::Cleanup(errors) => assert_eq!(
                errors,
                [
                    FakeTransportError("stop"),
                    FakeTransportError("verify"),
                    FakeTransportError("restore")
                ]
            ),
            other => panic!("expected cleanup-only failure, received {other:?}"),
        }
        assert_eq!(
            radio.calls,
            [
                "preflight",
                "configure",
                "start",
                "stop",
                "verify",
                "restore"
            ]
        );
    });
}

#[test]
fn ownership_cleanup_only_failure_retains_collection_without_selecting_a_single_source() {
    async_io::block_on(async {
        let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("valid percentage"), 42);
        let mut power = FakeRadioPower {
            cleanup_fails: true,
            ..FakeRadioPower::default()
        };
        let mut radio = FakeBroadcastRadio::default();
        let error = OwnedBroadcaster::new(&mut power, &mut FakeAdvertisement(&mut radio))
            .broadcast(sample)
            .await
            .expect_err("ownership inspection and release fail after a successful advertisement");

        assert!(error.source().is_none());
        match error {
            OwnedBroadcastError::Cleanup(errors) => assert_eq!(
                errors,
                [
                    FakeTransportError("cleanup ownership"),
                    FakeTransportError("source release")
                ]
            ),
            other => panic!("expected cleanup-only failure, received {other:?}"),
        }
        assert_eq!(
            power.calls,
            ["inspect", "acquire", "inspect", "inspect", "release"]
        );
        assert_eq!(
            radio.calls,
            [
                "preflight",
                "configure",
                "start",
                "stop",
                "verify",
                "restore"
            ]
        );
    });
}
