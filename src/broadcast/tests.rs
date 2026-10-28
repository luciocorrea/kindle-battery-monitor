//! Advertisement cleanup contracts across deadlines, cancellation, and radio failures.

use crate::broadcast::{run_broadcast, BroadcastRadio, EndReason, WaitForEnd};
use crate::bthome::{BatteryLevel, BthomeSample};
use std::io;

#[derive(Default)]
struct FakeRadio {
    advertising: bool,
    sample: Option<BthomeSample>,
    start_reply_lost: bool,
    stop_fails: bool,
    configure_fails: bool,
    public_identity: bool,
    restore_calls: usize,
}

impl BroadcastRadio for FakeRadio {
    type Error = io::Error;

    async fn ensure_available(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    async fn configure(&mut self, sample: BthomeSample) -> Result<(), Self::Error> {
        self.public_identity = true;
        if self.configure_fails {
            return Err(io::Error::other(
                "configuration reply lost after identity changed",
            ));
        }
        self.sample = Some(sample);
        Ok(())
    }
    async fn start(&mut self) -> Result<(), Self::Error> {
        self.advertising = true;
        if self.start_reply_lost {
            Err(io::Error::new(io::ErrorKind::TimedOut, "start reply lost"))
        } else {
            Ok(())
        }
    }
    async fn stop(&mut self) -> Result<(), Self::Error> {
        if self.stop_fails {
            return Err(io::Error::other("stop rejected"));
        }
        self.advertising = false;
        Ok(())
    }
    async fn verify_stopped(&mut self) -> Result<(), Self::Error> {
        if self.advertising {
            Err(io::Error::other("still advertising"))
        } else {
            Ok(())
        }
    }
    async fn restore_identity(&mut self) -> Result<(), Self::Error> {
        self.restore_calls += 1;
        self.public_identity = false;
        Ok(())
    }
}

#[test]
fn privacy_is_restored_after_configuration_failure_and_failed_stop() {
    async_io::block_on(async {
        let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
        for (configure_fails, stop_fails) in [(true, false), (false, true)] {
            let mut radio = FakeRadio {
                configure_fails,
                stop_fails,
                ..FakeRadio::default()
            };
            assert!(run_broadcast(&mut radio, sample, ElapsedDeadline)
                .await
                .is_err());
            assert!(!radio.public_identity);
            assert_eq!(radio.restore_calls, 1);
        }
    });
}

struct Cancellation;
impl WaitForEnd for Cancellation {
    async fn wait(self) -> EndReason {
        EndReason::Cancelled
    }
}

#[test]
fn cancellation_stops_the_owned_advertisement() {
    async_io::block_on(async {
        let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
        let mut radio = FakeRadio::default();
        assert_eq!(
            run_broadcast(&mut radio, sample, Cancellation)
                .await
                .expect("cleanup"),
            EndReason::Cancelled
        );
        assert!(!radio.advertising);
        assert!(!radio.public_identity);
        assert_eq!(radio.restore_calls, 1);
    });
}

#[test]
fn rejected_cleanup_is_reported_as_failure() {
    async_io::block_on(async {
        let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
        let mut radio = FakeRadio {
            stop_fails: true,
            ..FakeRadio::default()
        };
        assert!(run_broadcast(&mut radio, sample, ElapsedDeadline)
            .await
            .is_err());
        assert!(radio.advertising);
    });
}

#[test]
fn cleans_up_when_start_may_have_succeeded_but_the_reply_was_lost() {
    async_io::block_on(async {
        let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
        let mut radio = FakeRadio {
            start_reply_lost: true,
            ..FakeRadio::default()
        };
        assert!(run_broadcast(&mut radio, sample, ElapsedDeadline)
            .await
            .is_err());
        assert!(!radio.advertising);
    });
}

struct ElapsedDeadline;
impl WaitForEnd for ElapsedDeadline {
    async fn wait(self) -> EndReason {
        EndReason::Deadline
    }
}

#[test]
fn stops_the_test_advertisement_when_its_deadline_elapses() {
    async_io::block_on(async {
        let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
        let mut radio = FakeRadio::default();
        let end = run_broadcast(&mut radio, sample, ElapsedDeadline)
            .await
            .expect("bounded run");
        assert_eq!(end, EndReason::Deadline);
        assert_eq!(radio.sample, Some(sample));
        assert!(!radio.advertising);
    });
}
