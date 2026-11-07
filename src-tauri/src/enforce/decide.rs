//! The pure part of enforcement: given what is running, the block list, and what was already
//! done, which actions come next. Nothing here touches the OS or the database.

use std::collections::{HashMap, HashSet};

use crate::platform::AppIdentity;
use crate::timer::model::Phase;

use super::model::EnforceMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub mode: EnforceMode,
    pub grace_ms: i64,
    pub force_quit_allowed: bool,
}

/// An app the engine already asked to quit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requested {
    pub app: AppIdentity,
    pub at_ms: i64,
    /// A force kill was attempted; nothing further is done for this pid.
    pub forced: bool,
}

/// What the engine remembers within one study segment. Cleared when the segment changes or the
/// study block ends, so a break or the next segment starts clean.
#[derive(Default, Debug)]
pub struct Tracker {
    scope: Option<(Option<i64>, Option<i64>)>,
    pub requested: HashMap<u32, Requested>,
    reported: HashSet<(u32, &'static str)>,
    /// The last app listing failed, so the failure is logged once rather than every poll.
    pub list_failed: bool,
}

impl Tracker {
    /// Starts (or continues) tracking `scope` = (run, segment); a different scope drops all memory.
    pub fn enter(&mut self, scope: (Option<i64>, Option<i64>)) {
        if self.scope != Some(scope) {
            self.clear();
            self.scope = Some(scope);
        }
    }

    pub fn leave(&mut self) {
        self.scope = None;
        self.clear();
    }

    fn clear(&mut self) {
        self.requested.clear();
        self.reported.clear();
        self.list_failed = false;
    }

    /// True the first time a (pid, kind) pair is reported, so a dry-run or skip is logged once.
    pub fn first_report(&mut self, pid: u32, kind: &'static str) -> bool {
        self.reported.insert((pid, kind))
    }

    fn was_reported(&self, pid: u32, kind: &'static str) -> bool {
        self.reported.contains(&(pid, kind))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Dry-run: would have been ended.
    WouldTerminate(AppIdentity),
    SkipProtected(AppIdentity),
    RequestQuit(AppIdentity),
    ForceKill(AppIdentity),
    /// The grace time ran out but force quit is off: leave the app running and record it.
    ForceNotAllowed(AppIdentity),
}

pub const KIND_WOULD_TERMINATE: &str = "would_terminate";
pub const KIND_SKIPPED_PROTECTED: &str = "skipped_protected";
pub const KIND_FORCE_NOT_ALLOWED: &str = "force_not_allowed";

/// Enforcement applies only while the lock is held during a study block (not breaks).
pub fn applies(phase: Phase, lock_held: bool) -> bool {
    lock_held && phase == Phase::Study
}

/// Actions for one poll. `blocklist` holds normalized app ids. The grace clock starts when the
/// quit request is made, and a force kill needs `force_quit_allowed`, an elapsed grace time, and
/// enforce mode: dry-run never reaches this far because it never requests a quit.
pub fn decide(
    running: &[AppIdentity],
    blocklist: &[String],
    phase: Phase,
    lock_held: bool,
    cfg: &Config,
    tracker: &Tracker,
    now_ms: i64,
) -> Vec<Action> {
    if !applies(phase, lock_held) {
        return Vec::new();
    }
    let mut actions = Vec::new();
    for app in running.iter().filter(|app| blocklist.contains(&app.id)) {
        if app.is_protected {
            if !tracker.was_reported(app.pid, KIND_SKIPPED_PROTECTED) {
                actions.push(Action::SkipProtected(app.clone()));
            }
            continue;
        }
        match cfg.mode {
            EnforceMode::DryRun => {
                if !tracker.was_reported(app.pid, KIND_WOULD_TERMINATE) {
                    actions.push(Action::WouldTerminate(app.clone()));
                }
            }
            EnforceMode::Enforce => match tracker.requested.get(&app.pid) {
                None => actions.push(Action::RequestQuit(app.clone())),
                Some(request) if request.forced => {}
                Some(request) if now_ms - request.at_ms >= cfg.grace_ms => {
                    if cfg.force_quit_allowed {
                        actions.push(Action::ForceKill(app.clone()));
                    } else if !tracker.was_reported(app.pid, KIND_FORCE_NOT_ALLOWED) {
                        actions.push(Action::ForceNotAllowed(app.clone()));
                    }
                }
                Some(_) => {}
            },
        }
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, pid: u32, protected: bool) -> AppIdentity {
        AppIdentity {
            id: id.into(),
            display_name: format!("name-{pid}"),
            pid,
            is_protected: protected,
        }
    }

    fn cfg(mode: EnforceMode, force: bool) -> Config {
        Config {
            mode,
            grace_ms: 10_000,
            force_quit_allowed: force,
        }
    }

    fn list() -> Vec<String> {
        vec!["com.game".into()]
    }

    fn run(running: &[AppIdentity], c: &Config, t: &Tracker, now: i64) -> Vec<Action> {
        decide(running, &list(), Phase::Study, true, c, t, now)
    }

    #[test]
    fn only_blocked_apps_are_considered() {
        let running = [app("com.game", 1, false), app("com.editor", 2, false)];
        let actions = run(
            &running,
            &cfg(EnforceMode::Enforce, false),
            &Tracker::default(),
            0,
        );
        assert_eq!(
            actions,
            vec![Action::RequestQuit(app("com.game", 1, false))]
        );
    }

    #[test]
    fn dry_run_only_reports_and_reports_each_pid_once() {
        let running = [app("com.game", 1, false)];
        let mut tracker = Tracker::default();
        let c = cfg(EnforceMode::DryRun, true);
        assert_eq!(
            run(&running, &c, &tracker, 0),
            vec![Action::WouldTerminate(running[0].clone())]
        );
        assert!(tracker.first_report(1, KIND_WOULD_TERMINATE));
        assert!(run(&running, &c, &tracker, 1).is_empty());
    }

    #[test]
    fn nothing_applies_outside_a_locked_study_block() {
        let running = [app("com.game", 1, false)];
        let c = cfg(EnforceMode::Enforce, true);
        let t = Tracker::default();
        for phase in [Phase::Idle, Phase::Break, Phase::LongBreak] {
            assert!(
                decide(&running, &list(), phase, true, &c, &t, 0).is_empty(),
                "{phase:?}"
            );
        }
        assert!(decide(&running, &list(), Phase::Study, false, &c, &t, 0).is_empty());
        assert!(applies(Phase::Study, true));
    }

    #[test]
    fn protected_apps_are_skipped_even_when_listed() {
        let running = [app("com.game", 1, true)];
        let actions = run(
            &running,
            &cfg(EnforceMode::Enforce, true),
            &Tracker::default(),
            0,
        );
        assert_eq!(actions, vec![Action::SkipProtected(running[0].clone())]);
    }

    #[test]
    fn force_kill_waits_for_the_grace_time_and_needs_permission() {
        let running = [app("com.game", 1, false)];
        let mut tracker = Tracker::default();
        tracker.requested.insert(
            1,
            Requested {
                app: running[0].clone(),
                at_ms: 1_000,
                forced: false,
            },
        );
        assert!(run(&running, &cfg(EnforceMode::Enforce, true), &tracker, 10_999).is_empty());
        assert_eq!(
            run(&running, &cfg(EnforceMode::Enforce, true), &tracker, 11_000),
            vec![Action::ForceKill(running[0].clone())]
        );
        assert_eq!(
            run(
                &running,
                &cfg(EnforceMode::Enforce, false),
                &tracker,
                11_000
            ),
            vec![Action::ForceNotAllowed(running[0].clone())]
        );
        assert!(tracker.first_report(1, KIND_FORCE_NOT_ALLOWED));
        assert!(
            run(
                &running,
                &cfg(EnforceMode::Enforce, false),
                &tracker,
                99_000
            )
            .is_empty()
        );
    }

    #[test]
    fn a_forced_pid_is_left_alone() {
        let running = [app("com.game", 1, false)];
        let mut tracker = Tracker::default();
        tracker.requested.insert(
            1,
            Requested {
                app: running[0].clone(),
                at_ms: 0,
                forced: true,
            },
        );
        assert!(run(&running, &cfg(EnforceMode::Enforce, true), &tracker, 99_000).is_empty());
    }

    #[test]
    fn a_new_segment_forgets_everything() {
        let mut tracker = Tracker::default();
        tracker.enter((Some(1), Some(1)));
        tracker.requested.insert(
            1,
            Requested {
                app: app("com.game", 1, false),
                at_ms: 0,
                forced: false,
            },
        );
        assert!(tracker.first_report(1, KIND_WOULD_TERMINATE));
        tracker.enter((Some(1), Some(1)));
        assert_eq!(tracker.requested.len(), 1);
        tracker.enter((Some(1), Some(3)));
        assert!(tracker.requested.is_empty());
        assert!(tracker.first_report(1, KIND_WOULD_TERMINATE));
    }
}
