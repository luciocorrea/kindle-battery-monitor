//! Firmware-specific configuration encoding and initialized-field readback contracts.

use crate::bthome::{BatteryLevel, BthomeSample};
use crate::config::{advertising_data, visibility};
use crate::config::{AdvertisingState, DeviceConfig};
use pretty_assertions::assert_eq;

#[test]
fn reads_only_defined_radio_and_legacy_advertising_fields() {
    let mut reply = vec![0xa5; 836];
    reply[..2].copy_from_slice(&0_u16.to_le_bytes());
    reply[8] = 1;
    reply[9] = 0;
    reply[10] = 0;
    reply[4 + 0x201] = 0;
    reply[4 + 0x202] = 0;
    let config = DeviceConfig::from_reply(&reply).expect("valid target reply");
    assert_eq!(config.advertising_state(), AdvertisingState::Inactive);
    assert!(config.radio_enabled());
    assert!(!format!("{config:?}").contains("165"));
}

#[test]
fn visibility_controls_only_the_nonconnectable_default_advertisement() {
    let active = visibility(AdvertisingState::Active);
    let inactive = visibility(AdvertisingState::Inactive);
    assert_eq!(&active[..7], &[0, 0x20, 0, 0, 1, 0, 0]);
    assert_eq!(&active[0x201..0x203], &[1, 0]);
    assert_eq!(&inactive[0x201..0x203], &[0, 0]);
    assert_eq!(active[0x317], 0);
    let differences: Vec<_> = active
        .iter()
        .zip(inactive)
        .enumerate()
        .filter_map(|(offset, (left, right))| (*left != right).then_some(offset))
        .collect();
    assert_eq!(differences, vec![0x201]);
}

#[test]
fn serializes_battery_service_data_and_the_complete_kindle_name() {
    let sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
    let request = advertising_data(sample);
    let nonzero: Vec<_> = request
        .iter()
        .enumerate()
        .filter_map(|(offset, byte)| (*byte != 0).then_some((offset, *byte)))
        .collect();
    assert_eq!(request.len(), 832);
    assert_eq!(
        nonzero,
        vec![
            (1, 0x80),
            (4, 1),
            (0x219, 6),
            (0x23c, 2),
            (0x23d, 0x81),
            (0x250, 5),
            (0x254, 2),
            (0x258, 0xd2),
            (0x259, 0xfc),
            (0x268, 0x44),
            (0x26a, 42),
            (0x26b, 1),
            (0x26c, 52),
            (0x2b0, 1),
            (0x2b1, 0x09),
            (0x2b2, 6),
            (0x2b3, b'K'),
            (0x2b4, b'i'),
            (0x2b5, b'n'),
            (0x2b6, b'd'),
            (0x2b7, b'l'),
            (0x2b8, b'e'),
        ]
    );
}
#[test]
fn stock_privacy_requests_change_only_the_recovered_fields() {
    use crate::config::{privacy, AddressPolicy, CONFIG_BYTES};
    let mut expected = [0; CONFIG_BYTES];
    expected[2] = 2;
    expected[4] = 1;
    pretty_assertions::assert_eq!(privacy(AddressPolicy::Public), expected);
    expected[0x203] = 1;
    pretty_assertions::assert_eq!(privacy(AddressPolicy::Rotating), expected);
}
