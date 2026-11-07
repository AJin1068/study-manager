//! Windows implementations of the platform traits: process control, foreground/idle sensors,
//! suspend/resume notifications and the background-throttling opt-out. All Win32 calls live here
//! (and in the sibling Windows modules); callers only see the OS-neutral traits.
//!
//! Permissions: nothing here needs elevation. Processes of the same user can be listed, asked to
//! close and terminated. Elevated or other-user processes refuse (`OpenProcess` fails, or UIPI
//! drops the posted `WM_CLOSE`), which surfaces as a `Process` error and a `failed` log, never as
//! `Unsupported`. Window captions are read only by `front_window_title`; no other function in
//! this file touches them, and no error message carries one.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;

use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE, HWND, LPARAM, TRUE};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows_sys::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, RegisterSuspendResumeNotification,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, IsProcessCritical, OpenProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
    PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    ProcessPowerThrottling, QueryFullProcessImageNameW, SetProcessInformation, TerminateProcess,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetForegroundWindow, GetWindow, GetWindowLongW,
    GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
    WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};
use windows_sys::core::BOOL;

use super::focus::{FocusProbe, TitleAccess, whole_idle_seconds};
use super::process::{AppIdentity, ProcessControl, is_protected_id, normalize_app_id};
use super::{PlatformError, PowerEvent, PowerEventSource};

// ---- process handles and top-level windows ----

/// Closes the handle on drop. Pinning a handle between "verify identity" and "act" is what stops
/// a recycled pid from being quit or killed.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful OpenProcess and is closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

fn open_process(pid: u32, access: u32) -> Option<OwnedHandle> {
    // SAFETY: plain FFI call; a null result means failure and is checked.
    let handle = unsafe { OpenProcess(access, FALSE, pid) };
    if handle.is_null() {
        None
    } else {
        Some(OwnedHandle(handle))
    }
}

/// Executable path of an open process, or `None` when the OS refuses.
fn image_path(handle: &OwnedHandle) -> Option<String> {
    let mut capacity: usize = 260;
    loop {
        let mut buf = vec![0u16; capacity];
        let mut len = capacity as u32;
        // SAFETY: `buf` holds `len` UTF-16 units and outlives the call.
        let ok = unsafe { QueryFullProcessImageNameW(handle.0, 0, buf.as_mut_ptr(), &mut len) };
        if ok != 0 {
            return Some(String::from_utf16_lossy(&buf[..len as usize]));
        }
        if capacity >= 32768 {
            return None;
        }
        capacity *= 4;
    }
}

/// Builds an identity from an executable path. Pure so it is testable without Windows.
fn identity_from_path(pid: u32, path: &str, self_pid: u32) -> Option<AppIdentity> {
    let id = normalize_app_id(path);
    if id.is_empty() {
        return None;
    }
    let display_name = id
        .rsplit('/')
        .next()
        .unwrap_or(&id)
        .trim_end_matches(".exe")
        .to_owned();
    let is_protected = pid == self_pid || is_protected_id(&id);
    Some(AppIdentity {
        id,
        display_name,
        pid,
        is_protected,
    })
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid out pointer for the call.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid
}

/// Visible, unowned top-level windows that a user would call an app window: no tool windows
/// (unless they ask for a taskbar button) and nothing DWM has cloaked (other virtual desktops,
/// suspended UWP shells). Returns `(window, pid)`; captions are never read.
fn app_windows() -> Vec<(HWND, u32)> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam is the `&mut Vec` passed by `app_windows` below, alive for the call.
        let out = unsafe { &mut *(lparam as *mut Vec<(HWND, u32)>) };
        // SAFETY: plain FFI calls on a window handle from EnumWindows.
        unsafe {
            if IsWindowVisible(hwnd) == 0 || !GetWindow(hwnd, GW_OWNER).is_null() {
                return TRUE;
            }
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW != 0 && ex & WS_EX_APPWINDOW == 0 {
                return TRUE;
            }
            let mut cloaked = 0u32;
            let hr = DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED as u32,
                (&mut cloaked as *mut u32).cast::<c_void>(),
                std::mem::size_of::<u32>() as u32,
            );
            if hr == 0 && cloaked != 0 {
                return TRUE;
            }
        }
        let pid = window_pid(hwnd);
        if pid != 0 {
            out.push((hwnd, pid));
        }
        TRUE
    }

    let mut windows: Vec<(HWND, u32)> = Vec::new();
    // SAFETY: the callback only touches the vector for the duration of this call.
    unsafe {
        EnumWindows(
            Some(collect),
            &mut windows as *mut Vec<(HWND, u32)> as LPARAM,
        )
    };
    windows
}

/// Process control through Win32 directly: `EnumWindows` for listing, `PostMessageW(WM_CLOSE)`
/// for the normal quit request, `TerminateProcess` for the forced end. Child processes are not
/// touched. UWP apps run under the protected `ApplicationFrameHost.exe` and cannot be block-listed.
pub struct WindowsProcessControl;

/// Opens `app.pid`, refuses protected/self/critical processes, and confirms the pid still runs the
/// executable that was listed. The returned handle stays open so the pid cannot be recycled
/// before the caller acts.
fn verified_target(app: &AppIdentity, access: u32) -> Result<OwnedHandle, PlatformError> {
    // Protection is recomputed from the id, so an identity built by hand cannot bypass it.
    if app.is_protected || is_protected_id(&app.id) || app.pid == std::process::id() {
        return Err(PlatformError::Process(
            "refusing to end a protected app".into(),
        ));
    }
    let handle =
        open_process(app.pid, access | PROCESS_QUERY_LIMITED_INFORMATION).ok_or_else(|| {
            PlatformError::Process("app is not accessible or no longer running".into())
        })?;
    match image_path(&handle) {
        Some(path) if normalize_app_id(&path) == app.id => {}
        _ => return Err(PlatformError::Process("app is no longer running".into())),
    }
    let mut critical: BOOL = FALSE;
    // SAFETY: valid process handle and out pointer.
    if unsafe { IsProcessCritical(handle.0, &mut critical) } != 0 && critical != FALSE {
        return Err(PlatformError::Process(
            "refusing to end a critical system process".into(),
        ));
    }
    Ok(handle)
}

impl ProcessControl for WindowsProcessControl {
    fn list_running(&self) -> Result<Vec<AppIdentity>, PlatformError> {
        let self_pid = std::process::id();
        let mut pids: Vec<u32> = app_windows().into_iter().map(|(_, pid)| pid).collect();
        pids.sort_unstable();
        pids.dedup();
        let mut apps = Vec::new();
        for pid in pids {
            let Some(handle) = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
                continue;
            };
            if let Some(identity) =
                image_path(&handle).and_then(|path| identity_from_path(pid, &path, self_pid))
            {
                apps.push(identity);
            }
        }
        Ok(apps)
    }

    fn request_quit(&self, app: &AppIdentity) -> Result<(), PlatformError> {
        let _pinned = verified_target(app, 0)?;
        let mut posted = 0usize;
        for (hwnd, pid) in app_windows() {
            if pid != app.pid {
                continue;
            }
            // SAFETY: PostMessageW with a plain message; never blocks on the target.
            if unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) } != 0 {
                posted += 1;
            }
        }
        if posted == 0 {
            return Err(PlatformError::Process(
                "no window accepted the close request".into(),
            ));
        }
        Ok(())
    }

    fn force_kill(&self, app: &AppIdentity) -> Result<(), PlatformError> {
        let handle = verified_target(app, PROCESS_TERMINATE)?;
        // SAFETY: valid handle opened with PROCESS_TERMINATE.
        if unsafe { TerminateProcess(handle.0, 1) } == 0 {
            return Err(PlatformError::Process(format!(
                "TerminateProcess failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }
}

// ---- foreground app and idle ----

/// Foreground and idle sensors from `GetForegroundWindow` and `GetLastInputInfo`. Only the
/// owning executable and a count of idle seconds leave this type; `GetLastInputInfo` reports a
/// single tick count, not key or pointer content.
pub struct WindowsFocusProbe;

/// Milliseconds between two `GetTickCount` readings, correct across the 49.7-day wraparound.
fn idle_ms(now: u32, last_input: u32) -> u32 {
    now.wrapping_sub(last_input)
}

impl FocusProbe for WindowsFocusProbe {
    fn frontmost_app(&self) -> Result<Option<AppIdentity>, PlatformError> {
        // SAFETY: no arguments.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_null() {
            return Ok(None);
        }
        let pid = window_pid(hwnd);
        if pid == 0 {
            return Ok(None);
        }
        let handle = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION)
            .ok_or_else(|| PlatformError::Process("foreground process is not readable".into()))?;
        image_path(&handle)
            .and_then(|path| identity_from_path(pid, &path, std::process::id()))
            .map(Some)
            .ok_or_else(|| PlatformError::Process("foreground process has no image path".into()))
    }

    fn idle_seconds(&self) -> Result<u64, PlatformError> {
        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // SAFETY: `info.cbSize` is set as the API requires.
        if unsafe { GetLastInputInfo(&mut info) } == 0 {
            return Err(PlatformError::Process(
                "input idle reading is unavailable".into(),
            ));
        }
        // SAFETY: no arguments.
        let now = unsafe { GetTickCount() };
        whole_idle_seconds(f64::from(idle_ms(now, info.dwTime)) / 1000.0)
    }

    fn window_title_access(&self) -> TitleAccess {
        TitleAccess::Granted
    }

    fn front_window_title(&self, pid: u32) -> Option<String> {
        // SAFETY: no arguments.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_null() || window_pid(hwnd) != pid {
            return None;
        }
        let mut buf = [0u16; 512];
        // SAFETY: `buf` holds 512 units; the API truncates to it.
        let len = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        if len <= 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

// ---- power events ----

/// Suspend/resume notifications through `RegisterSuspendResumeNotification` with a callback, so
/// no hidden window or message loop is needed. On Modern Standby hardware the OS may not deliver
/// suspend; the core's heartbeat-gap detection still records those sleeps.
pub struct WindowsPowerSource;

const DEVICE_NOTIFY_CALLBACK: u32 = 2;
const PBT_APMSUSPEND: u32 = 0x0004;
const PBT_APMRESUMESUSPEND: u32 = 0x0007;
const PBT_APMRESUMEAUTOMATIC: u32 = 0x0012;

/// `PBT_APMRESUMESUSPEND` is ignored: it follows `PBT_APMRESUMEAUTOMATIC` after a user-triggered
/// wake and would report the same wake twice.
fn map_power_broadcast(kind: u32) -> Option<PowerEvent> {
    match kind {
        PBT_APMSUSPEND => Some(PowerEvent::WillSleep),
        PBT_APMRESUMEAUTOMATIC => Some(PowerEvent::DidWake),
        PBT_APMRESUMESUSPEND => None,
        _ => None,
    }
}

unsafe extern "system" fn power_callback(
    context: *const c_void,
    kind: u32,
    _setting: *const c_void,
) -> u32 {
    // SAFETY: `context` is the leaked `Mutex<Sender>` registered in `start`; it is never freed.
    let sender = unsafe { &*(context as *const Mutex<Sender<PowerEvent>>) };
    if let Some(event) = map_power_broadcast(kind)
        && let Ok(sender) = sender.lock()
    {
        let _ = sender.send(event);
    }
    0
}

impl PowerEventSource for WindowsPowerSource {
    fn start(self: Box<Self>, sender: Sender<PowerEvent>) -> Result<(), PlatformError> {
        // Leaked on purpose: the registration lives for the whole process.
        let context = Box::into_raw(Box::new(Mutex::new(sender)));
        let params = Box::into_raw(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(power_callback),
            Context: context.cast::<c_void>(),
        }));
        // SAFETY: `params` is a valid DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS for the DEVICE_NOTIFY_CALLBACK flag.
        let registration = unsafe {
            RegisterSuspendResumeNotification(params.cast::<c_void>(), DEVICE_NOTIFY_CALLBACK)
        };
        if registration == 0 {
            let err = std::io::Error::last_os_error();
            // SAFETY: registration failed, so nothing else holds these pointers.
            unsafe {
                drop(Box::from_raw(params));
                drop(Box::from_raw(context));
            }
            return Err(PlatformError::Process(format!(
                "suspend/resume registration failed: {err}"
            )));
        }
        Ok(())
    }
}

// ---- background activity guard ----

/// Number of live guards; throttling is restored when the last one drops.
static ACTIVE_GUARDS: AtomicUsize = AtomicUsize::new(0);

/// Holds the process out of Windows power throttling (EcoQoS and timer-resolution coalescing) so
/// the study timer keeps ticking while the window is hidden. This is the analogue of the macOS
/// App Nap opt-out; unlike `SetThreadExecutionState` it does not keep the PC awake.
pub struct WinActivity(());

fn set_throttling(control_mask: u32) -> bool {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: control_mask,
        StateMask: 0,
    };
    // SAFETY: `state` is a valid PROCESS_POWER_THROTTLING_STATE of the stated size.
    unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            (&state as *const PROCESS_POWER_THROTTLING_STATE).cast::<c_void>(),
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        ) != 0
    }
}

pub fn begin_activity() -> Result<WinActivity, PlatformError> {
    let both =
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION;
    if !set_throttling(both) && !set_throttling(PROCESS_POWER_THROTTLING_EXECUTION_SPEED) {
        return Err(PlatformError::Process(format!(
            "power throttling opt-out rejected: {}",
            std::io::Error::last_os_error()
        )));
    }
    ACTIVE_GUARDS.fetch_add(1, Ordering::SeqCst);
    Ok(WinActivity(()))
}

impl Drop for WinActivity {
    fn drop(&mut self) {
        if ACTIVE_GUARDS.fetch_sub(1, Ordering::SeqCst) == 1 {
            set_throttling(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_become_normalized_identities_with_protection_flags() {
        let steam = identity_from_path(10, "C:\\Games\\Steam\\Steam.exe", 99).expect("steam");
        assert_eq!(steam.id, "c:/games/steam/steam.exe");
        assert_eq!(steam.display_name, "steam");
        assert!(!steam.is_protected);
        assert!(
            identity_from_path(20, "C:\\Windows\\explorer.exe", 99)
                .expect("x")
                .is_protected
        );
        assert!(
            identity_from_path(30, "C:\\Apps\\Study-Manager.exe", 99)
                .expect("self")
                .is_protected
        );
        assert!(
            identity_from_path(99, "C:\\Games\\x.exe", 99)
                .expect("own pid")
                .is_protected
        );
        assert!(identity_from_path(1, "  ", 99).is_none());
    }

    #[test]
    fn power_broadcasts_map_to_one_event_each() {
        assert_eq!(
            map_power_broadcast(PBT_APMSUSPEND),
            Some(PowerEvent::WillSleep)
        );
        assert_eq!(
            map_power_broadcast(PBT_APMRESUMEAUTOMATIC),
            Some(PowerEvent::DidWake)
        );
        assert_eq!(map_power_broadcast(PBT_APMRESUMESUSPEND), None);
        assert_eq!(map_power_broadcast(0x0A), None);
    }

    #[test]
    fn idle_survives_tick_counter_wraparound() {
        assert_eq!(idle_ms(5_000, 2_000), 3_000);
        assert_eq!(idle_ms(1_000, u32::MAX - 999), 2_000);
        assert_eq!(idle_ms(7, 7), 0);
    }

    #[test]
    fn idle_and_foreground_sensors_answer_on_this_machine() {
        let probe = WindowsFocusProbe;
        assert!(probe.idle_seconds().is_ok());
        // Foreground may be none (locked, headless CI) or unreadable (elevated); both are fine,
        // but the call must not panic.
        let _ = probe.frontmost_app();
        assert_eq!(probe.window_title_access(), TitleAccess::Granted);
    }

    #[test]
    fn power_registration_succeeds_and_does_not_emit_spuriously() {
        let (tx, rx) = std::sync::mpsc::channel();
        Box::new(WindowsPowerSource).start(tx).expect("register");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn activity_guards_nest_and_restore() {
        let a = begin_activity().expect("first");
        let b = begin_activity().expect("second");
        assert!(ACTIVE_GUARDS.load(Ordering::SeqCst) >= 2);
        drop(a);
        drop(b);
    }

    /// Runs a throwaway copy of powershell.exe that shows a Forms window, so the target is not a
    /// protected system path. Returns `None` when no desktop is available.
    fn spawn_victim(tag: &str) -> Option<(std::process::Child, AppIdentity, std::path::PathBuf)> {
        let dir = std::env::temp_dir().join(format!("sm-victim-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let exe = dir.join(format!("smvictim{tag}.exe"));
        let source = std::path::Path::new(&std::env::var_os("SystemRoot")?)
            .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        std::fs::copy(source, &exe).ok()?;
        let child = std::process::Command::new(&exe)
            .args([
                "-NoProfile",
                "-Command",
                "Add-Type -AssemblyName System.Windows.Forms; \
                 [System.Windows.Forms.Application]::Run((New-Object System.Windows.Forms.Form))",
            ])
            .spawn()
            .ok()?;
        let pid = child.id();
        let control = WindowsProcessControl;
        for _ in 0..100 {
            if let Some(app) = control
                .list_running()
                .ok()?
                .into_iter()
                .find(|a| a.pid == pid)
            {
                return Some((child, app, dir));
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let mut child = child;
        let _ = child.kill();
        let _ = std::fs::remove_dir_all(&dir);
        None
    }

    #[test]
    fn live_window_is_listed_asked_to_close_and_gone() {
        let Some((mut child, app, dir)) = spawn_victim("q") else {
            eprintln!("no desktop window available; live quit test skipped");
            return;
        };
        assert!(!app.is_protected);
        WindowsProcessControl
            .request_quit(&app)
            .expect("close request");
        let mut exited = false;
        for _ in 0..50 {
            if child.try_wait().expect("wait").is_some() {
                exited = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        if !exited {
            let _ = child.kill();
        }
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(dir);
        assert!(exited, "WM_CLOSE should end the form app");
    }

    #[test]
    fn live_window_is_force_killed_and_a_stale_identity_is_refused() {
        let Some((mut child, app, dir)) = spawn_victim("k") else {
            eprintln!("no desktop window available; live kill test skipped");
            return;
        };
        let mut wrong = app.clone();
        wrong.id = "c:/other/thing.exe".into();
        assert!(WindowsProcessControl.force_kill(&wrong).is_err());
        assert!(child.try_wait().expect("wait").is_none());
        WindowsProcessControl.force_kill(&app).expect("terminate");
        let status = child.wait().expect("reaped");
        let _ = std::fs::remove_dir_all(dir);
        assert_eq!(status.code(), Some(1));
    }

    #[test]
    fn a_hand_built_identity_cannot_bypass_the_protected_list() {
        let mut forged = identity_from_path(4242, "C:\\Windows\\explorer.exe", 1).expect("id");
        forged.is_protected = false;
        assert!(WindowsProcessControl.request_quit(&forged).is_err());
        assert!(WindowsProcessControl.force_kill(&forged).is_err());
    }

    #[test]
    fn this_process_is_never_a_valid_target() {
        let me = identity_from_path(
            std::process::id(),
            "C:\\x\\anything.exe",
            std::process::id(),
        )
        .expect("identity");
        assert!(WindowsProcessControl.request_quit(&me).is_err());
        assert!(WindowsProcessControl.force_kill(&me).is_err());
    }
}
