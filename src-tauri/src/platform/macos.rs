//! macOS power events via NSWorkspace notifications (WillSleep / DidWake).
//!
//! Permissions: none required. Observers are registered on the shared NSWorkspace
//! notification center and stay registered for the process lifetime.

use std::ptr::NonNull;
use std::sync::mpsc::Sender;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace,
    NSWorkspaceDidWakeNotification, NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{
    NSActivityOptions, NSNotification, NSObjectProtocol, NSProcessInfo, NSString,
};

use super::focus::{FocusProbe, TitleAccess, whole_idle_seconds};
use super::process::{AppIdentity, ProcessControl, is_protected_id, normalize_app_id};
use super::{PlatformError, PowerEvent, PowerEventSource};

pub struct NsWorkspacePowerSource;

impl PowerEventSource for NsWorkspacePowerSource {
    fn start(self: Box<Self>, sender: Sender<PowerEvent>) -> Result<(), PlatformError> {
        let workspace = NSWorkspace::sharedWorkspace();
        let center = workspace.notificationCenter();

        let will_sender = sender.clone();
        let will_sleep = RcBlock::new(move |_notification: NonNull<NSNotification>| {
            // Send failure means the worker stopped; the process is shutting down.
            let _ = will_sender.send(PowerEvent::WillSleep);
        });
        let did_wake = RcBlock::new(move |_notification: NonNull<NSNotification>| {
            let _ = sender.send(PowerEvent::DidWake);
        });

        // SAFETY: the notification names are framework constants and the blocks are
        // retained by the notification center. Observer tokens are intentionally leaked:
        // they must stay registered for the whole process lifetime.
        unsafe {
            let sleep_token = center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceWillSleepNotification),
                None,
                None,
                &will_sleep,
            );
            let wake_token = center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidWakeNotification),
                None,
                None,
                &did_wake,
            );
            std::mem::forget(sleep_token);
            std::mem::forget(wake_token);
        }
        Ok(())
    }
}

/// Keeps App Nap from throttling the process while the window is hidden. Idle system sleep stays
/// allowed (`...AllowingIdleSystemSleep`): the guard must not keep the Mac awake.
pub struct MacActivity {
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

pub fn begin_activity(reason: &str) -> MacActivity {
    let info = NSProcessInfo::processInfo();
    let token = info.beginActivityWithOptions_reason(
        NSActivityOptions::UserInitiatedAllowingIdleSystemSleep,
        &NSString::from_str(reason),
    );
    MacActivity { token }
}

impl Drop for MacActivity {
    fn drop(&mut self) {
        // SAFETY: `token` came from `beginActivityWithOptions:reason:` on this same process info.
        unsafe { NSProcessInfo::processInfo().endActivity(&self.token) };
    }
}

/// Running-app control through `NSRunningApplication`.
///
/// Permissions: listing, `terminate`, and `forceTerminate` need none for apps of the same user.
/// Apps owned by another user (or protected by SIP) refuse; that surfaces as a `Process` error
/// and the engine logs `failed`. Only `Regular` activation-policy apps are listed: menu-bar
/// agents and background helpers are not blocking targets, and a quit request on the main app
/// takes its helpers down with it. Window titles are never read.
pub struct MacProcessControl;

/// Builds the identity of one running app, or `None` when it has neither a bundle id nor an
/// executable path to identify it by. Pure so the protection rules are testable.
fn build_identity(
    bundle_id: Option<&str>,
    exe_path: Option<&str>,
    name: Option<&str>,
    pid: u32,
    self_pid: u32,
) -> Option<AppIdentity> {
    let raw = bundle_id.filter(|b| !b.is_empty()).or(exe_path)?;
    let id = normalize_app_id(raw);
    let display_name = name.map(str::to_owned).unwrap_or_else(|| id.clone());
    let is_protected = pid == self_pid || is_protected_id(&id);
    Some(AppIdentity {
        id,
        display_name,
        pid,
        is_protected,
    })
}

fn pid_of(app: &AppIdentity) -> Result<i32, PlatformError> {
    i32::try_from(app.pid).map_err(|_| PlatformError::Process("pid out of range".into()))
}

/// Re-resolves the target by pid and checks it is still the same app, so a recycled pid is never
/// ended by mistake.
fn resolve(app: &AppIdentity) -> Result<Retained<NSRunningApplication>, PlatformError> {
    let running = NSRunningApplication::runningApplicationWithProcessIdentifier(pid_of(app)?)
        .ok_or_else(|| PlatformError::Process("app is no longer running".into()))?;
    let bundle = running.bundleIdentifier().map(|b| b.to_string());
    let exe = running
        .executableURL()
        .and_then(|u| u.path())
        .map(|p| p.to_string());
    let name = running.localizedName().map(|n| n.to_string());
    let current = build_identity(
        bundle.as_deref(),
        exe.as_deref(),
        name.as_deref(),
        app.pid,
        std::process::id(),
    );
    if current.as_ref().map(|c| &c.id) != Some(&app.id) {
        return Err(PlatformError::Process(
            "pid now belongs to a different app".into(),
        ));
    }
    Ok(running)
}

impl ProcessControl for MacProcessControl {
    fn list_running(&self) -> Result<Vec<AppIdentity>, PlatformError> {
        let self_pid = std::process::id();
        let apps = NSWorkspace::sharedWorkspace().runningApplications();
        let mut out = Vec::new();
        for app in apps.iter() {
            if app.activationPolicy() != NSApplicationActivationPolicy::Regular {
                continue;
            }
            let Ok(pid) = u32::try_from(app.processIdentifier()) else {
                continue;
            };
            let bundle = app.bundleIdentifier().map(|b| b.to_string());
            let exe = app
                .executableURL()
                .and_then(|u| u.path())
                .map(|p| p.to_string());
            let name = app.localizedName().map(|n| n.to_string());
            out.extend(build_identity(
                bundle.as_deref(),
                exe.as_deref(),
                name.as_deref(),
                pid,
                self_pid,
            ));
        }
        Ok(out)
    }

    fn request_quit(&self, app: &AppIdentity) -> Result<(), PlatformError> {
        if app.is_protected {
            return Err(PlatformError::Process(
                "refusing to end a protected app".into(),
            ));
        }
        if resolve(app)?.terminate() {
            Ok(())
        } else {
            Err(PlatformError::Process(
                "quit request was not delivered".into(),
            ))
        }
    }

    fn force_kill(&self, app: &AppIdentity) -> Result<(), PlatformError> {
        if app.is_protected {
            return Err(PlatformError::Process(
                "refusing to end a protected app".into(),
            ));
        }
        // `forceTerminate` sends SIGKILL to the process.
        if resolve(app)?.forceTerminate() {
            Ok(())
        } else {
            Err(PlatformError::Process("force terminate was refused".into()))
        }
    }
}

// Both symbols are plain C functions in system frameworks that are always present on macOS.
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    /// `state_id` 0 is `kCGEventSourceStateCombinedSessionState`; `event_type` `u32::MAX` is
    /// `kCGAnyInputEventType`. Returns seconds since the last such event; no event content.
    fn CGEventSourceSecondsSinceLastEventType(state_id: i32, event_type: u32) -> f64;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    /// Whether this process is a trusted accessibility client (`Boolean`, 0 or 1). Does not
    /// prompt: the prompting variant is never called.
    fn AXIsProcessTrusted() -> u8;
}

/// Foreground app and idle time. Permissions: none for either (`frontmostApplication` and the
/// combined-session idle counter need no Accessibility or Input Monitoring grant, and the
/// counter exposes no event content). Window titles are not read.
pub struct MacFocusProbe;

impl FocusProbe for MacFocusProbe {
    fn frontmost_app(&self) -> Result<Option<AppIdentity>, PlatformError> {
        let Some(app) = NSWorkspace::sharedWorkspace().frontmostApplication() else {
            return Ok(None);
        };
        let Ok(pid) = u32::try_from(app.processIdentifier()) else {
            return Ok(None);
        };
        let bundle = app.bundleIdentifier().map(|b| b.to_string());
        let exe = app
            .executableURL()
            .and_then(|u| u.path())
            .map(|p| p.to_string());
        let name = app.localizedName().map(|n| n.to_string());
        Ok(build_identity(
            bundle.as_deref(),
            exe.as_deref(),
            name.as_deref(),
            pid,
            std::process::id(),
        ))
    }

    fn idle_seconds(&self) -> Result<u64, PlatformError> {
        // SAFETY: pure query of a framework counter with constant, documented arguments.
        let raw = unsafe { CGEventSourceSecondsSinceLastEventType(0, u32::MAX) };
        whole_idle_seconds(raw)
    }

    fn window_title_access(&self) -> TitleAccess {
        // SAFETY: argument-free query with no side effects.
        if unsafe { AXIsProcessTrusted() } != 0 {
            TitleAccess::Granted
        } else {
            TitleAccess::Denied
        }
    }

    fn front_window_title(&self, pid: u32) -> Option<String> {
        if self.window_title_access() != TitleAccess::Granted {
            return None;
        }
        read_front_window_title(pid)
    }
}

/// Asks System Events for the caption of the front window of one process. The script text holds
/// only the numeric pid; the answer is returned to the caller and never logged here. Gives up
/// after two seconds so a stuck app cannot hold the sampler.
fn read_front_window_title(pid: u32) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let script = format!(
        "tell application \"System Events\" to get name of front window of (first process whose unix id is {pid})"
    );
    let mut child = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
    let mut out = String::new();
    child
        .stdout
        .take()?
        .take(4096)
        .read_to_string(&mut out)
        .ok()?;
    let title = out.trim().to_string();
    (!title.is_empty()).then_some(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_sensors_answer_without_error() {
        let probe = MacFocusProbe;
        assert!(probe.frontmost_app().is_ok());
        let first = probe.idle_seconds().expect("idle");
        assert!(
            first < 10 * 365 * 24 * 3600,
            "implausible idle reading {first}"
        );
        assert!(matches!(
            probe.window_title_access(),
            TitleAccess::Granted | TitleAccess::Denied
        ));
    }

    #[test]
    fn identity_prefers_bundle_id_and_falls_back_to_the_executable_path() {
        let by_bundle = build_identity(
            Some("Com.Example.App"),
            Some("/x/y"),
            Some("Example"),
            10,
            1,
        )
        .expect("id");
        assert_eq!(by_bundle.id, "com.example.app");
        assert_eq!(by_bundle.display_name, "Example");
        let by_path = build_identity(
            None,
            Some("/Applications/Tool.app/Contents/MacOS/Tool"),
            None,
            10,
            1,
        )
        .expect("id");
        assert_eq!(by_path.id, "/applications/tool.app/contents/macos/tool");
        assert!(build_identity(None, None, Some("Nameless"), 10, 1).is_none());
    }

    #[test]
    fn this_process_and_system_apps_are_marked_protected() {
        assert!(
            build_identity(Some("com.example.app"), None, None, 42, 42)
                .expect("id")
                .is_protected
        );
        assert!(
            build_identity(Some("com.apple.finder"), None, None, 10, 1)
                .expect("id")
                .is_protected
        );
        assert!(
            build_identity(Some("com.studymanager.app"), None, None, 10, 1)
                .expect("id")
                .is_protected
        );
        assert!(
            !build_identity(Some("com.example.app"), None, None, 10, 1)
                .expect("id")
                .is_protected
        );
    }

    #[test]
    fn real_enumeration_works_and_never_lists_this_process_unprotected() {
        let apps = MacProcessControl.list_running().expect("list");
        let me = std::process::id();
        assert!(apps.iter().filter(|a| a.pid == me).all(|a| a.is_protected));
    }

    #[test]
    fn a_protected_app_is_refused_before_any_os_call() {
        let app = AppIdentity {
            id: "com.apple.finder".into(),
            display_name: "Finder".into(),
            pid: 1,
            is_protected: true,
        };
        assert!(MacProcessControl.request_quit(&app).is_err());
        assert!(MacProcessControl.force_kill(&app).is_err());
    }
}
