//! Battery property reads through an injected LIPC transport.

use kindle_battery_monitor::monitor::BatterySource;
use kindle_battery_monitor::powerd::PowerdBattery;
use kindle_lipc_dbus::{LipcDbusClient, LipcDbusError, LipcEndpoint, LipcMethodTransport};
use pretty_assertions::assert_eq;

struct FakePowerd;

impl LipcMethodTransport for FakePowerd {
    fn call_get_int(
        &self,
        endpoint: &LipcEndpoint<'_>,
        method: &str,
    ) -> Result<(u32, i32), LipcDbusError> {
        assert_eq!(endpoint.service, "com.lab126.powerd");
        assert_eq!(method, "getbattLevelInt");
        Ok((0, 83))
    }

    fn call_get_string(
        &self,
        _: &LipcEndpoint<'_>,
        _: &str,
    ) -> Result<(u32, String), LipcDbusError> {
        unreachable!("monitor reads only the battery integer")
    }

    fn call_set_int(&self, _: &LipcEndpoint<'_>, _: &str, _: i32) -> Result<u32, LipcDbusError> {
        unreachable!("monitor never changes powerd properties")
    }

    fn call_set_string(
        &self,
        _: &LipcEndpoint<'_>,
        _: &str,
        _: &str,
    ) -> Result<u32, LipcDbusError> {
        unreachable!("monitor never changes powerd properties")
    }
}

#[test]
fn reads_kindles_current_battery_level_from_powerd() {
    let mut battery = PowerdBattery::new(LipcDbusClient::from_transport(FakePowerd));

    assert_eq!(battery.read_percentage().expect("battery"), 83);
}
