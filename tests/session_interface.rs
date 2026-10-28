//! External caller contracts for construction and cancellation without device IO.

use kindle_battery_monitor::session::errors::ReadinessError;
use kindle_battery_monitor::session::{RunMode, Session, SessionError};

#[test]
fn cancellation_before_run_stops_at_readiness_without_device_io() {
    let session = Session::new(RunMode::Once);
    let cancellation = session.cancellation();
    let another_handle = cancellation.clone();
    cancellation.request();
    another_handle.request();
    drop(another_handle);
    let error = async_io::block_on(session.run()).expect_err("cancelled readiness");
    assert!(matches!(
        error,
        SessionError::Readiness(ReadinessError::Cancelled)
    ));
    cancellation.request();
}

#[test]
fn external_callers_can_inspect_the_complete_typed_error_graph() {
    use kindle_battery_monitor::session::errors::{
        BroadcastError, BsaBurstError, BsaError, BsaRadioError, ConfigError, ContinuousError,
        EventListenerError, InvalidRefreshInterval, MonitorError, OwnedBroadcastError,
        PowerdCleanupError, PowerdEventError, PowerdReadError, PublicationError, RadioCommand,
        RadioError, RadioState, RunError, RuntimeEventError, SampleError, StartupError,
    };
    use std::error::Error;
    let publication: PublicationError = OwnedBroadcastError::Advertisement {
        source: BsaBurstError::Broadcast(BroadcastError::Configure {
            source: BsaRadioError::Config(ConfigError::Status(9)),
            cleanup_errors: vec![BsaRadioError::Transport(BsaError::DeadlineExceeded)],
        }),
        cleanup_errors: vec![RadioError::Readback {
            command: RadioCommand::DisableBle,
            state: RadioState::EnabledSources(4),
        }],
    };
    let primary: ContinuousError = RunError::Publication(publication);
    let error = SessionError::Combined {
        primary: Box::new(primary),
        listener: Box::new(EventListenerError::Shutdown(vec![
            PowerdCleanupError::Receive(PowerdEventError::Closed),
            PowerdCleanupError::UnsubscribeTimeout,
            PowerdCleanupError::CloseTimeout,
        ])),
    };
    assert!(error.source().is_some());
    match error {
        SessionError::Combined { primary, listener } => {
            assert!(
                matches!(*primary, RunError::Publication(OwnedBroadcastError::Advertisement { source: BsaBurstError::Broadcast(BroadcastError::Configure { cleanup_errors, .. }), cleanup_errors: ownership }) if cleanup_errors.len() == 1 && ownership.len() == 1)
            );
            assert!(
                matches!(*listener, EventListenerError::Shutdown(ref failures) if matches!(failures.as_slice(), [PowerdCleanupError::Receive(_), PowerdCleanupError::UnsubscribeTimeout, PowerdCleanupError::CloseTimeout]))
            );
        }
        other => panic!("lost structured failures: {other:?}"),
    }
    let sample: SampleError = MonitorError::Battery(PowerdReadError::Cancelled);
    assert!(matches!(
        sample,
        MonitorError::Battery(PowerdReadError::Cancelled)
    ));
    let startup: StartupError = ReadinessError::Cancelled;
    assert!(matches!(startup, ReadinessError::Cancelled));
    let refresh = InvalidRefreshInterval {
        interval: std::time::Duration::ZERO,
    };
    assert_eq!(refresh.interval, std::time::Duration::ZERO);
    let event: ContinuousError = RunError::Event(RuntimeEventError::BatteryListenerStopped);
    assert!(matches!(
        event,
        RunError::Event(RuntimeEventError::BatteryListenerStopped)
    ));
}
