//! Which apps stay usable while the phone is locked. The list lives on the phone and goes only to
//! the blocker plugin on the same device; it is never sent to the server (no payload type in
//! `phone::payload` has a field for it).
//!
//! The emergency call is not a choice: `AllowList` has no way to leave it out, whatever the
//! caller or the wire form says. The dialer and the messages app are roles the Android side
//! resolves to the device's default apps, so no package name is stored for them.
//!
//! Wire form (`fixtures/phone/blocker-bridge.json`, same file the TS mirror and the Kotlin plugin
//! read): `{ "roles": ["emergency_call", "dialer", "messages"], "packages": ["com.example.notes"] }`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

pub const ROLE_EMERGENCY_CALL: &str = "emergency_call";
pub const ROLE_DIALER: &str = "dialer";
pub const ROLE_MESSAGES: &str = "messages";

pub const MAX_CUSTOM_PACKAGES: usize = 50;
const PACKAGE_MAX_LEN: usize = 255;

/// Entering the settings app during a lock would let the user switch the blocker off, so it can
/// never be an allowed app. Prefix match: vendor settings live under the same prefix.
const SETTINGS_PREFIXES: [&str; 3] = ["com.android.settings", "com.android.packageinstaller", "com.google.android.packageinstaller"];
const SETTINGS_PACKAGES: [&str; 2] = ["com.google.android.permissioncontroller", "com.android.permissioncontroller"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AllowListError {
    #[error("malformed")]
    Malformed,
    #[error("unknown_role")]
    UnknownRole,
    #[error("invalid_package")]
    InvalidPackage,
    #[error("settings_package")]
    SettingsPackage,
    #[error("too_many_packages")]
    TooManyPackages,
}

impl AllowListError {
    pub fn code(self) -> &'static str {
        match self {
            AllowListError::Malformed => "malformed",
            AllowListError::UnknownRole => "unknown_role",
            AllowListError::InvalidPackage => "invalid_package",
            AllowListError::SettingsPackage => "settings_package",
            AllowListError::TooManyPackages => "too_many_packages",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllowList {
    dialer: bool,
    messages: bool,
    packages: BTreeSet<String>,
}

impl Default for AllowList {
    /// Phone and messages allowed, no custom apps.
    fn default() -> Self {
        Self { dialer: true, messages: true, packages: BTreeSet::new() }
    }
}

impl AllowList {
    pub fn new<I, S>(dialer: bool, messages: bool, packages: I) -> Result<Self, AllowListError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut set = BTreeSet::new();
        for package in packages {
            let package = package.as_ref();
            validate_package(package)?;
            set.insert(package.to_string());
        }
        if set.len() > MAX_CUSTOM_PACKAGES {
            return Err(AllowListError::TooManyPackages);
        }
        Ok(Self { dialer, messages, packages: set })
    }

    /// Always true. A method rather than a field so no caller can build a list without it.
    pub fn allows_emergency_call(&self) -> bool {
        true
    }

    pub fn allows_dialer(&self) -> bool {
        self.dialer
    }

    pub fn allows_messages(&self) -> bool {
        self.messages
    }

    pub fn packages(&self) -> impl Iterator<Item = &str> {
        self.packages.iter().map(String::as_str)
    }

    /// Reads the wire form. A missing `emergency_call` role is not an error: it is added. Roles the
    /// phone does not know, and anything but an object of two arrays, are refused.
    pub fn from_wire(wire: &Value) -> Result<Self, AllowListError> {
        let object = wire.as_object().ok_or(AllowListError::Malformed)?;
        if object.keys().any(|key| key != "roles" && key != "packages") {
            return Err(AllowListError::Malformed);
        }
        let roles = object.get("roles").and_then(Value::as_array).ok_or(AllowListError::Malformed)?;
        let packages = object.get("packages").and_then(Value::as_array).ok_or(AllowListError::Malformed)?;
        let (mut dialer, mut messages) = (false, false);
        for role in roles {
            match role.as_str().ok_or(AllowListError::Malformed)? {
                ROLE_EMERGENCY_CALL => {}
                ROLE_DIALER => dialer = true,
                ROLE_MESSAGES => messages = true,
                _ => return Err(AllowListError::UnknownRole),
            }
        }
        let names = packages.iter().map(|p| p.as_str().ok_or(AllowListError::Malformed)).collect::<Result<Vec<_>, _>>()?;
        Self::new(dialer, messages, names)
    }

    /// Roles in a fixed order with `emergency_call` first, packages sorted.
    pub fn to_wire(&self) -> Value {
        let mut roles = vec![ROLE_EMERGENCY_CALL];
        if self.dialer {
            roles.push(ROLE_DIALER);
        }
        if self.messages {
            roles.push(ROLE_MESSAGES);
        }
        json!({ "roles": roles, "packages": self.packages.iter().collect::<Vec<_>>() })
    }
}

/// Android application id: at least two dot-separated segments, each starting with a letter and
/// made of letters, digits and underscores.
fn validate_package(name: &str) -> Result<(), AllowListError> {
    let segments: Vec<&str> = name.split('.').collect();
    let well_formed = name.len() <= PACKAGE_MAX_LEN
        && segments.len() >= 2
        && segments.iter().all(|segment| {
            let mut chars = segment.chars();
            chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
    if !well_formed {
        return Err(AllowListError::InvalidPackage);
    }
    let lowered = name.to_ascii_lowercase();
    if SETTINGS_PACKAGES.contains(&lowered.as_str()) || SETTINGS_PREFIXES.iter().any(|prefix| lowered.starts_with(prefix)) {
        return Err(AllowListError::SettingsPackage);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> Value {
        serde_json::from_str(include_str!("../../../fixtures/phone/blocker-bridge.json")).expect("fixture")
    }

    #[test]
    fn fixture_allow_lists_normalize_to_the_shared_wire_form() {
        let contract = contract();
        for case in contract["allowList"]["valid"].as_array().expect("valid") {
            let name = case["name"].as_str().expect("name");
            let list = AllowList::from_wire(&case["input"]).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(list.to_wire(), case["wire"], "{name}");
        }
    }

    #[test]
    fn fixture_refusals_carry_the_shared_codes() {
        let contract = contract();
        for case in contract["allowList"]["invalid"].as_array().expect("invalid") {
            let name = case["name"].as_str().expect("name");
            let err = AllowList::from_wire(&case["input"]).expect_err(name);
            assert_eq!(err.code(), case["error"].as_str().expect("error"), "{name}");
        }
    }

    #[test]
    fn the_emergency_call_is_in_every_list_however_it_was_built() {
        assert!(AllowList::default().allows_emergency_call());
        let bare = AllowList::new(false, false, Vec::<&str>::new()).expect("bare");
        assert!(bare.allows_emergency_call());
        assert_eq!(bare.to_wire()["roles"], json!([ROLE_EMERGENCY_CALL]));
        let from_empty_roles = AllowList::from_wire(&json!({ "roles": [], "packages": [] })).expect("empty roles");
        assert_eq!(from_empty_roles.to_wire()["roles"], json!([ROLE_EMERGENCY_CALL]));
        let many = AllowList::new(true, true, (0..MAX_CUSTOM_PACKAGES).map(|n| format!("com.example.app{n}"))).expect("max");
        assert_eq!(many.to_wire()["roles"][0], json!(ROLE_EMERGENCY_CALL));
    }

    #[test]
    fn custom_packages_are_deduplicated_sorted_and_capped() {
        let list = AllowList::new(true, false, ["com.b.app", "com.a.app", "com.b.app"]).expect("list");
        assert_eq!(list.packages().collect::<Vec<_>>(), ["com.a.app", "com.b.app"]);
        let too_many = (0..=MAX_CUSTOM_PACKAGES).map(|n| format!("com.example.app{n}"));
        assert_eq!(AllowList::new(true, true, too_many), Err(AllowListError::TooManyPackages));
    }

    #[test]
    fn the_settings_app_can_never_be_allowed() {
        for name in ["com.android.settings", "COM.ANDROID.SETTINGS", "com.android.settings.intelligence", "com.google.android.permissioncontroller"] {
            assert_eq!(AllowList::new(true, true, [name]), Err(AllowListError::SettingsPackage), "{name}");
        }
    }

    #[test]
    fn malformed_package_names_are_refused() {
        for name in ["", "notes", "com..app", "com.1app", "com.app!", ".com.app", "com.app.", "com.한글.app"] {
            assert_eq!(AllowList::new(true, true, [name]), Err(AllowListError::InvalidPackage), "{name:?}");
        }
        assert_eq!(AllowList::new(true, true, [format!("com.{}", "a".repeat(PACKAGE_MAX_LEN))]), Err(AllowListError::InvalidPackage));
    }
}
