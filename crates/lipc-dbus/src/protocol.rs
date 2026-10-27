//! Pure LIPC method naming, endpoint constants, and signal match strings.
//!
//! Helpers preserve supplied names verbatim. They do not validate DBus names or
//! escape match-rule fragments; callers must supply valid service/event names.

/// The stock LIPC method object path.
pub const LIPC_OBJECT_PATH: &str = "/default";

/// The operation prefix used in a LIPC method name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PropertyAccess {
    /// Use the `get` prefix.
    Get,
    /// Use the `set` prefix.
    Set,
}

/// The suffix used by a LIPC property method.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PropertyType {
    /// Use the `Int` suffix for integer properties.
    Int,
    /// Use the `Str` suffix for string properties.
    Str,
    /// Use the `Has` suffix.
    Has,
}

/// Concatenates access prefix, unchanged property spelling, and type suffix.
///
/// For example, `Get`, `battLevel`, and `Int` produce `getbattLevelInt`.
/// No name validation or IO is performed.
pub fn property_method_name(
    access: PropertyAccess,
    property: &str,
    property_type: PropertyType,
) -> String {
    let mut method = String::new();
    method.push_str(access.method_prefix());
    method.push_str(property);
    method.push_str(property_type.method_suffix());
    method
}

/// Returns the service name unchanged as the LIPC interface name.
pub fn service_interface(service: &str) -> &str {
    service
}

/// Builds a signal rule filtered by interface and optional member.
///
/// `None` accepts any member of that interface. The rule does not restrict path
/// or sender and does not authenticate a service owner. Inputs are inserted
/// verbatim and must be valid match-rule fragments.
pub fn event_match_rule(service: &str, event: Option<&str>) -> String {
    let mut rule = String::from("type='signal',interface='");
    rule.push_str(service);
    rule.push('\'');

    if let Some(event) = event {
        rule.push_str(",member='");
        rule.push_str(event);
        rule.push('\'');
    }

    rule
}

impl PropertyAccess {
    const fn method_prefix(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Set => "set",
        }
    }
}

impl PropertyType {
    const fn method_suffix(self) -> &'static str {
        match self {
            Self::Int => "Int",
            Self::Str => "Str",
            Self::Has => "Has",
        }
    }
}
