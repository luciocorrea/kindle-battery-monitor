//! LIPC naming, endpoint, match-rule, and lossless status-code contracts.

use kindle_lipc_dbus::{
    event_match_rule, property_method_name, service_interface, LipcStatus, PropertyAccess,
    PropertyType, LIPC_OBJECT_PATH,
};
use pretty_assertions::assert_eq;

#[test]
fn property_method_name_encodes_access_property_and_type() {
    let cases = [
        (
            PropertyAccess::Get,
            "battLevel",
            PropertyType::Int,
            "getbattLevelInt",
        ),
        (
            PropertyAccess::Set,
            "orientationLock",
            PropertyType::Str,
            "setorientationLockStr",
        ),
        (
            PropertyAccess::Get,
            "profileData",
            PropertyType::Has,
            "getprofileDataHas",
        ),
    ];

    for (access, property, property_type, expected) in cases {
        assert_eq!(
            property_method_name(access, property, property_type),
            expected
        );
    }
}

#[test]
fn service_interface_and_object_path_match_lipc_wire_convention() {
    assert_eq!(LIPC_OBJECT_PATH, "/default");
    assert_eq!(service_interface("com.lab126.powerd"), "com.lab126.powerd");
}

#[test]
fn event_match_rule_encodes_publisher_and_optional_event() {
    assert_eq!(
        event_match_rule("com.lab126.powerd", None),
        "type='signal',interface='com.lab126.powerd'"
    );
    assert_eq!(
        event_match_rule("com.lab126.powerd", Some("battLevelChanged")),
        "type='signal',interface='com.lab126.powerd',member='battLevelChanged'"
    );
}

#[test]
fn lipc_status_maps_known_codes_and_preserves_unknown_codes() {
    assert_eq!(LipcStatus::from_code(0), LipcStatus::Ok);
    assert_eq!(LipcStatus::from_code(3), LipcStatus::NoSuchSource);
    assert_eq!(LipcStatus::from_code(8), LipcStatus::NoSuchProperty);
    assert_eq!(LipcStatus::from_code(15), LipcStatus::TimedOut);
    assert_eq!(
        LipcStatus::from_code(0x100),
        LipcStatus::PropertyInvalidState
    );
    assert_eq!(LipcStatus::from_code(9999), LipcStatus::UnknownCode(9999));
}
