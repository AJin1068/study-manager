//! macOS login item written as our own LaunchAgent so it can carry
//! `KeepAlive { SuccessfulExit = false }`, which tauri-plugin-autostart cannot express.
//!
//! launchd relaunches the app only when it exits with a non-zero status or by a signal
//! (`kill -9`, crash). A normal exit (status 0: tray quit, Cmd+Q through the study-lock gate)
//! is not relaunched, so an intended quit is never fought. The file name and label match the
//! plugin's (`~/Library/LaunchAgents/<package name>.plist`), so an entry the plugin wrote
//! earlier is taken over rather than duplicated.
//!
//! Nothing here calls `launchctl`: the agent is loaded by launchd at the next login, and the
//! running instance is never touched. Only compiled on macOS (and in tests, which use a temp dir).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::{AutostartBackend, PlatformError};

fn escape_xml(raw: &str) -> String {
    raw.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Plist text. `RunAtLoad` always (it is a login item); `KeepAlive` only when `relaunch`.
/// `ThrottleInterval` bounds a crash loop to one start per 10 seconds.
pub fn build_plist(label: &str, program_args: &[String], bundle_id: &str, relaunch: bool) -> String {
    let args: String = program_args
        .iter()
        .map(|arg| format!("    <string>{}</string>\n", escape_xml(arg)))
        .collect();
    let keep_alive = if relaunch {
        "  <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n  \
         <key>ThrottleInterval</key>\n  <integer>10</integer>\n"
    } else {
        ""
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n  \
         <key>Label</key>\n  <string>{label}</string>\n  \
         <key>AssociatedBundleIdentifiers</key>\n  <array>\n    <string>{bundle}</string>\n  </array>\n  \
         <key>ProgramArguments</key>\n  <array>\n{args}  </array>\n  \
         <key>RunAtLoad</key>\n  <true/>\n\
         {keep_alive}\
         </dict>\n\
         </plist>\n",
        label = escape_xml(label),
        bundle = escape_xml(bundle_id),
    )
}

pub struct LaunchAgentAutostart {
    file: PathBuf,
    label: String,
    program_args: Vec<String>,
    bundle_id: String,
    /// Desired relaunch supervision. Applied to the plist whenever it is written, so it also
    /// holds while autostart is off (no file exists then, hence no supervision either).
    relaunch: AtomicBool,
}

impl LaunchAgentAutostart {
    pub fn new(agents_dir: &Path, label: &str, program_args: Vec<String>, bundle_id: &str) -> Self {
        Self {
            file: agents_dir.join(format!("{label}.plist")),
            label: label.to_owned(),
            program_args,
            bundle_id: bundle_id.to_owned(),
            relaunch: AtomicBool::new(false),
        }
    }

    fn write(&self) -> Result<(), PlatformError> {
        let io = |e: std::io::Error| PlatformError::Autostart(e.to_string());
        let dir = self.file.parent().ok_or_else(|| PlatformError::Autostart("launch agent path has no parent".into()))?;
        fs::create_dir_all(dir).map_err(io)?;
        let text = build_plist(&self.label, &self.program_args, &self.bundle_id, self.relaunch.load(Ordering::SeqCst));
        // Temp file + rename: launchd never sees a half-written plist.
        let tmp = self.file.with_extension("plist.tmp");
        let mut out = fs::File::create(&tmp).map_err(io)?;
        out.write_all(text.as_bytes()).map_err(io)?;
        out.sync_all().map_err(io)?;
        fs::rename(&tmp, &self.file).map_err(io)
    }
}

impl AutostartBackend for LaunchAgentAutostart {
    fn is_enabled(&self) -> Result<bool, PlatformError> {
        Ok(self.file.exists())
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), PlatformError> {
        if enabled {
            self.write()
        } else if self.file.exists() {
            fs::remove_file(&self.file).map_err(|e| PlatformError::Autostart(e.to_string()))
        } else {
            Ok(())
        }
    }

    fn set_relaunch(&self, enabled: bool) -> Result<(), PlatformError> {
        self.relaunch.store(enabled, Ordering::SeqCst);
        if self.file.exists() {
            self.write()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sm-launch-agent-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn backend(dir: &Path) -> LaunchAgentAutostart {
        LaunchAgentAutostart::new(dir, "Study Manager", vec!["/Apps/Study Manager.app/Contents/MacOS/study-manager".into()], "com.studymanager.app")
    }

    fn contents(dir: &Path) -> String {
        fs::read_to_string(dir.join("Study Manager.plist")).expect("plist")
    }

    #[test]
    fn plist_with_relaunch_keeps_the_login_item_and_relaunches_only_unsuccessful_exits() {
        let text = build_plist("L", &["/bin/app".into()], "com.x", true);
        assert!(text.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert!(text.contains("<key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>"));
        assert!(text.contains("<key>ThrottleInterval</key>\n  <integer>10</integer>"));
        assert!(text.contains("<key>Label</key>\n  <string>L</string>"));
        assert!(text.contains("<string>/bin/app</string>"));
        assert!(text.contains("<string>com.x</string>"));
    }

    #[test]
    fn plist_without_relaunch_has_no_keep_alive() {
        let text = build_plist("L", &["/bin/app".into()], "com.x", false);
        assert!(text.contains("<key>RunAtLoad</key>"));
        assert!(!text.contains("KeepAlive"));
        assert!(!text.contains("SuccessfulExit"));
    }

    #[test]
    fn plist_escapes_xml_in_paths() {
        let text = build_plist("A&B", &["/Apps/<x>&y".into()], "com.x", false);
        assert!(text.contains("<string>A&amp;B</string>"));
        assert!(text.contains("<string>/Apps/&lt;x&gt;&amp;y</string>"));
    }

    #[test]
    fn enable_and_disable_follow_the_file() {
        let dir = temp_dir("enable");
        let agent = backend(&dir);
        assert!(!agent.is_enabled().expect("read"));
        agent.set_enabled(true).expect("enable");
        assert!(agent.is_enabled().expect("read"));
        assert!(contents(&dir).contains("<key>RunAtLoad</key>"));
        agent.set_enabled(false).expect("disable");
        assert!(!agent.is_enabled().expect("read"));
        agent.set_enabled(false).expect("disabling twice is fine");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn relaunch_toggle_rewrites_an_existing_plist_and_survives_re_enable() {
        let dir = temp_dir("toggle");
        let agent = backend(&dir);
        agent.set_relaunch(true).expect("flag before the file exists");
        assert!(!agent.is_enabled().expect("read"), "relaunch alone must not create a login item");
        agent.set_enabled(true).expect("enable");
        assert!(contents(&dir).contains("KeepAlive"));
        agent.set_relaunch(false).expect("off");
        assert!(!contents(&dir).contains("KeepAlive"));
        assert!(contents(&dir).contains("RunAtLoad"), "login item behaviour stays");
        agent.set_relaunch(true).expect("on");
        agent.set_enabled(false).expect("disable");
        agent.set_enabled(true).expect("re-enable");
        assert!(contents(&dir).contains("KeepAlive"), "desired flag is kept across disable/enable");
        let _ = fs::remove_dir_all(&dir);
    }
}
