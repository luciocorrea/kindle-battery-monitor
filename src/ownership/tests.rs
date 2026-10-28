//! Source-4 acquisition and release while preserving competing radio ownership.

use crate::bthome::{BatteryLevel, BthomeSample};
use crate::monitor::{Broadcaster, Publication};
use crate::ownership::OwnedBroadcastError;
use crate::ownership::{AdvertisementBurst, OwnedBroadcaster, RadioPower};
use crate::radio::RadioState;
use crate::radio::{RadioCommand, RadioError};
use pretty_assertions::assert_eq;
use std::cell::{Cell, RefCell};
use std::io;
use std::rc::Rc;

struct TimeoutRadio {
    timeout_on_read: usize,
    reads: usize,
    release_fails: bool,
    mask: Rc<Cell<u32>>,
    trace: Rc<RefCell<Vec<&'static str>>>,
}

impl RadioPower for TimeoutRadio {
    type Error = RadioError;

    async fn state(&mut self) -> Result<RadioState, Self::Error> {
        self.trace.borrow_mut().push("inspect");
        self.reads += 1;
        if self.reads == self.timeout_on_read {
            return Err(RadioError::OwnershipTimeout);
        }
        Ok(if self.mask.get() == 0 {
            RadioState::DaemonAbsent
        } else {
            RadioState::EnabledSources(self.mask.get())
        })
    }
    async fn enable_source4(&mut self) -> Result<(), Self::Error> {
        self.trace.borrow_mut().push("enable");
        self.mask.set(self.mask.get() | 4);
        Ok(())
    }
    async fn disable_source4(&mut self) -> Result<(), Self::Error> {
        self.trace.borrow_mut().push("disable");
        if self.release_fails {
            return Err(RadioError::Timeout {
                command: RadioCommand::DisableBle,
            });
        }
        self.mask.set(self.mask.get() & !4);
        Ok(())
    }
}

struct TraceAdvertisement {
    trace: Rc<RefCell<Vec<&'static str>>>,
    mask: Rc<Cell<u32>>,
    outcome: Option<Publication>,
}

impl AdvertisementBurst for TraceAdvertisement {
    type Error = io::Error;
    async fn broadcast(&mut self, _: BthomeSample) -> Result<Publication, Self::Error> {
        self.trace.borrow_mut().push("burst-cleanup");
        self.mask.set(self.mask.get() | 1);
        self.outcome
            .ok_or_else(|| io::Error::other("burst failed after cleanup"))
    }
}

#[test]
fn ownership_timeouts_preserve_release_attempts_and_all_cleanup_failures() {
    async_io::block_on(async {
        for timeout_on_read in 1..=3 {
            for release_fails in [false, true] {
                for outcome in [
                    Some(Publication::Published),
                    Some(Publication::Cancelled),
                    None,
                ] {
                    let trace = Rc::new(RefCell::new(Vec::new()));
                    let mask = Rc::new(Cell::new(0));
                    let mut radio = TimeoutRadio {
                        timeout_on_read,
                        reads: 0,
                        release_fails,
                        mask: mask.clone(),
                        trace: trace.clone(),
                    };
                    let mut burst = TraceAdvertisement {
                        trace: trace.clone(),
                        mask: mask.clone(),
                        outcome,
                    };
                    let sample =
                        BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
                    let error = OwnedBroadcaster::new(&mut radio, &mut burst)
                        .broadcast(sample)
                        .await
                        .expect_err("inspection times out");
                    match timeout_on_read {
                        1 => {
                            assert!(matches!(
                                error,
                                OwnedBroadcastError::Inspect(RadioError::OwnershipTimeout)
                            ));
                            assert_eq!(*trace.borrow(), ["inspect"]);
                            assert_eq!(mask.get(), 0);
                        }
                        2 => {
                            match error {
                                OwnedBroadcastError::Enable {
                                    source: RadioError::OwnershipTimeout,
                                    cleanup_error,
                                } => {
                                    assert_eq!(cleanup_error.is_some(), release_fails);
                                    if let Some(error) = cleanup_error {
                                        assert!(matches!(
                                            error,
                                            RadioError::Timeout {
                                                command: RadioCommand::DisableBle
                                            }
                                        ));
                                    }
                                }
                                other => panic!("unexpected acquisition error: {other:?}"),
                            }
                            assert_eq!(
                                *trace.borrow(),
                                ["inspect", "enable", "inspect", "disable"]
                            );
                            assert_eq!(mask.get(), if release_fails { 4 } else { 0 });
                        }
                        3 => {
                            let errors = match error {
                                OwnedBroadcastError::Advertisement {
                                    source,
                                    cleanup_errors,
                                } => {
                                    assert!(outcome.is_none());
                                    assert_eq!(source.to_string(), "burst failed after cleanup");
                                    cleanup_errors
                                }
                                OwnedBroadcastError::Cleanup(errors) => {
                                    assert!(outcome.is_some());
                                    errors
                                }
                                other => panic!("unexpected cleanup error: {other:?}"),
                            };
                            assert_eq!(errors.len(), if release_fails { 2 } else { 1 });
                            assert!(matches!(errors[0], RadioError::OwnershipTimeout));
                            if release_fails {
                                assert!(matches!(
                                    errors[1],
                                    RadioError::Timeout {
                                        command: RadioCommand::DisableBle
                                    }
                                ));
                            }
                            assert_eq!(
                                *trace.borrow(),
                                [
                                    "inspect",
                                    "enable",
                                    "inspect",
                                    "burst-cleanup",
                                    "inspect",
                                    "disable"
                                ]
                            );
                            assert_eq!(mask.get(), if release_fails { 5 } else { 1 });
                        }
                        _ => unreachable!(),
                    }
                }
            }
        }
    });
}

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

    async fn state(&mut self) -> Result<RadioState, Self::Error> {
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

    async fn enable_source4(&mut self) -> Result<(), Self::Error> {
        self.enable_calls += 1;
        self.mask |= if self.competing_source_on_enable {
            5
        } else {
            4
        };
        Ok(())
    }

    async fn disable_source4(&mut self) -> Result<(), Self::Error> {
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

    async fn broadcast(&mut self, sample: BthomeSample) -> Result<Publication, Self::Error> {
        if self.fail {
            return Err(io::Error::other("advertisement rejected"));
        }
        self.samples.push(sample);
        Ok(Publication::Published)
    }
}

#[test]
fn an_existing_radio_owner_defers_without_mutation() {
    async_io::block_on(async {
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
                .await
                .expect("deferred");
            assert_eq!(outcome, Publication::Deferred);
            assert_eq!(
                (radio.enable_calls, radio.disable_calls, radio.mask),
                (0, 0, mask)
            );
            assert!(advertisement.samples.is_empty());
        }
    });
}

#[test]
fn a_competing_source_after_acquisition_defers_and_releases_only_our_source() {
    async_io::block_on(async {
        let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
        let mut radio = FakeRadio {
            competing_source_on_enable: true,
            ..FakeRadio::default()
        };
        let mut advertisement = FakeAdvertisement::default();
        let outcome = OwnedBroadcaster::new(&mut radio, &mut advertisement)
            .broadcast(sample)
            .await
            .expect("deferred");
        assert_eq!(outcome, Publication::Deferred);
        assert_eq!(radio.mask, 1);
        assert_eq!(radio.disable_calls, 1);
        assert!(advertisement.samples.is_empty());
    });
}

#[test]
fn rejected_advertisement_still_releases_owned_source() {
    async_io::block_on(async {
        let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
        let mut radio = FakeRadio::default();
        let mut advertisement = FakeAdvertisement {
            fail: true,
            ..FakeAdvertisement::default()
        };

        assert!(OwnedBroadcaster::new(&mut radio, &mut advertisement)
            .broadcast(sample)
            .await
            .is_err());
        assert_eq!(radio.enable_calls, 1);
        assert_eq!(radio.disable_calls, 1);
        assert_eq!(radio.mask, 0);
    });
}

#[test]
fn burst_releases_only_the_stock_radio_source_it_acquired() {
    async_io::block_on(async {
        let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
        let mut radio = FakeRadio::default();
        let mut advertisement = FakeAdvertisement::default();

        OwnedBroadcaster::new(&mut radio, &mut advertisement)
            .broadcast(sample)
            .await
            .expect("owned burst");

        assert_eq!(radio.enable_calls, 1);
        assert_eq!(radio.disable_calls, 1);
        assert_eq!(radio.mask, 0);
        assert_eq!(advertisement.samples, vec![sample]);
    });
}

#[test]
fn a_competing_owner_during_the_burst_survives_cleanup() {
    async_io::block_on(async {
        let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
        let mut radio = FakeRadio {
            competing_source_on_cleanup: true,
            ..FakeRadio::default()
        };
        let mut advertisement = FakeAdvertisement::default();
        assert_eq!(
            OwnedBroadcaster::new(&mut radio, &mut advertisement)
                .broadcast(sample)
                .await
                .expect("published"),
            Publication::Published
        );
        assert_eq!(radio.mask, 1);
        assert_eq!(radio.disable_calls, 1);
    });
}

#[test]
fn cleanup_inspection_failure_is_reported_but_owned_source_is_released() {
    async_io::block_on(async {
        let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
        let mut radio = FakeRadio {
            fail_cleanup_inspection: true,
            ..FakeRadio::default()
        };
        let mut advertisement = FakeAdvertisement::default();
        let error = OwnedBroadcaster::new(&mut radio, &mut advertisement)
            .broadcast(sample)
            .await
            .expect_err("inspection failed");
        assert!(error.to_string().contains("cleanup inspection failed"));
        assert_eq!(radio.mask, 0);
        assert_eq!(radio.disable_calls, 1);
    });
}
