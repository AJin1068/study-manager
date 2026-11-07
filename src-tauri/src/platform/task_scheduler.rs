//! Windows relaunch supervision through Task Scheduler.
//!
//! Task Scheduler's own "restart on failure" does NOT react to a process exit code (measured on
//! Windows 11: a task whose action exits non-zero or is killed is not restarted), so supervision
//! uses two per-user tasks instead, neither needing elevation:
//!
//! * `StudyManagerRelaunch` starts the app at logon (replaces the HKCU Run entry).
//! * `StudyManagerWatchdog` starts the app with `--supervised` every minute. A start that finds
//!   the app already running is dropped by the single-instance plugin; a start after a *clean*
//!   quit is dropped by the marker file below; a start after a kill or crash brings the app back
//!   within about a minute.
//!
//! Exactly one startup mechanism is active: the Run entry, or this task pair. The pure parts
//! (XML, marker, state machine over fakes) compile and are tested on every OS.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::{AutostartBackend, PlatformError};

pub const LOGON_TASK: &str = "StudyManagerRelaunch";
pub const WATCHDOG_TASK: &str = "StudyManagerWatchdog";
/// Argument the watchdog task passes; the single-instance callback ignores it.
pub const SUPERVISED_ARG: &str = "--supervised";
const MARKER_FILE: &str = "relaunch-suppress";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Trigger {
    Logon,
    Watchdog,
}

fn xml_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Task definition XML (schema 1.4). `user` is `DOMAIN\name`; the task runs as that user with an
/// interactive token and no elevation, so a non-elevated app can create and delete it.
pub fn build_task_xml(trigger: Trigger, exe: &str, user: &str) -> String {
    let user = xml_escape(user);
    let (trigger_xml, arguments) = match trigger {
        Trigger::Logon => (
            format!(
                "<LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger>"
            ),
            String::new(),
        ),
        Trigger::Watchdog => (
            "<TimeTrigger><Enabled>true</Enabled><StartBoundary>2000-01-01T00:00:00</StartBoundary>\
             <Repetition><Interval>PT1M</Interval></Repetition></TimeTrigger>"
                .to_owned(),
            format!("<Arguments>{SUPERVISED_ARG}</Arguments>"),
        ),
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n\
         <Task version=\"1.4\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\
         <Triggers>{trigger_xml}</Triggers>\
         <Principals><Principal id=\"Author\"><UserId>{user}</UserId>\
         <LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>\
         <Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\
         <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\
         <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\
         <StartWhenAvailable>true</StartWhenAvailable>\
         <ExecutionTimeLimit>PT0S</ExecutionTimeLimit></Settings>\
         <Actions Context=\"Author\"><Exec><Command>{}</Command>{arguments}</Exec></Actions></Task>",
        xml_escape(exe)
    )
}

/// `schtasks /XML` wants UTF-16 with a byte-order mark.
pub fn utf16_with_bom(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

/// Runs `schtasks` with the given arguments and returns its exit code.
pub trait SchtasksRunner: Send + Sync {
    fn run(&self, args: &[String]) -> io::Result<i32>;
}

/// Creates, queries and deletes the task pair. Names and the XML directory are injected so tests
/// never touch the real tasks.
pub struct TaskSupervisor {
    runner: Box<dyn SchtasksRunner>,
    logon_name: String,
    watchdog_name: String,
    xml_dir: PathBuf,
}

impl TaskSupervisor {
    pub fn new(
        runner: Box<dyn SchtasksRunner>,
        logon_name: &str,
        watchdog_name: &str,
        xml_dir: PathBuf,
    ) -> Self {
        Self {
            runner,
            logon_name: logon_name.to_owned(),
            watchdog_name: watchdog_name.to_owned(),
            xml_dir,
        }
    }

    fn code(&self, args: &[&str]) -> Result<i32, PlatformError> {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        self.runner
            .run(&args)
            .map_err(|e| PlatformError::Autostart(format!("schtasks could not run: {e}")))
    }

    fn present(&self, name: &str) -> Result<bool, PlatformError> {
        Ok(self.code(&["/Query", "/TN", name])? == 0)
    }

    fn create(&self, name: &str, xml: &str) -> Result<(), PlatformError> {
        std::fs::create_dir_all(&self.xml_dir)
            .map_err(|e| PlatformError::Autostart(e.to_string()))?;
        let path = self.xml_dir.join(format!("{name}.xml"));
        std::fs::write(&path, utf16_with_bom(xml))
            .map_err(|e| PlatformError::Autostart(e.to_string()))?;
        let code = self.code(&[
            "/Create",
            "/TN",
            name,
            "/XML",
            &path.to_string_lossy(),
            "/F",
        ]);
        let _ = std::fs::remove_file(&path);
        match code? {
            0 => Ok(()),
            other => Err(PlatformError::Autostart(format!(
                "schtasks /Create exited with {other}"
            ))),
        }
    }

    fn delete(&self, name: &str) -> Result<(), PlatformError> {
        if self.code(&["/Delete", "/TN", name, "/F"])? == 0 || !self.present(name)? {
            return Ok(());
        }
        Err(PlatformError::Autostart(
            "schtasks /Delete failed for an existing task".into(),
        ))
    }

    /// Creates (or replaces) both tasks; if the second fails the first is removed again.
    pub fn install(&self, exe: &str, user: &str) -> Result<(), PlatformError> {
        self.create(&self.logon_name, &build_task_xml(Trigger::Logon, exe, user))?;
        if let Err(err) = self.create(
            &self.watchdog_name,
            &build_task_xml(Trigger::Watchdog, exe, user),
        ) {
            let _ = self.delete(&self.logon_name);
            return Err(err);
        }
        Ok(())
    }

    pub fn remove(&self) -> Result<(), PlatformError> {
        self.delete(&self.watchdog_name)?;
        self.delete(&self.logon_name)
    }

    /// True when either task exists (a half pair still counts as "something is installed").
    pub fn any_present(&self) -> Result<bool, PlatformError> {
        Ok(self.present(&self.logon_name)? || self.present(&self.watchdog_name)?)
    }
}

/// The HKCU Run entry half of startup control.
pub trait RunEntry: Send + Sync {
    fn is_enabled(&self) -> Result<bool, PlatformError>;
    fn set_enabled(&self, enabled: bool) -> Result<(), PlatformError>;
}

/// Keeps exactly one startup mechanism: the Run entry when relaunch is off, the task pair when it
/// is on. `set_relaunch` re-applies the desired state whenever autostart is on, so the first
/// call at startup also heals a half-finished switch.
pub struct SupervisedAutostart<E: RunEntry> {
    run: E,
    tasks: TaskSupervisor,
    exe: String,
    user: String,
    relaunch: AtomicBool,
}

impl<E: RunEntry> SupervisedAutostart<E> {
    pub fn new(run: E, tasks: TaskSupervisor, exe: String, user: String) -> Self {
        Self {
            run,
            tasks,
            exe,
            user,
            relaunch: AtomicBool::new(false),
        }
    }

    fn apply(&self) -> Result<(), PlatformError> {
        if self.relaunch.load(Ordering::SeqCst) {
            // New mechanism first, old one second: a failure never leaves the user with neither.
            self.tasks.install(&self.exe, &self.user)?;
            self.run.set_enabled(false)
        } else {
            self.run.set_enabled(true)?;
            self.tasks.remove()
        }
    }
}

impl<E: RunEntry> AutostartBackend for SupervisedAutostart<E> {
    fn is_enabled(&self) -> Result<bool, PlatformError> {
        Ok(self.run.is_enabled()? || self.tasks.any_present()?)
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), PlatformError> {
        if enabled {
            self.apply()
        } else {
            self.tasks.remove()?;
            self.run.set_enabled(false)
        }
    }

    fn set_relaunch(&self, enabled: bool) -> Result<(), PlatformError> {
        self.relaunch.store(enabled, Ordering::SeqCst);
        if self.is_enabled()? {
            self.apply()?;
        }
        Ok(())
    }
}

/// Records that the user (or the OS) ended the app on purpose, so the watchdog leaves it alone
/// until the next normal start.
pub fn note_clean_exit(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(MARKER_FILE), b"")
}

/// Decides, before the UI boots, whether this start must be dropped. A watchdog start
/// (`--supervised`) is dropped while the clean-quit marker exists; any other start clears the
/// marker so supervision resumes.
pub fn supervised_start_suppressed(dir: &Path, args: &[String]) -> bool {
    let marker = dir.join(MARKER_FILE);
    if args.iter().any(|a| a == SUPERVISED_ARG) {
        marker.exists()
    } else {
        let _ = std::fs::remove_file(marker);
        false
    }
}

/// True when the args mark a watchdog start (the single-instance callback must not raise the
/// window for it).
pub fn is_supervised_start(args: &[String]) -> bool {
    args.iter().any(|a| a == SUPERVISED_ARG)
}

#[cfg(windows)]
pub use real::{RealRunner, current_user};

#[cfg(windows)]
mod real {
    use std::io;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    use super::SchtasksRunner;

    /// `CREATE_NO_WINDOW`: no console flash for the helper.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub struct RealRunner;

    impl SchtasksRunner for RealRunner {
        fn run(&self, args: &[String]) -> io::Result<i32> {
            let status = Command::new("schtasks")
                .args(args)
                .creation_flags(CREATE_NO_WINDOW)
                .output()?
                .status;
            Ok(status.code().unwrap_or(-1))
        }
    }

    /// `DOMAIN\name` of the signed-in user, as the task principal needs it.
    pub fn current_user() -> Option<String> {
        let name = std::env::var("USERNAME").ok()?;
        match std::env::var("USERDOMAIN") {
            Ok(domain) if !domain.is_empty() => Some(format!("{domain}\\{name}")),
            _ => Some(name),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// In-memory Task Scheduler: tracks existing task names; can be told to fail a create.
    #[derive(Default)]
    struct FakeState {
        tasks: Vec<String>,
        calls: Vec<String>,
        fail_create: Option<String>,
    }

    struct FakeRunner(Arc<Mutex<FakeState>>);

    impl SchtasksRunner for FakeRunner {
        fn run(&self, args: &[String]) -> io::Result<i32> {
            let mut s = self.0.lock().expect("lock");
            s.calls.push(args.join(" "));
            let name = args.get(2).cloned().unwrap_or_default();
            Ok(match args[0].as_str() {
                "/Query" => i32::from(!s.tasks.contains(&name)),
                "/Create" => {
                    if s.fail_create.as_deref() == Some(name.as_str()) {
                        1
                    } else {
                        if !s.tasks.contains(&name) {
                            s.tasks.push(name);
                        }
                        0
                    }
                }
                "/Delete" => {
                    let before = s.tasks.len();
                    s.tasks.retain(|t| *t != name);
                    i32::from(s.tasks.len() == before)
                }
                _ => 2,
            })
        }
    }

    #[derive(Clone, Default)]
    struct FakeRun(Arc<Mutex<bool>>);

    impl RunEntry for FakeRun {
        fn is_enabled(&self) -> Result<bool, PlatformError> {
            Ok(*self.0.lock().expect("lock"))
        }
        fn set_enabled(&self, enabled: bool) -> Result<(), PlatformError> {
            *self.0.lock().expect("lock") = enabled;
            Ok(())
        }
    }

    fn setup() -> (SupervisedAutostart<FakeRun>, FakeRun, Arc<Mutex<FakeState>>) {
        let state = Arc::new(Mutex::new(FakeState::default()));
        let dir = std::env::temp_dir().join(format!("sm-task-test-{}", std::process::id()));
        let tasks = TaskSupervisor::new(
            Box::new(FakeRunner(state.clone())),
            "T-logon",
            "T-watch",
            dir,
        );
        let run = FakeRun::default();
        let backend = SupervisedAutostart::new(
            run.clone(),
            tasks,
            "C:\\a b\\app.exe".into(),
            "PC\\me".into(),
        );
        (backend, run, state)
    }

    fn tasks(state: &Arc<Mutex<FakeState>>) -> Vec<String> {
        let mut t = state.lock().expect("lock").tasks.clone();
        t.sort();
        t
    }

    #[test]
    fn xml_has_triggers_principal_and_escaped_exe() {
        let logon = build_task_xml(Trigger::Logon, "C:\\R&D\\app.exe", "PC\\me");
        assert!(logon.contains("<LogonTrigger>"));
        assert!(logon.contains("<UserId>PC\\me</UserId>"));
        assert!(logon.contains("<Command>C:\\R&amp;D\\app.exe</Command>"));
        assert!(logon.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(!logon.contains("<Arguments>"));
        let watch = build_task_xml(Trigger::Watchdog, "C:\\app.exe", "PC\\me");
        assert!(watch.contains("<Interval>PT1M</Interval>"));
        assert!(watch.contains("<Arguments>--supervised</Arguments>"));
        assert!(watch.contains("IgnoreNew"));
    }

    #[test]
    fn utf16_encoding_starts_with_a_bom() {
        assert_eq!(utf16_with_bom("a"), vec![0xFF, 0xFE, b'a', 0]);
    }

    #[test]
    fn enabling_without_relaunch_uses_only_the_run_entry() {
        let (backend, run, state) = setup();
        backend.set_enabled(true).expect("enable");
        assert!(run.is_enabled().expect("run"));
        assert!(tasks(&state).is_empty());
        assert!(backend.is_enabled().expect("enabled"));
    }

    #[test]
    fn relaunch_with_autostart_off_creates_nothing() {
        let (backend, run, state) = setup();
        backend.set_relaunch(true).expect("relaunch");
        assert!(!run.is_enabled().expect("run"));
        assert!(tasks(&state).is_empty());
        assert!(!backend.is_enabled().expect("enabled"));
    }

    #[test]
    fn switching_relaunch_moves_between_the_run_entry_and_the_task_pair() {
        let (backend, run, state) = setup();
        backend.set_enabled(true).expect("enable");
        backend.set_relaunch(true).expect("on");
        assert!(!run.is_enabled().expect("run"));
        assert_eq!(tasks(&state), vec!["T-logon", "T-watch"]);
        backend.set_relaunch(false).expect("off");
        assert!(run.is_enabled().expect("run"));
        assert!(tasks(&state).is_empty());
    }

    #[test]
    fn enabling_with_relaunch_set_installs_tasks_and_disabling_removes_everything() {
        let (backend, run, state) = setup();
        backend.set_relaunch(true).expect("relaunch");
        backend.set_enabled(true).expect("enable");
        assert_eq!(tasks(&state), vec!["T-logon", "T-watch"]);
        assert!(!run.is_enabled().expect("run"));
        backend.set_enabled(false).expect("disable");
        assert!(tasks(&state).is_empty());
        assert!(!run.is_enabled().expect("run"));
        assert!(!backend.is_enabled().expect("enabled"));
    }

    #[test]
    fn a_failed_task_create_keeps_the_run_entry_and_rolls_back_the_first_task() {
        let (backend, run, state) = setup();
        backend.set_enabled(true).expect("enable");
        state.lock().expect("lock").fail_create = Some("T-watch".into());
        assert!(backend.set_relaunch(true).is_err());
        assert!(run.is_enabled().expect("run"), "never left with neither");
        assert!(tasks(&state).is_empty());
    }

    #[test]
    fn startup_reconcile_repairs_every_partial_state() {
        // Both mechanisms present, relaunch on: Run entry goes away.
        let (backend, run, state) = setup();
        run.set_enabled(true).expect("run");
        state.lock().expect("lock").tasks = vec!["T-logon".into(), "T-watch".into()];
        backend.set_relaunch(true).expect("reconcile");
        assert!(!run.is_enabled().expect("run"));
        assert_eq!(tasks(&state), vec!["T-logon", "T-watch"]);

        // Half a pair, relaunch on: the pair is completed.
        let (backend, run, state) = setup();
        state.lock().expect("lock").tasks = vec!["T-logon".into()];
        backend.set_relaunch(true).expect("reconcile");
        assert_eq!(tasks(&state), vec!["T-logon", "T-watch"]);
        assert!(!run.is_enabled().expect("run"));

        // Tasks left behind, relaunch off: Run entry is restored and tasks removed.
        let (backend, run, state) = setup();
        state.lock().expect("lock").tasks = vec!["T-watch".into()];
        backend.set_relaunch(false).expect("reconcile");
        assert!(run.is_enabled().expect("run"));
        assert!(tasks(&state).is_empty());

        // Neither present: nothing is created.
        let (backend, run, state) = setup();
        backend.set_relaunch(true).expect("reconcile");
        assert!(!run.is_enabled().expect("run"));
        assert!(tasks(&state).is_empty());
    }

    #[test]
    fn watchdog_start_is_dropped_only_after_a_clean_quit_until_a_normal_start() {
        let dir = std::env::temp_dir().join(format!("sm-marker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let supervised = vec![SUPERVISED_ARG.to_owned()];
        assert!(
            !supervised_start_suppressed(&dir, &supervised),
            "crash path restarts"
        );
        note_clean_exit(&dir).expect("marker");
        assert!(supervised_start_suppressed(&dir, &supervised));
        assert!(
            supervised_start_suppressed(&dir, &supervised),
            "stays suppressed"
        );
        assert!(
            !supervised_start_suppressed(&dir, &[]),
            "normal start proceeds"
        );
        assert!(
            !supervised_start_suppressed(&dir, &supervised),
            "and re-arms the watchdog"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real Task Scheduler with throwaway names: proves the generated XML is accepted without
    /// elevation. Always cleans up.
    #[cfg(windows)]
    #[test]
    fn real_task_scheduler_accepts_the_generated_pair_and_removes_it() {
        let pid = std::process::id();
        let (logon, watch) = (
            format!("StudyManagerTest-{pid}-a"),
            format!("StudyManagerTest-{pid}-b"),
        );
        let tasks = TaskSupervisor::new(
            Box::new(RealRunner),
            &logon,
            &watch,
            std::env::temp_dir().join(format!("sm-task-live-{pid}")),
        );
        let user = current_user().expect("user");
        let exe = std::env::current_exe().expect("exe").display().to_string();
        let installed = tasks.install(&exe, &user);
        let present = tasks.any_present();
        let removed = tasks.remove();
        let after = tasks.any_present();
        installed.expect("install");
        assert!(present.expect("query"));
        removed.expect("remove");
        assert!(!after.expect("query after"));
    }

    #[test]
    fn supervised_arg_is_detected() {
        assert!(is_supervised_start(&["--supervised".into()]));
        assert!(!is_supervised_start(&["--other".into()]));
    }
}
