//! Focus sensors: which app is in front and how long the user has not touched keyboard or mouse.
//! Only the app identity and a count of idle seconds leave this module. Window titles are never
//! read, and no key or pointer event content is observed (macOS reports a single idle number).

use serde::Serialize;

use super::{AppIdentity, PlatformError};

/// Whether the OS would let this app read window titles. Titles are not read or stored in stage 1
/// (D-30); this exists for the first-run explanation screen only.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TitleAccess {
    // Only the macOS probe reports Granted/Denied; every other target reports Unsupported.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Granted,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Denied,
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Unsupported,
}

pub trait FocusProbe: Send + Sync {
    /// The app in front, or `None` when nothing is (login window, locked screen, screen saver).
    fn frontmost_app(&self) -> Result<Option<AppIdentity>, PlatformError>;
    /// Whole seconds since the last keyboard or mouse input, session-wide.
    fn idle_seconds(&self) -> Result<u64, PlatformError>;
    fn window_title_access(&self) -> TitleAccess;
}

/// Converts the OS idle reading to whole seconds. A negative or non-finite value means the OS
/// gave no usable answer; it is an error rather than a fabricated zero.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(super) fn whole_idle_seconds(raw: f64) -> Result<u64, PlatformError> {
    if !raw.is_finite() || raw < 0.0 {
        return Err(PlatformError::Process("idle time reading is not a valid duration".into()));
    }
    // Saturating float-to-int conversion; the reading is a small positive number of seconds.
    Ok(raw.floor() as u64)
}

/// Sensors for targets without an implementation: every call says so explicitly.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub struct UnsupportedFocusProbe;

impl FocusProbe for UnsupportedFocusProbe {
    fn frontmost_app(&self) -> Result<Option<AppIdentity>, PlatformError> {
        Err(PlatformError::Unsupported("foreground app sensor (Windows pending real-device verification)"))
    }

    fn idle_seconds(&self) -> Result<u64, PlatformError> {
        Err(PlatformError::Unsupported("input idle sensor (Windows pending real-device verification)"))
    }

    fn window_title_access(&self) -> TitleAccess {
        TitleAccess::Unsupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_readings_floor_to_whole_seconds_and_reject_garbage() {
        assert_eq!(whole_idle_seconds(0.0).expect("zero"), 0);
        assert_eq!(whole_idle_seconds(599.9).expect("floor"), 599);
        assert_eq!(whole_idle_seconds(600.0).expect("exact"), 600);
        assert!(whole_idle_seconds(-1.0).is_err());
        assert!(whole_idle_seconds(f64::NAN).is_err());
        assert!(whole_idle_seconds(f64::INFINITY).is_err());
    }

    #[test]
    fn the_unsupported_probe_fails_every_call_explicitly() {
        let probe = UnsupportedFocusProbe;
        assert!(matches!(probe.frontmost_app(), Err(PlatformError::Unsupported(_))));
        assert!(matches!(probe.idle_seconds(), Err(PlatformError::Unsupported(_))));
        assert_eq!(probe.window_title_access(), TitleAccess::Unsupported);
    }
}
