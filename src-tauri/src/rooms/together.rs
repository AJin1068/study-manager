//! Same-time study: the room owner shares upcoming blocks; a member previews them and copies the
//! ones they choose into their own plan.
//!
//! Sharing sends only start time, plan name, subject and set count of the owner's next blocks
//! (the same fields the plan mirror already stores). Importing is never automatic: the member picks
//! blocks, each is created through the ordinary plan API (lock check, overlap check, `plan_history`
//! row), and the room's schedule is re-read from the server at that moment, so the screen cannot
//! hand the core a block the owner did not share.

use chrono::{Duration, TimeZone};
use rusqlite::Connection;
use serde::Serialize;

use super::model::WireSchedule;
use super::view::schedule_key;
use crate::error::{AppError, AppResult};
use crate::plan::{self, CreatePlanInput};
use crate::remote::payload::ScheduleItemRow;
use crate::store;

/// The owner shares at most this many upcoming blocks, within this many days.
pub const MAX_SHARED_BLOCKS: usize = 20;
pub const SHARE_HORIZON_DAYS: i64 = 7;

/// The owner's upcoming blocks, earliest first, as the server accepts them.
pub fn shareable_items<Tz: TimeZone>(
    conn: &Connection,
    tz: &Tz,
    now_ms: i64,
) -> AppResult<Vec<ScheduleItemRow>> {
    let today = tz
        .timestamp_millis_opt(now_ms)
        .single()
        .ok_or_else(|| AppError::Internal("timestamp has no local date".into()))?
        .date_naive();
    let from = today.format("%Y-%m-%d").to_string();
    let to = (today + Duration::days(SHARE_HORIZON_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    let mut upcoming: Vec<plan::Occurrence> = plan::list_occurrences(conn, tz, &from, &to)?
        .into_iter()
        .filter(|o| o.planned_start_ms >= now_ms)
        .collect();
    upcoming.sort_by_key(|o| (o.planned_start_ms, o.plan_id));
    Ok(upcoming
        .into_iter()
        .take(MAX_SHARED_BLOCKS)
        .map(|o| ScheduleItemRow {
            start_at_utc_ms: o.planned_start_ms,
            name: o.name.chars().take(60).collect(),
            subject: o.subject.chars().take(40).collect(),
            sets: o.sets,
        })
        .collect())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportOutcome {
    Applied,
    AlreadyImported,
    /// The block's start has passed.
    InPast,
    /// The owner no longer shares this block.
    NotShared,
    /// A study run is in progress, so plans cannot change.
    RejectedLocked,
    /// The plan API refused the block (it overlaps another plan, or a field is out of range).
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub key: String,
    pub outcome: ImportOutcome,
    pub plan_id: Option<i64>,
}

/// Copies the chosen blocks (by key) into the user's plan. One block failing does not stop the
/// others; each result says what happened.
pub fn import_items<Tz: TimeZone>(
    conn: &mut Connection,
    tz: &Tz,
    room_id: &str,
    schedule: &WireSchedule,
    keys: &[String],
    now_ms: i64,
) -> AppResult<Vec<ImportResult>> {
    let already = store::rooms::imported_keys(conn, room_id)?;
    let mut results = Vec::with_capacity(keys.len());
    for key in keys {
        let result = |outcome, plan_id| ImportResult {
            key: key.clone(),
            outcome,
            plan_id,
        };
        let Some(item) = schedule
            .items
            .iter()
            .find(|i| schedule_key(i.start_at_utc_ms) == *key)
        else {
            results.push(result(ImportOutcome::NotShared, None));
            continue;
        };
        if already.contains(key) {
            results.push(result(ImportOutcome::AlreadyImported, None));
            continue;
        }
        if item.start_at_utc_ms < now_ms {
            results.push(result(ImportOutcome::InPast, None));
            continue;
        }
        let Some(local) = tz.timestamp_millis_opt(item.start_at_utc_ms).single() else {
            results.push(result(ImportOutcome::Invalid, None));
            continue;
        };
        let input = CreatePlanInput {
            name: item.name.clone(),
            subject: item.subject.clone(),
            start_date: local.date_naive().format("%Y-%m-%d").to_string(),
            start_time: local.time().format("%H:%M").to_string(),
            sets: item.sets,
            repeat: None,
        };
        match plan::create_plan(conn, tz, input, now_ms) {
            Ok(change) if change.is_applied() => {
                let plan_id = change.plan_id.unwrap_or_default();
                store::rooms::record_import(conn, room_id, key, plan_id, now_ms)?;
                results.push(result(ImportOutcome::Applied, change.plan_id));
            }
            Ok(_) => results.push(result(ImportOutcome::RejectedLocked, None)),
            Err(AppError::Invalid(_)) => results.push(result(ImportOutcome::Invalid, None)),
            Err(other) => return Err(other),
        }
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;
    use crate::db::open_in_memory;
    use crate::rooms::model::WireScheduleItem;
    use crate::rooms::testkit::T0;
    use crate::testutil::set_lock_for_test;

    fn seoul() -> FixedOffset {
        FixedOffset::east_opt(9 * 3600).expect("offset")
    }

    fn item(start: i64, name: &str) -> WireScheduleItem {
        WireScheduleItem {
            start_at_utc_ms: start,
            name: name.into(),
            subject: "수학".into(),
            sets: 2,
        }
    }

    fn schedule(items: Vec<WireScheduleItem>) -> WireSchedule {
        WireSchedule {
            items,
            updated_at_ms: 1,
        }
    }

    // 2026-11-07 10:00 in Seoul = 01:00 UTC. A block starting after T0 (2026-11-06T14:30Z).
    const NEXT_DAY_10_KST: i64 = 1_794_013_200_000;

    fn plan_input(day: &str, time: &str) -> CreatePlanInput {
        CreatePlanInput {
            name: "내 계획".into(),
            subject: "영어".into(),
            start_date: day.into(),
            start_time: time.into(),
            sets: 2,
            repeat: None,
        }
    }

    #[test]
    fn the_owner_shares_only_upcoming_blocks_in_order_with_nothing_but_the_four_fields() {
        let mut conn = open_in_memory().expect("db");
        plan::create_plan(&mut conn, &seoul(), plan_input("2026-11-08", "09:00"), T0)
            .expect("later");
        plan::create_plan(&mut conn, &seoul(), plan_input("2026-11-07", "20:00"), T0)
            .expect("sooner");
        plan::create_plan(&mut conn, &seoul(), plan_input("2026-11-30", "09:00"), T0)
            .expect("beyond the week");
        let items = shareable_items(&conn, &seoul(), T0).expect("items");
        assert_eq!(items.len(), 2);
        assert!(items[0].start_at_utc_ms < items[1].start_at_utc_ms);
        let json = serde_json::to_value(&items[0]).expect("json");
        let mut keys: Vec<&str> = json
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["name", "sets", "start_at_utc_ms", "subject"]);
    }

    #[test]
    fn a_chosen_block_becomes_a_plan_in_local_time_once_and_leaves_a_history_row() {
        let mut conn = open_in_memory().expect("db");
        let key = schedule_key(NEXT_DAY_10_KST);
        let shared = schedule(vec![item(NEXT_DAY_10_KST, "수학 집중")]);

        let first = import_items(
            &mut conn,
            &seoul(),
            "room-aaaa-1",
            &shared,
            std::slice::from_ref(&key),
            T0,
        )
        .expect("import");
        assert_eq!(first[0].outcome, ImportOutcome::Applied);
        let plans =
            plan::list_occurrences(&conn, &seoul(), "2026-11-07", "2026-11-07").expect("list");
        assert_eq!(
            (plans[0].name.as_str(), plans[0].start_time.as_str()),
            ("수학 집중", "10:00")
        );
        let history = plan::plan_history(&conn, first[0].plan_id, 10).expect("history");
        assert_eq!(
            history[0].action, "create",
            "the ordinary plan history records the copy"
        );

        let second = import_items(&mut conn, &seoul(), "room-aaaa-1", &shared, &[key], T0 + 1)
            .expect("import");
        assert_eq!(second[0].outcome, ImportOutcome::AlreadyImported);
        assert_eq!(
            plan::list_occurrences(&conn, &seoul(), "2026-11-07", "2026-11-07")
                .expect("list")
                .len(),
            1
        );
    }

    #[test]
    fn a_block_the_owner_did_not_share_or_that_already_started_is_not_imported() {
        let mut conn = open_in_memory().expect("db");
        let shared = schedule(vec![item(T0 - 60_000, "지난 블록")]);
        let results = import_items(
            &mut conn,
            &seoul(),
            "room-aaaa-1",
            &shared,
            &[schedule_key(T0 - 60_000), "999".to_string()],
            T0,
        )
        .expect("import");
        assert_eq!(
            results.iter().map(|r| r.outcome).collect::<Vec<_>>(),
            [ImportOutcome::InPast, ImportOutcome::NotShared]
        );
    }

    #[test]
    fn nothing_changes_while_a_study_run_holds_the_lock_or_when_the_block_overlaps_a_plan() {
        let mut conn = open_in_memory().expect("db");
        let shared = schedule(vec![item(NEXT_DAY_10_KST, "A")]);
        let key = schedule_key(NEXT_DAY_10_KST);

        set_lock_for_test(&conn, true);
        let locked = import_items(
            &mut conn,
            &seoul(),
            "room-aaaa-1",
            &shared,
            std::slice::from_ref(&key),
            T0,
        )
        .expect("import");
        assert_eq!(locked[0].outcome, ImportOutcome::RejectedLocked);
        set_lock_for_test(&conn, false);
        assert!(
            plan::list_occurrences(&conn, &seoul(), "2026-11-07", "2026-11-07")
                .expect("list")
                .is_empty()
        );
        assert!(
            store::rooms::imported_keys(&conn, "room-aaaa-1")
                .expect("keys")
                .is_empty(),
            "a refused copy is not marked as done"
        );

        plan::create_plan(&mut conn, &seoul(), plan_input("2026-11-07", "10:00"), T0)
            .expect("existing plan");
        let overlap =
            import_items(&mut conn, &seoul(), "room-aaaa-1", &shared, &[key], T0).expect("import");
        assert_eq!(overlap[0].outcome, ImportOutcome::Invalid);
    }
}
