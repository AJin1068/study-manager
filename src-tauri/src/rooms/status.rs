//! Publishes this user's own status to the server: friend settings first, then the state report,
//! the daily minutes, a content-free distraction notice, and the worn character.
//!
//! Only this user's data is ever sent, and only what `friends.*` allows (`prefs::status_report` /
//! `daily_reports` build the payloads). Nothing is sent when signed out, not configured, or not in
//! any room. A failed call backs off 5, 10, 20, 40, then 60 seconds; the local app is unaffected.
//!
//! The engine is a plain struct driven by `tick(now_ms, source)`, so timing is tested with a
//! fake clock; `service` runs it on a thread.

use std::collections::HashMap;
use std::sync::Arc;

use super::model::{RoomCode, RoomError};
use super::port::RoomApi;
use super::prefs::{FriendSettings, LocalSnapshot, daily_reports, status_report};
use crate::remote::payload::{FriendPrefsArgs, InventoryPublicArgs};

/// A changed status goes out at the next tick; an unchanged one is repeated this often so the
/// server's "last seen" stays well inside the 90-second offline rule.
pub const KEEPALIVE_MS: i64 = 30_000;
/// A day's minutes are re-sent at most this often.
pub const DAILY_GAP_MS: i64 = 60_000;
pub const TICK_MS: i64 = 5_000;
const BACKOFF_CAP_MS: i64 = 60_000;

/// Where the engine reads its inputs from.
pub trait PublishSource: Send + Sync {
    /// The server handle; `None` when signed out or not configured.
    fn api(&self) -> Option<Arc<dyn RoomApi>>;
    fn settings(&self) -> Option<FriendSettings>;
    fn in_a_room(&self) -> bool;
    fn local(&self, now_ms: i64) -> Option<LocalSnapshot>;
    fn inventory(&self) -> Option<InventoryPublicArgs>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum PublishOutcome {
    /// Signed out, not configured, or not in a room: nothing was sent.
    Inactive,
    /// Not due, or nothing changed.
    Idle,
    Sent {
        calls: u32,
    },
    Failed(&'static str),
}

#[derive(Default)]
pub struct Publisher {
    next_due_ms: i64,
    failures: u32,
    pushed_prefs: Option<FriendPrefsArgs>,
    last_status: Option<(String, i64)>,
    last_daily: HashMap<String, (i64, i64)>,
    last_attempts: Option<i64>,
    last_inventory: Option<InventoryPublicArgs>,
}

fn status_key(report: &crate::remote::payload::StatusReportArgs) -> String {
    format!(
        "{}|{}|{:?}",
        report.state, report.level, report.attempts_today
    )
}

impl Publisher {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub fn consecutive_failures(&self) -> u32 {
        self.failures
    }

    fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn tick(&mut self, now_ms: i64, source: &dyn PublishSource) -> PublishOutcome {
        if now_ms < self.next_due_ms {
            return PublishOutcome::Idle;
        }
        let Some(api) = source.api() else {
            self.reset();
            return PublishOutcome::Inactive;
        };
        let Some(settings) = source.settings() else {
            return PublishOutcome::Idle;
        };
        let in_room = source.in_a_room();
        // A settings change must still reach the server after the last room is left.
        if !in_room && self.pushed_prefs.is_none() {
            return PublishOutcome::Inactive;
        }
        match self.publish(now_ms, source, api.as_ref(), &settings, in_room) {
            Ok(calls) => {
                self.failures = 0;
                self.next_due_ms = now_ms + TICK_MS;
                if calls == 0 {
                    PublishOutcome::Idle
                } else {
                    PublishOutcome::Sent { calls }
                }
            }
            Err(err) => {
                self.failures = self.failures.saturating_add(1);
                let backoff =
                    TICK_MS.saturating_mul(1_i64 << self.failures.saturating_sub(1).min(4));
                self.next_due_ms = now_ms + backoff.min(BACKOFF_CAP_MS);
                PublishOutcome::Failed(err.code())
            }
        }
    }

    fn publish(
        &mut self,
        now_ms: i64,
        source: &dyn PublishSource,
        api: &dyn RoomApi,
        settings: &FriendSettings,
        in_room: bool,
    ) -> Result<u32, RoomError> {
        let mut calls = 0;
        // 1. Settings first: a narrowing must reach the server before anything else is sent.
        let args = settings.as_args();
        let prefs_changed = self.pushed_prefs != Some(args);
        if prefs_changed {
            api.set_prefs(&args)?;
            self.pushed_prefs = Some(args);
            self.last_status = None;
            self.last_daily.clear();
            calls += 1;
        }
        if !settings.share_level {
            self.last_inventory = None;
        }
        if !in_room {
            return Ok(calls);
        }
        let Some(local) = source.local(now_ms) else {
            return Ok(calls);
        };

        // 2. State report: on change, or as a keep-alive.
        if let Some(report) = status_report(settings, &local) {
            let key = status_key(&report);
            let due = match &self.last_status {
                Some((last_key, at)) => *last_key != key || now_ms - at >= KEEPALIVE_MS,
                None => true,
            };
            if due {
                api.report_status(&report)?;
                self.last_status = Some((key, now_ms));
                calls += 1;
            }
        }

        // 3. Daily minutes, only when shared. A refusal for sharing or a jump is not retried until
        //    the value changes again.
        for report in daily_reports(settings, &local) {
            let sent = self.last_daily.get(&report.day).copied();
            let due = match sent {
                Some((minutes, at)) => minutes != report.study_min && now_ms - at >= DAILY_GAP_MS,
                None => true,
            };
            if !due {
                continue;
            }
            match api.report_daily(&report) {
                Ok(())
                | Err(RoomError::Rejected(RoomCode::NotShared | RoomCode::SurgeRejected)) => {
                    self.last_daily
                        .insert(report.day.clone(), (report.study_min, now_ms));
                    calls += 1;
                }
                Err(err) => return Err(err),
            }
        }

        // 4. Distraction notice: one content-free call per new attempt, only if published.
        if let Some(previous) = self.last_attempts {
            if settings.distraction_publish && local.attempts_today > previous {
                api.report_distraction()?;
                calls += 1;
            }
        }
        self.last_attempts = Some(local.attempts_today);

        // 5. Worn character, when the level/character scope is shared.
        if settings.share_level {
            if let Some(inventory) = source.inventory() {
                if self.last_inventory.as_ref() != Some(&inventory) {
                    api.set_inventory_public(&inventory)?;
                    self.last_inventory = Some(inventory);
                    calls += 1;
                }
            }
        }
        Ok(calls)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::rooms::port::RoomService;
    use crate::rooms::prefs::{LocalDay, LocalPhase};
    use crate::rooms::testkit::{Person, T0, World};

    struct Source {
        api: Option<Arc<dyn RoomApi>>,
        settings: Mutex<FriendSettings>,
        local: Mutex<LocalSnapshot>,
        in_room: bool,
        inventory: Option<InventoryPublicArgs>,
    }

    impl PublishSource for Source {
        fn api(&self) -> Option<Arc<dyn RoomApi>> {
            self.api.clone()
        }
        fn settings(&self) -> Option<FriendSettings> {
            self.settings.lock().ok().map(|s| *s)
        }
        fn in_a_room(&self) -> bool {
            self.in_room
        }
        fn local(&self, _now_ms: i64) -> Option<LocalSnapshot> {
            self.local.lock().ok().map(|l| l.clone())
        }
        fn inventory(&self) -> Option<InventoryPublicArgs> {
            self.inventory.clone()
        }
    }

    fn defaults() -> FriendSettings {
        FriendSettings {
            share_level: true,
            share_status: true,
            share_week_time: false,
            share_attempts: false,
            distraction_publish: false,
            distraction_receive: false,
        }
    }

    fn snapshot(attempts: i64, study_min: i64) -> LocalSnapshot {
        LocalSnapshot {
            phase: LocalPhase::Studying,
            remaining_sec: 1_800,
            level: 4,
            attempts_today: attempts,
            days: vec![LocalDay {
                day: "2026-11-06".into(),
                study_min,
                plan_min: 90,
            }],
        }
    }

    fn source(person: &Person, settings: FriendSettings, in_room: bool) -> Source {
        let api: Arc<dyn RoomApi> = person.rooms.clone();
        Source {
            api: Some(api),
            settings: Mutex::new(settings),
            local: Mutex::new(snapshot(0, 10)),
            in_room,
            inventory: None,
        }
    }

    #[test]
    fn nothing_is_sent_while_signed_out_or_outside_every_room() {
        let world = World::new();
        let alice = world.person("a@example.test");
        let mut publisher = Publisher::new();

        let signed_out = Source {
            api: None,
            ..source(&alice, defaults(), true)
        };
        assert_eq!(publisher.tick(T0, &signed_out), PublishOutcome::Inactive);

        let no_room = source(&alice, defaults(), false);
        assert_eq!(publisher.tick(T0, &no_room), PublishOutcome::Inactive);
        assert!(
            world
                .recorder
                .bodies()
                .iter()
                .all(|(name, _)| name == "token" || name == "verify" || name == "otp"),
            "{:?}",
            world.recorder.bodies()
        );
    }

    #[test]
    fn the_first_tick_sends_settings_then_status_and_an_unchanged_status_waits_for_the_keepalive() {
        let world = World::new();
        let alice = world.person("a@example.test");
        alice.rooms.create_room("room", "앨리스").expect("room");
        let src = source(&alice, defaults(), true);
        let mut publisher = Publisher::new();

        assert_eq!(publisher.tick(T0, &src), PublishOutcome::Sent { calls: 2 });
        let order: Vec<String> = world
            .recorder
            .bodies()
            .into_iter()
            .map(|(name, _)| name)
            .filter(|n| n.contains('_'))
            .collect();
        assert_eq!(
            order[order.len() - 2..],
            ["set_friend_prefs", "report_status"],
            "settings go before the status"
        );

        assert_eq!(
            publisher.tick(T0 + 1_000, &src),
            PublishOutcome::Idle,
            "not due yet"
        );
        assert_eq!(
            publisher.tick(T0 + 5_000, &src),
            PublishOutcome::Idle,
            "nothing changed"
        );
        assert_eq!(
            publisher.tick(T0 + 30_000, &src),
            PublishOutcome::Sent { calls: 1 },
            "keep-alive"
        );

        src.local.lock().expect("lock").phase = LocalPhase::Break;
        assert_eq!(
            publisher.tick(T0 + 35_000, &src),
            PublishOutcome::Sent { calls: 1 },
            "a state change goes at the next tick"
        );
    }

    #[test]
    fn what_is_private_never_reaches_the_server_and_switching_a_share_on_adds_it() {
        let world = World::new();
        let alice = world.person("a@example.test");
        alice.rooms.create_room("room", "앨리스").expect("room");
        let src = source(&alice, defaults(), true);
        *src.local.lock().expect("lock") = snapshot(6, 75);
        let mut publisher = Publisher::new();
        publisher.tick(T0, &src);
        assert!(
            world.recorder.bodies_of("report_daily").is_empty(),
            "study minutes are private by default"
        );
        assert!(
            world
                .recorder
                .bodies_of("report_status")
                .iter()
                .all(|b| b.contains("\"p_attempts_today\":null"))
        );

        *src.settings.lock().expect("lock") = FriendSettings {
            share_week_time: true,
            share_attempts: true,
            ..defaults()
        };
        assert!(matches!(
            publisher.tick(T0 + 5_000, &src),
            PublishOutcome::Sent { .. }
        ));
        assert_eq!(world.recorder.bodies_of("report_daily").len(), 1);
        assert!(
            world
                .recorder
                .bodies_of("report_status")
                .last()
                .is_some_and(|b| b.contains("\"p_attempts_today\":6"))
        );
    }

    #[test]
    fn a_distraction_notice_is_one_call_per_new_attempt_and_only_when_published() {
        let world = World::new();
        let alice = world.person("a@example.test");
        alice.rooms.create_room("room", "앨리스").expect("room");
        let src = source(
            &alice,
            FriendSettings {
                distraction_publish: true,
                ..defaults()
            },
            true,
        );
        let mut publisher = Publisher::new();
        publisher.tick(T0, &src);
        assert!(
            world.recorder.bodies_of("report_distraction").is_empty(),
            "the first reading is only a baseline"
        );

        src.local.lock().expect("lock").attempts_today = 1;
        publisher.tick(T0 + 5_000, &src);
        assert_eq!(
            world.recorder.bodies_of("report_distraction"),
            ["{}"],
            "the body is empty: no app, no count"
        );
        publisher.tick(T0 + 10_000, &src);
        assert_eq!(
            world.recorder.bodies_of("report_distraction").len(),
            1,
            "no new attempt, no new notice"
        );

        *src.settings.lock().expect("lock") = defaults();
        src.local.lock().expect("lock").attempts_today = 2;
        publisher.tick(T0 + 15_000, &src);
        assert_eq!(
            world.recorder.bodies_of("report_distraction").len(),
            1,
            "publishing is off"
        );
    }

    #[test]
    fn a_failure_backs_off_5_10_20_40_60_seconds_and_recovers() {
        let world = World::new();
        let alice = world.person("a@example.test");
        alice.rooms.create_room("room", "앨리스").expect("room");
        let src = source(&alice, defaults(), true);
        let mut publisher = Publisher::new();
        world.backend.set_outage(true);

        let mut at = T0;
        let mut failed_at = Vec::new();
        while failed_at.len() < 6 {
            if matches!(publisher.tick(at, &src), PublishOutcome::Failed(_)) {
                failed_at.push(at);
            }
            at += 1_000;
        }
        let waits: Vec<i64> = failed_at.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(waits, [5_000, 10_000, 20_000, 40_000, 60_000]);
        let at = failed_at[5];
        world.backend.set_outage(false);
        assert!(matches!(
            publisher.tick(at + 60_000, &src),
            PublishOutcome::Sent { .. }
        ));
        assert_eq!(publisher.consecutive_failures(), 0);
    }

    #[test]
    fn turning_a_share_off_reaches_the_server_even_after_the_last_room_was_left() {
        let world = World::new();
        let alice = world.person("a@example.test");
        let room = alice.rooms.create_room("room", "앨리스").expect("room");
        let mut src = source(
            &alice,
            FriendSettings {
                share_week_time: true,
                ..defaults()
            },
            true,
        );
        let mut publisher = Publisher::new();
        publisher.tick(T0, &src);
        assert_eq!(world.recorder.bodies_of("report_daily").len(), 1);

        alice.rooms.leave_room(&room).expect("leave");
        src.in_room = false;
        *src.settings.lock().expect("lock") = defaults();
        assert!(matches!(
            publisher.tick(T0 + 5_000, &src),
            PublishOutcome::Sent { calls: 1 }
        ));
        assert!(
            world
                .recorder
                .bodies_of("set_friend_prefs")
                .last()
                .is_some_and(|b| b.contains("\"p_share_week_time\":false"))
        );
    }

    #[test]
    fn the_worn_character_is_sent_once_and_only_while_level_sharing_is_on() {
        let world = World::new();
        let alice = world.person("a@example.test");
        alice.rooms.create_room("room", "앨리스").expect("room");
        let mut src = source(&alice, defaults(), true);
        src.inventory = Some(InventoryPublicArgs {
            species: "fox".into(),
            hat: Some("hat_beanie".into()),
            top: None,
            accessory: None,
            hand: None,
        });
        let mut publisher = Publisher::new();
        publisher.tick(T0, &src);
        publisher.tick(T0 + 5_000, &src);
        assert_eq!(world.recorder.bodies_of("set_inventory_public").len(), 1);

        *src.settings.lock().expect("lock") = FriendSettings {
            share_level: false,
            ..defaults()
        };
        publisher.tick(T0 + 10_000, &src);
        *src.settings.lock().expect("lock") = defaults();
        publisher.tick(T0 + 15_000, &src);
        assert_eq!(
            world.recorder.bodies_of("set_inventory_public").len(),
            2,
            "re-sent after being switched back on"
        );
    }
}
