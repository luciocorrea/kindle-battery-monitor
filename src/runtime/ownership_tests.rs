//! Ownership inspection deadlines at the connection and readback boundaries.

use super::*;
use std::cell::Cell;
use std::rc::Rc;

struct DropRecordingOperation(Rc<Cell<usize>>);

impl Drop for DropRecordingOperation {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

struct PendingConnection(DropRecordingOperation);

impl PendingConnection {
    async fn connect(self) -> Result<StalledRadioProtocol, RadioError> {
        let _owned = self.0;
        std::future::pending().await
    }
}

struct StalledRadioProtocol {
    _owned: DropRecordingOperation,
    readback_started: Rc<Cell<usize>>,
    readback: Option<async_channel::Receiver<Result<RadioState, RadioError>>>,
}

impl RadioProtocol for StalledRadioProtocol {
    async fn subscribe(&mut self, _: crate::radio::RadioEvent) -> Result<(), RadioError> {
        panic!("ownership inspection must not subscribe")
    }
    async fn set_source(&mut self, _: &str) -> Result<u32, RadioError> {
        panic!("ownership inspection must not mutate sources")
    }
    async fn wait_completion(&mut self) -> Result<(), RadioError> {
        panic!("ownership inspection must not await command completion")
    }
    async fn enabled_sources(&mut self) -> Result<RadioState, RadioError> {
        self.readback_started.set(self.readback_started.get() + 1);
        match &self.readback {
            Some(receiver) => receiver.recv().await.expect("scripted readback"),
            None => std::future::pending().await,
        }
    }
}

#[test]
fn the_same_deadline_spans_connection_and_pending_readback() {
    async_io::block_on(async {
        let drops = Rc::new(Cell::new(0));
        let readback_started = Rc::new(Cell::new(0));
        let (connected, connection) = async_channel::bounded::<()>(1);
        let (expired, deadline) = async_channel::bounded::<()>(1);
        let protocol = StalledRadioProtocol {
            _owned: DropRecordingOperation(drops.clone()),
            readback_started: readback_started.clone(),
            readback: None,
        };
        let mut operation = Box::pin(inspect_ownership(
            async {
                connection.recv().await.expect("connection gate");
                Ok(protocol)
            },
            async {
                let _ = deadline.recv().await;
            },
        ));
        assert!(futures_lite::future::poll_once(operation.as_mut())
            .await
            .is_none());
        assert_eq!(readback_started.get(), 0);
        connected.send(()).await.expect("connect");
        assert!(futures_lite::future::poll_once(operation.as_mut())
            .await
            .is_none());
        assert_eq!(readback_started.get(), 1);
        assert_eq!(drops.get(), 0);
        expired.send(()).await.expect("expire original deadline");
        assert!(matches!(operation.await, Err(RadioError::OwnershipTimeout)));
        assert_eq!(drops.get(), 1);
    });
}

#[test]
fn an_already_expired_deadline_does_not_start_protocol_io() {
    let started = Cell::new(false);
    let result = async_io::block_on(inspect_ownership(
        async {
            started.set(true);
            std::future::pending::<Result<StalledRadioProtocol, RadioError>>().await
        },
        std::future::ready(()),
    ));
    assert!(matches!(result, Err(RadioError::OwnershipTimeout)));
    assert!(!started.get());
}

#[test]
fn ownership_results_and_protocol_failures_are_preserved() {
    for reply in [
        Ok(RadioState::DaemonAbsent),
        Ok(RadioState::EnabledSources(5)),
        Err(RadioError::ReadStatus(7)),
    ] {
        let expected_state = reply.as_ref().ok().copied();
        let expected_status = match &reply {
            Err(RadioError::ReadStatus(code)) => Some(*code),
            _ => None,
        };
        let drops = Rc::new(Cell::new(0));
        let (sender, receiver) = async_channel::bounded(1);
        sender.try_send(reply).expect("queue readback");
        let protocol = StalledRadioProtocol {
            _owned: DropRecordingOperation(drops.clone()),
            readback_started: Rc::new(Cell::new(0)),
            readback: Some(receiver),
        };
        let result = async_io::block_on(inspect_ownership(
            std::future::ready(Ok(protocol)),
            std::future::pending(),
        ));
        match result {
            Ok(state) => assert_eq!(Some(state), expected_state),
            Err(RadioError::ReadStatus(code)) => assert_eq!(Some(code), expected_status),
            other => panic!("unexpected ownership result: {other:?}"),
        }
        assert_eq!(drops.get(), 1);
    }
    let result = async_io::block_on(inspect_ownership(
        std::future::ready(Err::<StalledRadioProtocol, _>(RadioError::FrontendAbsent)),
        std::future::pending(),
    ));
    assert!(matches!(result, Err(RadioError::FrontendAbsent)));
}

#[test]
fn the_real_deadline_expires_without_waiting_for_a_device() {
    let drops = Rc::new(Cell::new(0));
    let connection = PendingConnection(DropRecordingOperation(drops.clone()));
    let result = async_io::block_on(futures_lite::future::or(
        inspect_ownership(connection.connect(), async {
            async_io::Timer::after(Duration::from_millis(10)).await;
        }),
        async {
            async_io::Timer::after(Duration::from_secs(3)).await;
            Err(RadioError::Protocol("test watchdog"))
        },
    ));
    assert!(matches!(result, Err(RadioError::OwnershipTimeout)));
    assert_eq!(drops.get(), 1);
}

#[test]
fn ownership_timeout_cancels_a_pending_connection() {
    async_io::block_on(async {
        let drops = Rc::new(Cell::new(0));
        let (expired, deadline) = async_channel::bounded::<()>(1);
        let connection = PendingConnection(DropRecordingOperation(drops.clone()));
        let mut operation = Box::pin(inspect_ownership(connection.connect(), async {
            let _ = deadline.recv().await;
        }));
        assert!(futures_lite::future::poll_once(operation.as_mut())
            .await
            .is_none());
        assert_eq!(drops.get(), 0);
        expired.send(()).await.expect("expire shared deadline");
        assert!(matches!(operation.await, Err(RadioError::OwnershipTimeout)));
        assert_eq!(drops.get(), 1);
    });
}
