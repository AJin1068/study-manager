//! Today's topic and next-topic assignment. The panel reads only local rows and files. An
//! assignment is metadata about an occurrence: the plan (name, subject) never changes and
//! `plan_history` is untouched; `topic_assignment` keeps its own insert-only history. A manual
//! choice stays; a later automatic run neither replaces it nor hands its page to another slot.

use std::collections::HashSet;

use chrono::{Duration, NaiveDate, TimeZone, Timelike};
use rusqlite::Connection;
use serde::Serialize;

use super::recommend::{self, PageStat, Reason, Slot};
use crate::error::{AppError, AppResult};
use crate::plan::{self, Occurrence};
use crate::store;
use crate::store::topic::{self as repo, AssignmentRow};
use crate::store::wiki as wiki_repo;
use crate::wiki::service::{subject_for_plan, WikiService};

/// Days ahead (today included) that get a topic.
pub const ASSIGN_DAYS: i64 = 3;

/// Why there is nothing to show or assign. The UI turns the code into guidance.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TopicNotice {
    /// No plan today (or none left to assign).
    NoPlan,
    /// The plan's subject has no wiki.
    NoWiki,
    /// The wiki exists but has no pages.
    NoPages,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TopicSource {
    Assigned,
    NameMatch,
    Weak,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TopicPage {
    pub page_id: i64,
    pub page_name: String,
    pub markdown: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TodayTopic {
    pub plan_id: Option<i64>,
    pub plan_name: Option<String>,
    pub date: String,
    pub source: Option<TopicSource>,
    /// `auto` or `manual` when the page was assigned.
    pub assigned_by: Option<String>,
    pub reason: Option<Reason>,
    pub page: Option<TopicPage>,
    pub notice: Option<TopicNotice>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssignmentView {
    pub plan_id: i64,
    pub plan_name: String,
    pub date: String,
    pub page_id: i64,
    pub page_name: String,
    pub source: String,
    pub reason: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssignReport {
    pub assigned: Vec<AssignmentView>,
    /// Occurrences that kept the page the user chose.
    pub kept_manual: usize,
    pub notice: Option<TopicNotice>,
}

fn date_of<Tz: TimeZone>(tz: &Tz, ms: i64) -> AppResult<NaiveDate> {
    tz.timestamp_millis_opt(ms).single().map(|t| t.date_naive()).ok_or_else(|| AppError::Internal("time out of range".into()))
}

fn ends_at(o: &Occurrence) -> i64 {
    o.planned_start_ms.saturating_add(o.duration_min.saturating_mul(60_000))
}

/// The occurrence the panel is about: the running one, else the first of today not yet over.
fn pick_today<Tz: TimeZone>(conn: &Connection, tz: &Tz, now_ms: i64, today: &[Occurrence]) -> AppResult<Option<Occurrence>> {
    let state = crate::timer::current_state(conn, tz, now_ms)?;
    if let Some(run) = state.run_id.map(|id| store::get_run(conn, id)).transpose()?.flatten() {
        if let Some(found) = today.iter().find(|o| o.plan_id == run.plan_id) {
            return Ok(Some(found.clone()));
        }
    }
    let mut left: Vec<&Occurrence> = today.iter().filter(|o| ends_at(o) > now_ms).collect();
    left.sort_by_key(|o| (o.planned_start_ms, o.plan_id));
    Ok(left.first().map(|o| (*o).clone()))
}

fn name_matches(page: &str, plan: &str) -> bool {
    let (page, plan) = (page.trim().to_lowercase(), plan.trim().to_lowercase());
    !page.is_empty() && !plan.is_empty() && (plan.contains(&page) || page.contains(&plan))
}

fn empty_topic(plan: Option<&Occurrence>, date: String, notice: TopicNotice) -> TodayTopic {
    TodayTopic {
        plan_id: plan.map(|o| o.plan_id),
        plan_name: plan.map(|o| o.name.clone()),
        date,
        source: None,
        assigned_by: None,
        reason: None,
        page: None,
        notice: Some(notice),
    }
}

/// The topic for the plan being studied (or next today): its assigned page, else the page named
/// like the plan, else the weakest page of the plan's subject.
pub fn today_topic<Tz: TimeZone>(conn: &Connection, tz: &Tz, now_ms: i64) -> AppResult<TodayTopic> {
    let today = date_of(tz, now_ms)?;
    let date = today.format("%Y-%m-%d").to_string();
    let (occurrences, _) = plan::occurrences_in(conn, tz, today, today)?;
    let Some(occ) = pick_today(conn, tz, now_ms, &occurrences)? else {
        return Ok(empty_topic(None, date, TopicNotice::NoPlan));
    };
    let Some(subject) = subject_for_plan(conn, &occ.subject)? else {
        return Ok(empty_topic(Some(&occ), date, TopicNotice::NoWiki));
    };
    let stats = recommend::page_stats(conn, subject.id)?;
    if stats.is_empty() {
        return Ok(empty_topic(Some(&occ), date, TopicNotice::NoPages));
    }
    let assigned = repo::current(conn, occ.plan_id, &occ.date)?.filter(|a| stats.iter().any(|p| p.page_id == a.page_id));
    let (page_id, source, assigned_by, reason) = if let Some(a) = assigned {
        (a.page_id, TopicSource::Assigned, Some(a.source), Reason::parse(&a.reason))
    } else if let Some(p) = stats.iter().find(|p| name_matches(&p.page_name, &occ.name)) {
        (p.page_id, TopicSource::NameMatch, None, None)
    } else {
        let first = recommend::rank(&stats, now_ms).into_iter().next();
        (first.as_ref().map_or(stats[0].page_id, |p| p.page_id), TopicSource::Weak, None, first.map(|p| p.reason))
    };
    let page_name = stats.iter().find(|p| p.page_id == page_id).map(|p| p.page_name.clone()).unwrap_or_default();
    let fs = WikiService::wiki_fs(conn)?;
    let markdown = fs.read_page(&subject.slug, &page_name).ok().flatten().unwrap_or_default();
    Ok(TodayTopic {
        plan_id: Some(occ.plan_id),
        plan_name: Some(occ.name),
        date,
        source: Some(source),
        assigned_by,
        reason,
        page: Some(TopicPage { page_id, page_name, markdown }),
        notice: None,
    })
}

fn view_of(conn: &Connection, row: AssignmentRow, pages: &[PageStat]) -> AppResult<AssignmentView> {
    let plan_name = store::get_plan(conn, row.plan_id)?.map(|p| p.name).unwrap_or_default();
    let page_name = pages.iter().find(|p| p.page_id == row.page_id).map(|p| p.page_name.clone()).unwrap_or_default();
    Ok(AssignmentView { plan_id: row.plan_id, plan_name, date: row.date, page_id: row.page_id, page_name, source: row.source, reason: row.reason })
}

/// Gives the coming occurrences a page each (today through [`ASSIGN_DAYS`]), keeping manual
/// choices. Uses focus data when there is any; with none, the priority order alone.
pub fn assign_next<Tz: TimeZone>(conn: &Connection, tz: &Tz, now_ms: i64) -> AppResult<AssignReport> {
    let today = date_of(tz, now_ms)?;
    let (all, _) = plan::occurrences_in(conn, tz, today, today + Duration::days(ASSIGN_DAYS - 1))?;
    let mut upcoming: Vec<Occurrence> = all.into_iter().filter(|o| ends_at(o) > now_ms).collect();
    upcoming.sort_by_key(|o| (o.planned_start_ms, o.plan_id));
    if upcoming.is_empty() {
        return Ok(AssignReport { assigned: Vec::new(), kept_manual: 0, notice: Some(TopicNotice::NoPlan) });
    }
    let focus = recommend::focus_hours(conn, tz, now_ms)?;

    let mut report = AssignReport { assigned: Vec::new(), kept_manual: 0, notice: None };
    let (mut without_wiki, mut without_pages) = (0, 0);
    let mut done_subjects: HashSet<i64> = HashSet::new();
    for first in &upcoming {
        let Some(subject) = subject_for_plan(conn, &first.subject)? else {
            without_wiki += 1;
            continue;
        };
        if !done_subjects.insert(subject.id) {
            continue;
        }
        let group: Vec<&Occurrence> = upcoming
            .iter()
            .filter(|o| subject_for_plan(conn, &o.subject).ok().flatten().is_some_and(|s| s.id == subject.id))
            .collect();
        let stats = recommend::page_stats(conn, subject.id)?;
        if stats.is_empty() {
            without_pages += 1;
            continue;
        }
        let mut taken: HashSet<i64> = HashSet::new();
        let mut open: Vec<&Occurrence> = Vec::new();
        for occ in &group {
            match repo::current(conn, occ.plan_id, &occ.date)? {
                Some(a) if a.source == "manual" && stats.iter().any(|p| p.page_id == a.page_id) => {
                    taken.insert(a.page_id);
                    report.kept_manual += 1;
                    report.assigned.push(view_of(conn, a, &stats)?);
                }
                _ => open.push(occ),
            }
        }
        let free: Vec<PageStat> = stats.iter().filter(|p| !taken.contains(&p.page_id)).cloned().collect();
        let slots: Vec<Slot> = open
            .iter()
            .map(|o| Slot {
                plan_id: o.plan_id,
                date: o.date.clone(),
                hour: tz.timestamp_millis_opt(o.planned_start_ms).single().map_or(0, |t| t.hour()),
            })
            .collect();
        for a in recommend::recommend_next(&free, &focus, &slots, now_ms) {
            let row = AssignmentRow {
                plan_id: a.plan_id,
                date: a.date,
                page_id: a.page_id,
                source: "auto".into(),
                reason: a.reason.as_str().into(),
                assigned_at_ms: now_ms,
            };
            let same = repo::current(conn, row.plan_id, &row.date)?.is_some_and(|c| c.page_id == row.page_id && c.reason == row.reason && c.source == "auto");
            if !same {
                repo::insert(conn, &row)?;
            }
            report.assigned.push(view_of(conn, row, &stats)?);
        }
    }
    report.assigned.sort_by(|a, b| (&a.date, a.plan_id).cmp(&(&b.date, b.plan_id)));
    if report.assigned.is_empty() {
        report.notice = Some(if without_pages > 0 { TopicNotice::NoPages } else if without_wiki > 0 { TopicNotice::NoWiki } else { TopicNotice::NoPlan });
    }
    Ok(report)
}

/// The user picks the page for an occurrence. Refused while the lock is held.
pub fn set_manual(conn: &Connection, plan_id: i64, date: &str, page_id: i64, now_ms: i64) -> AppResult<()> {
    if crate::study::lock_held(conn)? {
        return Err(AppError::Invalid("the topic cannot be changed while the lock is held".into()));
    }
    plan::parse_date(date)?;
    let plan = store::get_plan(conn, plan_id)?.ok_or_else(|| AppError::Invalid("the plan does not exist".into()))?;
    let subject = subject_for_plan(conn, &plan.subject)?.ok_or_else(|| AppError::Invalid("the plan's subject has no wiki".into()))?;
    if !wiki_repo::list_pages(conn, subject.id)?.iter().any(|p| p.id == page_id) {
        return Err(AppError::Invalid("the page is not in the plan's wiki".into()));
    }
    repo::insert(
        conn,
        &AssignmentRow { plan_id, date: date.to_string(), page_id, source: "manual".into(), reason: Reason::Chosen.as_str().into(), assigned_at_ms: now_ms },
    )
}

/// Current assignments for dates in `from..=to`.
pub fn list_assignments(conn: &Connection, from: &str, to: &str) -> AppResult<Vec<AssignmentView>> {
    plan::parse_date(from)?;
    plan::parse_date(to)?;
    let mut out = Vec::new();
    for row in repo::current_between(conn, from, to)? {
        let pages = match store::get_plan(conn, row.plan_id)?.map(|p| subject_for_plan(conn, &p.subject)).transpose()?.flatten() {
            Some(subject) => recommend::page_stats(conn, subject.id)?,
            None => Vec::new(),
        };
        out.push(view_of(conn, row, &pages)?);
    }
    Ok(out)
}

/// Every assignment made for one occurrence, newest first.
pub fn assignment_history(conn: &Connection, plan_id: i64, date: &str) -> AppResult<Vec<AssignmentView>> {
    plan::parse_date(date)?;
    let pages = match store::get_plan(conn, plan_id)?.map(|p| subject_for_plan(conn, &p.subject)).transpose()?.flatten() {
        Some(subject) => recommend::page_stats(conn, subject.id)?,
        None => Vec::new(),
    };
    repo::history(conn, plan_id, date)?.into_iter().map(|row| view_of(conn, row, &pages)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::llm::config::{update_setting, LlmSettingKey};
    use crate::settings::SettingValue;
    use crate::testutil::{set_lock_for_test, TempDir};
    use chrono::Utc;

    /// 2026-11-06 03:00 UTC.
    fn now() -> i64 {
        Utc.with_ymd_and_hms(2026, 11, 6, 3, 0, 0).single().expect("time").timestamp_millis()
    }

    struct Rig {
        conn: Connection,
        _dir: TempDir,
        subject_id: i64,
    }

    fn rig(pages: &[(&str, usize)]) -> Rig {
        let mut conn = open_in_memory().expect("db");
        let dir = TempDir::new("topic");
        let root = dir.path().join("wiki");
        update_setting(&mut conn, LlmSettingKey::WikiDir, SettingValue::Text(root.to_string_lossy().into_owned()), 1).expect("dir");
        let subject_id = wiki_repo::insert_subject(&conn, "math", "수학", 1).expect("subject");
        std::fs::create_dir_all(root.join("math").join("pages")).expect("dirs");
        for (name, chars) in pages {
            std::fs::write(root.join("math").join("pages").join(format!("{name}.md")), "가".repeat(*chars)).expect("page");
            wiki_repo::upsert_page(&conn, subject_id, name, &format!("pages/{name}.md"), "h", 1).expect("row");
        }
        Rig { conn, _dir: dir, subject_id }
    }

    fn add_plan(conn: &Connection, name: &str, subject: &str, date: &str, time: &str) -> i64 {
        conn.execute(
            "INSERT INTO plans (series_id, plan_name, subject, start_date, start_time, sets, effective_from_date, created_at_ms)
             VALUES (1, ?1, ?2, ?3, ?4, 1, ?3, 0)",
            rusqlite::params![name, subject, date, time],
        )
        .expect("plan");
        conn.last_insert_rowid()
    }

    fn page_id(r: &Rig, name: &str) -> i64 {
        wiki_repo::get_page(&r.conn, r.subject_id, name).expect("get").expect("page").id
    }

    #[test]
    fn the_panel_prefers_the_assigned_page_then_the_name_match_then_the_weakest() {
        let r = rig(&[("미분", 900), ("적분", 900), ("빈페이지", 10)]);
        let plan = add_plan(&r.conn, "적분 연습", "수학", "2026-11-06", "09:00");
        let by_name = today_topic(&r.conn, &Utc, now()).expect("topic");
        assert_eq!((by_name.source, by_name.page.expect("page").page_name.as_str()), (Some(TopicSource::NameMatch), "적분"));

        set_manual(&r.conn, plan, "2026-11-06", page_id(&r, "미분"), now()).expect("manual");
        let assigned = today_topic(&r.conn, &Utc, now()).expect("topic");
        assert_eq!(assigned.source, Some(TopicSource::Assigned));
        assert_eq!(assigned.assigned_by.as_deref(), Some("manual"));
        assert_eq!(assigned.page.expect("page").page_name, "미분");

        add_plan(&r.conn, "다른 것", "수학", "2026-11-06", "08:00");
        let weak = today_topic(&r.conn, &Utc, now()).expect("topic");
        assert_eq!((weak.source, weak.page.expect("page").page_name.as_str()), (Some(TopicSource::Weak), "빈페이지"));
    }

    #[test]
    fn the_panel_explains_what_is_missing() {
        let r = rig(&[]);
        assert_eq!(today_topic(&r.conn, &Utc, now()).expect("topic").notice, Some(TopicNotice::NoPlan));
        add_plan(&r.conn, "복습", "수학", "2026-11-06", "09:00");
        assert_eq!(today_topic(&r.conn, &Utc, now()).expect("topic").notice, Some(TopicNotice::NoPages));
        add_plan(&r.conn, "영어", "영어", "2026-11-06", "07:00");
        assert_eq!(today_topic(&r.conn, &Utc, now()).expect("topic").notice, Some(TopicNotice::NoWiki));
    }

    #[test]
    fn auto_assignment_gives_each_plan_a_distinct_page_and_leaves_the_plan_alone() {
        let r = rig(&[("미분", 900), ("적분", 900), ("빈페이지", 10)]);
        let a = add_plan(&r.conn, "A", "수학", "2026-11-06", "09:00");
        let b = add_plan(&r.conn, "B", "수학", "2026-11-07", "09:00");
        let history_before: i64 = r.conn.query_row("SELECT COUNT(*) FROM plan_history", [], |x| x.get(0)).expect("count");
        let report = assign_next(&r.conn, &Utc, now()).expect("assign");
        assert_eq!(report.assigned.len(), 2);
        assert_eq!(report.assigned[0].page_name, "빈페이지", "the empty page goes first");
        assert_ne!(report.assigned[0].page_id, report.assigned[1].page_id);
        assert_eq!((report.assigned[0].plan_id, report.assigned[1].plan_id), (a, b));
        let history_after: i64 = r.conn.query_row("SELECT COUNT(*) FROM plan_history", [], |x| x.get(0)).expect("count");
        assert_eq!(history_before, history_after);
        // Running it again changes nothing.
        assign_next(&r.conn, &Utc, now()).expect("again");
        let rows: i64 = r.conn.query_row("SELECT COUNT(*) FROM topic_assignment", [], |x| x.get(0)).expect("count");
        assert_eq!(rows, 2);
    }

    #[test]
    fn a_manual_choice_stays_and_its_page_is_not_given_to_another_slot() {
        let r = rig(&[("미분", 900), ("적분", 900)]);
        let a = add_plan(&r.conn, "A", "수학", "2026-11-06", "09:00");
        let b = add_plan(&r.conn, "B", "수학", "2026-11-07", "09:00");
        let chosen = page_id(&r, "적분");
        set_manual(&r.conn, a, "2026-11-06", chosen, now()).expect("manual");
        let report = assign_next(&r.conn, &Utc, now()).expect("assign");
        assert_eq!(report.kept_manual, 1);
        let for_a = report.assigned.iter().find(|x| x.plan_id == a).expect("a");
        let for_b = report.assigned.iter().find(|x| x.plan_id == b).expect("b");
        assert_eq!((for_a.page_id, for_a.source.as_str()), (chosen, "manual"));
        assert_ne!(for_b.page_id, chosen);
        // The superseded choice stays in the history.
        set_manual(&r.conn, a, "2026-11-06", page_id(&r, "미분"), now() + 1).expect("change");
        assert_eq!(assignment_history(&r.conn, a, "2026-11-06").expect("history").len(), 2);
    }

    #[test]
    fn nothing_is_assigned_without_a_wiki_and_a_manual_change_is_refused_while_locked() {
        let r = rig(&[("미분", 900)]);
        let none = add_plan(&r.conn, "영어", "영어", "2026-11-06", "09:00");
        let report = assign_next(&r.conn, &Utc, now()).expect("assign");
        assert!(report.assigned.is_empty());
        assert_eq!(report.notice, Some(TopicNotice::NoWiki));
        assert!(set_manual(&r.conn, none, "2026-11-06", page_id(&r, "미분"), now()).is_err(), "the subject has no wiki");

        let math = add_plan(&r.conn, "수학", "수학", "2026-11-06", "10:00");
        set_lock_for_test(&r.conn, true);
        assert!(set_manual(&r.conn, math, "2026-11-06", page_id(&r, "미분"), now()).is_err());
        assert!(repo::current(&r.conn, math, "2026-11-06").expect("current").is_none());
        set_lock_for_test(&r.conn, false);
        assert!(set_manual(&r.conn, math, "2026-11-06", 9999, now()).is_err(), "a page of another wiki is refused");
        assert!(set_manual(&r.conn, math, "2026-11-06", page_id(&r, "미분"), now()).is_ok());
    }
}
