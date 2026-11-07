//! OS-neutral process control: list running apps, ask one to quit, force-kill it. The enforcement
//! engine decides *what* to end; this module only knows *how*, per OS (`macos.rs`, `windows.rs`).
//!
//! Identity is never a display name. macOS apps are identified by bundle id (executable path when
//! an app has none), Windows apps by normalized executable path. Window titles are never read.

use super::PlatformError;

/// One running GUI app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppIdentity {
    /// Normalized bundle id or executable path (see [`normalize_app_id`]). The only value logged.
    pub id: String,
    /// The app's name, for lists and notifications. Never a window title.
    pub display_name: String,
    pub pid: u32,
    /// System, security, and self processes. The engine never ends these.
    pub is_protected: bool,
}

pub trait ProcessControl: Send + Sync {
    /// GUI apps only (background daemons are not blocking targets).
    fn list_running(&self) -> Result<Vec<AppIdentity>, PlatformError>;
    /// Asks the app to quit the normal way (macOS `terminate`, Windows `WM_CLOSE`). The app may
    /// refuse or show a save dialog; success means the request was delivered, not that it quit.
    fn request_quit(&self, app: &AppIdentity) -> Result<(), PlatformError>;
    /// Ends the app immediately (macOS SIGKILL, Windows `TerminateProcess`). Unsaved work is lost.
    fn force_kill(&self, app: &AppIdentity) -> Result<(), PlatformError>;
}

/// Canonical form used for stored patterns and for comparing with [`AppIdentity::id`]: trimmed,
/// lowercase, `/` separators. Bundle ids pass through unchanged apart from case.
pub fn normalize_app_id(raw: &str) -> String {
    raw.trim().to_lowercase().replace('\\', "/")
}

/// Bundle ids of macOS system and security apps that must never be ended (AGENTS.md section 5).
/// Already normalized.
pub const PROTECTED_MACOS_IDS: &[&str] = &[
    "com.apple.finder",
    "com.apple.dock",
    "com.apple.loginwindow",
    "com.apple.securityd",
    "com.apple.securityagent",
    "com.apple.systemuiserver",
    "com.apple.controlcenter",
    "com.apple.notificationcenterui",
    "com.apple.windowserver",
    "com.apple.coreservices.uiagent",
    "com.apple.systempreferences",
    "com.apple.keychainaccess",
    "com.apple.activitymonitor",
];

/// Executable file names (lowercase) of Windows system and security processes.
pub const PROTECTED_WINDOWS_NAMES: &[&str] = &[
    "csrss.exe",
    "winlogon.exe",
    "lsass.exe",
    "services.exe",
    "smss.exe",
    "wininit.exe",
    "svchost.exe",
    "dwm.exe",
    "explorer.exe",
    "taskmgr.exe",
    "logonui.exe",
    "sihost.exe",
    "ctfmon.exe",
    "fontdrvhost.exe",
    "runtimebroker.exe",
    "applicationframehost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "searchhost.exe",
    "securityhealthservice.exe",
    "securityhealthsystray.exe",
    "msmpeng.exe",
    "msedgewebview2.exe",
];

/// The app's own bundle id (`tauri.conf.json` identifier) and executable name.
pub const SELF_BUNDLE_ID: &str = "com.studymanager.app";
pub const SELF_EXE_NAMES: &[&str] = &["study-manager", "study-manager.exe"];

/// Whether an already-normalized app id is on a protected list or is this app. Used to refuse a
/// block-list entry up front and by the OS backends when they build an [`AppIdentity`].
pub fn is_protected_id(id: &str) -> bool {
    if id == SELF_BUNDLE_ID || PROTECTED_MACOS_IDS.contains(&id) {
        return true;
    }
    // A path id: judge by file name, and treat anything under the Windows directory as system.
    let file_name = id.rsplit('/').next().unwrap_or(id);
    SELF_EXE_NAMES.contains(&file_name)
        || PROTECTED_WINDOWS_NAMES.contains(&file_name)
        || id.contains(":/windows/")
}

/// Process control for targets without an implementation: every call reports `Unsupported`.
#[cfg_attr(any(target_os = "macos", target_os = "windows"), allow(dead_code))]
pub struct UnsupportedProcessControl;

impl ProcessControl for UnsupportedProcessControl {
    fn list_running(&self) -> Result<Vec<AppIdentity>, PlatformError> {
        Err(PlatformError::Unsupported("process control"))
    }

    fn request_quit(&self, _app: &AppIdentity) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("process control"))
    }

    fn force_kill(&self, _app: &AppIdentity) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("process control"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_normalized_case_and_separators() {
        assert_eq!(normalize_app_id("  Com.Apple.Safari "), "com.apple.safari");
        assert_eq!(
            normalize_app_id("C:\\Program Files\\Game\\Game.EXE"),
            "c:/program files/game/game.exe"
        );
    }

    #[test]
    fn macos_system_and_security_apps_are_protected() {
        for id in [
            "com.apple.finder",
            "com.apple.dock",
            "com.apple.loginwindow",
            "com.apple.securityd",
        ] {
            assert!(is_protected_id(id), "{id}");
        }
        assert!(!is_protected_id("com.apple.safari"));
        assert!(!is_protected_id("com.spotify.client"));
    }

    #[test]
    fn windows_system_security_and_self_are_protected_by_file_name_or_location() {
        for id in [
            "c:/windows/system32/csrss.exe",
            "c:/windows/system32/winlogon.exe",
            "c:/windows/system32/lsass.exe",
            "c:/windows/explorer.exe",
            "d:/tools/anything/explorer.exe",
            "c:/windows/system32/notepad.exe",
            "c:/program files/study-manager/study-manager.exe",
        ] {
            assert!(is_protected_id(id), "{id}");
        }
        assert!(!is_protected_id("c:/program files/steam/steam.exe"));
    }

    #[test]
    fn this_app_is_protected_by_bundle_id() {
        assert!(is_protected_id(SELF_BUNDLE_ID));
    }

    #[test]
    fn the_self_bundle_id_matches_the_tauri_identifier() {
        let conf = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json"))
            .expect("conf");
        assert!(conf.contains(&format!("\"identifier\": \"{SELF_BUNDLE_ID}\"")));
    }

    #[test]
    fn protected_lists_are_already_normalized() {
        for id in PROTECTED_MACOS_IDS {
            assert_eq!(*id, normalize_app_id(id));
        }
        for name in PROTECTED_WINDOWS_NAMES {
            assert_eq!(*name, normalize_app_id(name));
        }
    }

    #[test]
    fn unsupported_control_reports_unsupported_for_every_call() {
        let control = UnsupportedProcessControl;
        let app = AppIdentity {
            id: "x".into(),
            display_name: "X".into(),
            pid: 1,
            is_protected: false,
        };
        assert!(matches!(
            control.list_running(),
            Err(PlatformError::Unsupported(_))
        ));
        assert!(matches!(
            control.request_quit(&app),
            Err(PlatformError::Unsupported(_))
        ));
        assert!(matches!(
            control.force_kill(&app),
            Err(PlatformError::Unsupported(_))
        ));
    }
}
