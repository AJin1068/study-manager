//! Windows power events. Modern Standby (S0 low-power) wake/sleep messages are unverified on
//! real hardware, so this source reports `Unsupported` until S7a passes. The core then
//! relies on heartbeat-gap detection, and the status is shown in the UI as "검증 안 됨".

use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::mpsc::Sender;

use super::process::{AppIdentity, ProcessControl, is_protected_id, normalize_app_id};
use super::{PlatformError, PowerEvent, PowerEventSource};

pub struct WindowsPowerSource;

impl PowerEventSource for WindowsPowerSource {
    fn start(self: Box<Self>, _sender: Sender<PowerEvent>) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported(
            "Windows power events pending S7a real-device verification; heartbeat fallback only",
        ))
    }
}

/// `CREATE_NO_WINDOW`: keeps a console window from flashing for every helper command.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Process control through PowerShell (listing) and `taskkill` (quit/kill). Windows-only and
/// **not verified on a real machine**; see docs/stage-1-3-verification.md.
///
/// Permissions: no elevation is needed for processes of the same user. Elevated or other-user
/// processes refuse, which surfaces as a `Process` error and a `failed` log. `taskkill /PID`
/// without `/F` posts `WM_CLOSE` to the process's top-level windows (the normal-quit request);
/// `/F` is `TerminateProcess`. Child processes are not touched (no `/T`). Listing is limited to
/// processes that own a main window, and only the process id and executable path are read.
pub struct WindowsProcessControl;

/// Parses `pid<TAB>path` lines into identities. Pure so it is testable without Windows.
fn parse_listing(output: &str, self_pid: u32) -> Vec<AppIdentity> {
    let mut apps = Vec::new();
    for line in output.lines() {
        let Some((pid, path)) = line.trim().split_once('\t') else {
            continue;
        };
        let Ok(pid) = pid.trim().parse::<u32>() else {
            continue;
        };
        let id = normalize_app_id(path);
        if id.is_empty() {
            continue;
        }
        let display_name = id
            .rsplit('/')
            .next()
            .unwrap_or(&id)
            .trim_end_matches(".exe")
            .to_owned();
        let is_protected = pid == self_pid || is_protected_id(&id);
        apps.push(AppIdentity {
            id,
            display_name,
            pid,
            is_protected,
        });
    }
    apps
}

fn run(program: &str, args: &[&str]) -> Result<std::process::Output, PlatformError> {
    Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| PlatformError::Process(format!("could not run {program}: {e}")))
}

fn taskkill(app: &AppIdentity, force: bool) -> Result<(), PlatformError> {
    if app.is_protected {
        return Err(PlatformError::Process(
            "refusing to end a protected app".into(),
        ));
    }
    // The pid may have been recycled since listing: confirm it still runs the same executable.
    let still_same = WindowsProcessControl
        .list_running()?
        .iter()
        .any(|current| current.pid == app.pid && current.id == app.id);
    if !still_same {
        return Err(PlatformError::Process("app is no longer running".into()));
    }
    let pid = app.pid.to_string();
    let args: Vec<&str> = if force {
        vec!["/F", "/PID", pid.as_str()]
    } else {
        vec!["/PID", pid.as_str()]
    };
    let output = run("taskkill", &args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(PlatformError::Process(format!(
            "taskkill exited with {}",
            output.status
        )))
    }
}

impl ProcessControl for WindowsProcessControl {
    fn list_running(&self) -> Result<Vec<AppIdentity>, PlatformError> {
        let script = "Get-Process | Where-Object { $_.MainWindowHandle -ne 0 -and $_.Path } | \
             ForEach-Object { \"$($_.Id)`t$($_.Path)\" }";
        let output = run(
            "powershell",
            &["-NoProfile", "-NonInteractive", "-Command", script],
        )?;
        if !output.status.success() {
            return Err(PlatformError::Process(format!(
                "process listing exited with {}",
                output.status
            )));
        }
        Ok(parse_listing(
            &String::from_utf8_lossy(&output.stdout),
            std::process::id(),
        ))
    }

    fn request_quit(&self, app: &AppIdentity) -> Result<(), PlatformError> {
        taskkill(app, false)
    }

    fn force_kill(&self, app: &AppIdentity) -> Result<(), PlatformError> {
        taskkill(app, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_is_parsed_into_normalized_identities_and_protection_flags() {
        let out = "10\tC:\\Games\\Steam\\Steam.exe\r\n20\tC:\\Windows\\explorer.exe\r\n30\tC:\\Apps\\Study-Manager.exe\r\nbad line\r\n";
        let apps = parse_listing(out, 99);
        assert_eq!(apps.len(), 3);
        assert_eq!(apps[0].id, "c:/games/steam/steam.exe");
        assert_eq!(apps[0].display_name, "steam");
        assert!(!apps[0].is_protected);
        assert!(apps[1].is_protected);
        assert!(apps[2].is_protected);
        assert!(parse_listing("99\tC:\\Games\\x.exe", 99)[0].is_protected);
    }
}
