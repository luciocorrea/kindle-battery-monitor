//! Stock radio command sequencing, completion readback, and transaction deadlines.

use std::time::Duration;

use futures_lite::future::block_on;
use kindle_battery_monitor::radio::{
    RadioCommand, RadioController, RadioError, RadioEvent, RadioProtocol, RadioState,
};
use pretty_assertions::assert_eq;

#[derive(Debug, PartialEq, Eq)]
enum ProtocolAction {
    Subscribe(RadioEvent),
    Set(String),
    AwaitCompletion,
    ReadEnabledSources,
}

struct FakeRadioProtocol {
    actions: Vec<ProtocolAction>,
    state: RadioState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtocolStep {
    Subscription,
    Setter,
    Completion,
    Readback,
}

struct StalledRadioProtocol {
    step: ProtocolStep,
}

impl StalledRadioProtocol {
    async fn advance(&self, step: ProtocolStep) {
        if step == self.step {
            futures_lite::future::pending::<()>().await;
        }
    }
}

impl RadioProtocol for StalledRadioProtocol {
    async fn subscribe(&mut self, _event: RadioEvent) -> Result<(), RadioError> {
        self.advance(ProtocolStep::Subscription).await;
        Ok(())
    }

    async fn set_source(&mut self, _value: &str) -> Result<u32, RadioError> {
        self.advance(ProtocolStep::Setter).await;
        Ok(0)
    }

    async fn wait_completion(&mut self) -> Result<(), RadioError> {
        self.advance(ProtocolStep::Completion).await;
        Ok(())
    }

    async fn enabled_sources(&mut self) -> Result<RadioState, RadioError> {
        self.advance(ProtocolStep::Readback).await;
        Ok(RadioState::EnabledSources(4))
    }
}

impl RadioProtocol for FakeRadioProtocol {
    async fn subscribe(&mut self, event: RadioEvent) -> Result<(), RadioError> {
        self.actions.push(ProtocolAction::Subscribe(event));
        Ok(())
    }

    async fn set_source(&mut self, value: &str) -> Result<u32, RadioError> {
        self.actions.push(ProtocolAction::Set(value.to_owned()));
        Ok(0)
    }

    async fn wait_completion(&mut self) -> Result<(), RadioError> {
        self.actions.push(ProtocolAction::AwaitCompletion);
        Ok(())
    }

    async fn enabled_sources(&mut self) -> Result<RadioState, RadioError> {
        self.actions.push(ProtocolAction::ReadEnabledSources);
        Ok(self.state)
    }
}

#[test]
fn enabling_subscribes_before_request_and_requires_completion_and_readback() {
    let protocol = FakeRadioProtocol {
        actions: Vec::new(),
        state: RadioState::EnabledSources(4),
    };
    let mut radio = RadioController::new(protocol);

    let state =
        block_on(radio.execute_with_timeout(RadioCommand::EnableBle, Duration::from_secs(30)))
            .expect("the stock BLE enable transaction completes");

    assert_eq!(state, RadioState::EnabledSources(4));
    assert_eq!(
        radio.into_protocol().actions,
        vec![
            ProtocolAction::Subscribe(RadioEvent::BleEnabled),
            ProtocolAction::Set("1:4".to_owned()),
            ProtocolAction::AwaitCompletion,
            ProtocolAction::ReadEnabledSources,
        ]
    );
}

#[test]
fn an_expired_deadline_cannot_start_a_radio_transaction() {
    let protocol = FakeRadioProtocol {
        actions: Vec::new(),
        state: RadioState::EnabledSources(4),
    };
    let mut radio = RadioController::new(protocol);

    let result = block_on(radio.execute_with_timeout(RadioCommand::EnableBle, Duration::ZERO));

    assert!(result.is_err());
    assert!(radio.into_protocol().actions.is_empty());
}

#[test]
fn a_completion_event_cannot_override_a_contradictory_readback() {
    for (command, state) in [
        (RadioCommand::EnableBle, RadioState::EnabledSources(0)),
        (RadioCommand::EnableBle, RadioState::DaemonAbsent),
        (RadioCommand::DisableBle, RadioState::EnabledSources(4)),
    ] {
        let mut radio = RadioController::new(FakeRadioProtocol {
            actions: Vec::new(),
            state,
        });

        assert!(block_on(radio.execute(command)).is_err());
    }
}

#[test]
fn disabling_only_requests_source_four_and_accepts_stock_daemon_teardown() {
    for state in [RadioState::EnabledSources(0), RadioState::DaemonAbsent] {
        let mut radio = RadioController::new(FakeRadioProtocol {
            actions: Vec::new(),
            state,
        });

        assert_eq!(
            block_on(radio.execute(RadioCommand::DisableBle)).expect("BLE is disabled"),
            state
        );
        assert_eq!(
            radio.into_protocol().actions,
            vec![
                ProtocolAction::Subscribe(RadioEvent::BleDisabled),
                ProtocolAction::Set("0:4".to_owned()),
                ProtocolAction::AwaitCompletion,
                ProtocolAction::ReadEnabledSources,
            ]
        );
    }
}

#[test]
fn the_overall_deadline_bounds_every_protocol_stage() {
    for step in [
        ProtocolStep::Subscription,
        ProtocolStep::Setter,
        ProtocolStep::Completion,
        ProtocolStep::Readback,
    ] {
        let mut radio = RadioController::new(StalledRadioProtocol { step });
        let result =
            block_on(radio.execute_with_timeout(RadioCommand::EnableBle, Duration::from_millis(2)));
        assert!(
            matches!(result, Err(RadioError::Timeout { .. })),
            "{step:?}"
        );
    }
}

struct RejectedRadioProtocol {
    actions: Vec<ProtocolAction>,
}

impl RadioProtocol for RejectedRadioProtocol {
    async fn subscribe(&mut self, event: RadioEvent) -> Result<(), RadioError> {
        self.actions.push(ProtocolAction::Subscribe(event));
        Ok(())
    }

    async fn set_source(&mut self, value: &str) -> Result<u32, RadioError> {
        self.actions.push(ProtocolAction::Set(value.to_owned()));
        Ok(13)
    }

    async fn wait_completion(&mut self) -> Result<(), RadioError> {
        self.actions.push(ProtocolAction::AwaitCompletion);
        Err(RadioError::Protocol("unexpected completion wait"))
    }

    async fn enabled_sources(&mut self) -> Result<RadioState, RadioError> {
        self.actions.push(ProtocolAction::ReadEnabledSources);
        Err(RadioError::Protocol("unexpected readback"))
    }
}

#[test]
fn a_rejected_stock_command_never_waits_for_completion_or_claims_success() {
    let mut radio = RadioController::new(RejectedRadioProtocol {
        actions: Vec::new(),
    });
    let result = block_on(radio.execute(RadioCommand::EnableBle));
    assert!(matches!(result, Err(RadioError::Status { status: 13, .. })));
    assert_eq!(
        radio.into_protocol().actions,
        vec![
            ProtocolAction::Subscribe(RadioEvent::BleEnabled),
            ProtocolAction::Set("1:4".to_owned()),
        ]
    );
}

struct PrivateTestBus(std::process::Child);

impl PrivateTestBus {
    fn start() -> (Self, String) {
        use std::io::BufRead;
        let mut child = std::process::Command::new("dbus-daemon")
            .args([
                "--session",
                "--nofork",
                "--print-address=1",
                "--address=unix:tmpdir=/tmp",
            ])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("private test bus starts");
        let stdout = child.stdout.take().expect("bus address output is piped");
        let mut address = String::new();
        std::io::BufReader::new(stdout)
            .read_line(&mut address)
            .expect("test bus reports its address");
        (Self(child), address.trim().to_owned())
    }
}

impl Drop for PrivateTestBus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct FakeStockFrontend {
    requests: crossbeam_channel::Sender<String>,
}

#[zbus::interface(name = "com.lab126.btfd")]
impl FakeStockFrontend {
    #[zbus(name = "setBTenableStr")]
    async fn set_bt_enable(
        &self,
        value: &str,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> (u32,) {
        self.requests
            .send(value.to_owned())
            .expect("test observer is alive");
        connection
            .emit_signal(
                None::<&str>,
                "/default",
                "com.lab126.btfd",
                "BleEnabled",
                &(),
            )
            .await
            .expect("fake frontend publishes completion before replying");
        (0,)
    }
}

struct FakeStockManager;

#[zbus::interface(name = "com.lab126.btmd")]
impl FakeStockManager {
    #[zbus(name = "getEnableInt")]
    fn get_enable(&self) -> (u32, i32) {
        (0, 4)
    }
}

#[test]
#[ignore = "requires a host dbus-daemon; uses only an isolated test bus"]
fn a_real_bus_catches_completion_emitted_before_the_setter_reply() {
    use kindle_battery_monitor::radio::DbusRadio;
    let (_bus, address) = PrivateTestBus::start();
    let (requests, received) = crossbeam_channel::unbounded();
    block_on(async {
        let _server = zbus::connection::Builder::address(address.as_str())
            .expect("valid private bus address")
            .name("com.lab126.btfd")
            .expect("valid frontend name")
            .name("com.lab126.btmd")
            .expect("valid manager name")
            .serve_at("/default", FakeStockFrontend { requests })
            .expect("fake frontend")
            .serve_at("/default", FakeStockManager)
            .expect("fake manager")
            .build()
            .await
            .expect("fake services registered");
        let connection = zbus::connection::Builder::address(address.as_str())
            .expect("valid private bus address")
            .build()
            .await
            .expect("private client connects");
        let mut radio = RadioController::new(DbusRadio::from_connection(connection));

        assert_eq!(
            radio
                .execute(RadioCommand::EnableBle)
                .await
                .expect("transaction completes"),
            RadioState::EnabledSources(4)
        );
        assert_eq!(received.try_recv().expect("one request recorded"), "1:4");
        assert!(received.try_recv().is_err());
    });
}
