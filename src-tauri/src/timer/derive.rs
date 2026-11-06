//! The pure heart of the timer: `derive(now, occurrences, runs)` says what the database must
//! look like at `now`. It never reads the clock or the database, so the same inputs always give
//! the same answer whatever woke the engine (tick, wake from sleep, restart, plan edit), and
//! applying its output twice changes nothing.

use crate::error::{AppError, AppResult};
use crate::plan::{block_sequence, Block, BlockKind, ModeLookup, Occurrence};
use crate::study::{ModeConfig, StudyLevel};

use super::model::{EndReason, NewRun, NewSegment, NextPlan, Phase, RunRow, RunStatus, SegmentRow, TimerState};

/// A boundary observed this much after it happened still counts as seen live (tick jitter,
/// App Nap). Beyond it the engine was not watching: the segment is marked `late_join` or
/// `elapsed_while_down`.
pub const LATE_TOLERANCE_MS: i64 = 15_000;
const MINUTE_MS: i64 = 60_000;

#[derive(Clone, Debug)]
pub struct RunWithSegments {
    pub run: RunRow,
    /// Loaded for active runs only; terminal runs need no segment logic.
    pub segments: Vec<SegmentRow>,
}

pub struct DeriveInput<'a> {
    /// Effective now: never behind the highest time already observed.
    pub now_ms: i64,
    pub occurrences: &'a [Occurrence],
    pub modes: &'a ModeLookup,
    pub runs: &'a [RunWithSegments],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunTarget {
    Existing(i64),
    New(NewRun),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentClose {
    pub segment_id: i64,
    pub ended_at_ms: i64,
    pub reason: EndReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunFinish {
    pub status: RunStatus,
    pub ended_at_ms: i64,
    pub detail_code: Option<&'static str>,
}

/// Everything to write for one run, in order: create the run, close segments, open segments,
/// finish the run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunDelta {
    pub target: RunTarget,
    pub closes: Vec<SegmentClose>,
    pub opens: Vec<NewSegment>,
    pub finish: Option<RunFinish>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Derived {
    pub deltas: Vec<RunDelta>,
    /// The lock the database must hold after applying `deltas`.
    pub lock_target: bool,
    /// The state after applying `deltas` (ids are `None` for rows not written yet).
    pub state: TimerState,
}

enum Item<'a> {
    Run(&'a RunWithSegments),
    Occurrence(&'a Occurrence),
}

/// A run's frozen shape: planned start plus the mode snapshot it was created with.
struct Frozen {
    planned_start_ms: i64,
    sets_total: i64,
    blocks: Vec<Block>,
}

struct Progress {
    closes: Vec<SegmentClose>,
    opens: Vec<NewSegment>,
    /// Every block is over: the run is complete.
    finished: bool,
    /// The last close or open reason, to tell a live finish from one found after the fact.
    last_reason: Option<EndReason>,
    current: Option<Current>,
}

struct Current {
    segment_id: Option<i64>,
    seq: i64,
    kind: BlockKind,
    planned_end_ms: i64,
}

fn mode_config_of(run: &RunRow) -> AppResult<ModeConfig> {
    let level = StudyLevel::parse(&run.mode_level)
        .ok_or_else(|| AppError::Internal(format!("run {} has unknown mode level", run.id)))?;
    Ok(ModeConfig {
        level,
        study_min: run.study_min,
        break_min: run.break_min,
        long_break_min: run.long_break_min,
        sets_before_long_break: run.sets_before_long_break,
    })
}

fn frozen_of(run: &RunRow) -> AppResult<Frozen> {
    Ok(Frozen {
        planned_start_ms: run.planned_start_ms,
        sets_total: run.sets_total,
        blocks: block_sequence(&mode_config_of(run)?, run.sets_total)?,
    })
}

fn lag_reason(lag_ms: i64) -> EndReason {
    if lag_ms <= LATE_TOLERANCE_MS { EndReason::Elapsed } else { EndReason::ElapsedWhileDown }
}

/// Walks a run's blocks and decides what to close and open at `now`. `force_late` marks the first
/// opened segment `late_join` even when seen on time (a run pushed back by an earlier one).
fn progress(now: i64, frozen: &Frozen, existing: &[SegmentRow], force_late: bool) -> Progress {
    let mut out = Progress { closes: Vec::new(), opens: Vec::new(), finished: false, last_reason: None, current: None };
    let mut start = frozen.planned_start_ms;
    let mut first_open = true;
    for (index, block) in frozen.blocks.iter().enumerate() {
        let seq = i64::try_from(index).unwrap_or(i64::MAX);
        let seg_start = start;
        let end = start + block.minutes * MINUTE_MS;
        start = end;
        match existing.iter().find(|s| s.seq == seq) {
            Some(seg) if seg.ended_at_ms.is_some() => continue,
            Some(seg) => {
                if now >= end {
                    let reason = lag_reason(now - end);
                    out.closes.push(SegmentClose { segment_id: seg.id, ended_at_ms: end, reason });
                    out.last_reason = Some(reason);
                } else {
                    out.current = Some(Current {
                        segment_id: Some(seg.id),
                        seq,
                        kind: block.kind,
                        planned_end_ms: end,
                    });
                    return out;
                }
            }
            None => {
                if now < seg_start {
                    return out;
                }
                let late = now - seg_start > LATE_TOLERANCE_MS || (force_late && first_open);
                first_open = false;
                if now >= end {
                    let reason = lag_reason(now - end);
                    out.opens.push(NewSegment {
                        seq,
                        kind: block.kind,
                        planned_start_ms: seg_start,
                        planned_end_ms: end,
                        started_at_ms: seg_start,
                        ended_at_ms: Some(end),
                        end_reason: Some(reason),
                        late_join: late || reason == EndReason::ElapsedWhileDown,
                    });
                    out.last_reason = Some(reason);
                } else {
                    // A segment found already running starts when it was found, not in the past.
                    let started = if now - seg_start > LATE_TOLERANCE_MS { now } else { seg_start };
                    out.opens.push(NewSegment {
                        seq,
                        kind: block.kind,
                        planned_start_ms: seg_start,
                        planned_end_ms: end,
                        started_at_ms: started,
                        ended_at_ms: None,
                        end_reason: None,
                        late_join: late,
                    });
                    out.current = Some(Current { segment_id: None, seq, kind: block.kind, planned_end_ms: end });
                    return out;
                }
            }
        }
    }
    out.finished = true;
    out
}

fn set_index(frozen: &Frozen, seq: i64) -> i64 {
    let upto = usize::try_from(seq).map_or(frozen.blocks.len(), |s| (s + 1).min(frozen.blocks.len()));
    let studies = frozen.blocks[..upto].iter().filter(|b| b.kind == BlockKind::Study).count();
    i64::try_from(studies).unwrap_or(i64::MAX)
}

struct ActiveView {
    run_id: Option<i64>,
    current: Option<Current>,
    set_index: i64,
    sets_total: i64,
    planned_end_ms: i64,
}

pub fn derive(input: &DeriveInput<'_>) -> AppResult<Derived> {
    let now = input.now_ms;
    let mut items: Vec<(i64, i64, Item<'_>)> = Vec::new();
    for r in input.runs {
        items.push((r.run.planned_start_ms, r.run.plan_id, Item::Run(r)));
    }
    // `plan_runs` is unique per (plan, date); a plan with two occurrences on one date (a move
    // onto a day that already has one) gets a run for the earlier only.
    let mut seen: Vec<(i64, &str)> = input.runs.iter().map(|r| (r.run.plan_id, r.run.date.as_str())).collect();
    for o in input.occurrences {
        let key = (o.plan_id, o.date.as_str());
        if !seen.contains(&key) {
            seen.push(key);
            items.push((o.planned_start_ms, o.plan_id, Item::Occurrence(o)));
        }
    }
    items.sort_by_key(|(start, plan_id, _)| (*start, *plan_id));

    let mut deltas = Vec::new();
    let mut active: Option<ActiveView> = None;
    let mut next: Option<NextPlan> = None;
    // End of the previous run that actually ran: a later run never starts inside it.
    let mut cursor = i64::MIN;

    for (_, _, item) in &items {
        match item {
            Item::Run(r) => match r.run.status {
                RunStatus::Active => {
                    let frozen = frozen_of(&r.run)?;
                    let p = progress(now, &frozen, &r.segments, false);
                    cursor = cursor.max(r.run.planned_end_ms);
                    if p.finished {
                        let detail = (p.last_reason == Some(EndReason::ElapsedWhileDown)).then_some("ended_while_down");
                        deltas.push(RunDelta {
                            target: RunTarget::Existing(r.run.id),
                            closes: p.closes,
                            opens: p.opens,
                            finish: Some(RunFinish {
                                status: RunStatus::Completed,
                                ended_at_ms: r.run.planned_end_ms,
                                detail_code: detail,
                            }),
                        });
                    } else {
                        if !p.closes.is_empty() || !p.opens.is_empty() {
                            deltas.push(RunDelta {
                                target: RunTarget::Existing(r.run.id),
                                closes: p.closes,
                                opens: p.opens,
                                finish: None,
                            });
                        }
                        if active.is_none() {
                            active = Some(ActiveView {
                                run_id: Some(r.run.id),
                                set_index: p.current.as_ref().map_or(0, |c| set_index(&frozen, c.seq)),
                                current: p.current,
                                sets_total: frozen.sets_total,
                                planned_end_ms: r.run.planned_end_ms,
                            });
                        }
                    }
                }
                RunStatus::Completed | RunStatus::Aborted => {
                    cursor = cursor.max(r.run.ended_at_ms.unwrap_or(r.run.planned_end_ms));
                }
                RunStatus::Missed | RunStatus::Skipped => {}
            },
            Item::Occurrence(o) => {
                // An occurrence that began before its plan existed is not the user's commitment.
                if !o.eligible_for_missed {
                    continue;
                }
                let cfg = input.modes.config(o.level)?;
                let eff_start = o.planned_start_ms.max(cursor);
                let shifted = eff_start > o.planned_start_ms;
                let end = eff_start + o.duration_min * MINUTE_MS;
                let new_run = |status: RunStatus, actual: Option<i64>, ended: Option<i64>, detail: Option<&'static str>| {
                    NewRun {
                        plan_id: o.plan_id,
                        date: o.date.clone(),
                        planned_start_ms: eff_start,
                        planned_end_ms: end,
                        actual_start_ms: actual,
                        mode_level: cfg.level.as_str().to_string(),
                        study_min: cfg.study_min,
                        break_min: cfg.break_min,
                        long_break_min: cfg.long_break_min,
                        sets_before_long_break: cfg.sets_before_long_break,
                        sets_total: o.sets,
                        status,
                        ended_at_ms: ended,
                        detail_code: detail,
                    }
                };
                if now < eff_start {
                    cursor = cursor.max(end);
                    if next.is_none() {
                        next = Some(NextPlan {
                            plan_id: o.plan_id,
                            name: o.name.clone(),
                            subject: o.subject.clone(),
                            date: o.date.clone(),
                            sets: o.sets,
                            planned_start_ms: eff_start,
                            planned_end_ms: end,
                        });
                    }
                } else if now >= end {
                    deltas.push(RunDelta {
                        target: RunTarget::New(new_run(RunStatus::Missed, None, Some(end), Some("not_started"))),
                        closes: Vec::new(),
                        opens: Vec::new(),
                        finish: None,
                    });
                } else {
                    let late = now - eff_start > LATE_TOLERANCE_MS;
                    let detail = if shifted {
                        Some("shifted_by_overlap")
                    } else if late {
                        Some("joined_late")
                    } else {
                        None
                    };
                    let run = new_run(RunStatus::Active, Some(now), None, detail);
                    let frozen = Frozen {
                        planned_start_ms: eff_start,
                        sets_total: o.sets,
                        blocks: block_sequence(cfg, o.sets)?,
                    };
                    let p = progress(now, &frozen, &[], shifted);
                    cursor = cursor.max(end);
                    if active.is_none() {
                        active = Some(ActiveView {
                            run_id: None,
                            set_index: p.current.as_ref().map_or(0, |c| set_index(&frozen, c.seq)),
                            current: p.current,
                            sets_total: o.sets,
                            planned_end_ms: end,
                        });
                    }
                    deltas.push(RunDelta { target: RunTarget::New(run), closes: p.closes, opens: p.opens, finish: None });
                }
            }
        }
    }

    let lock_target = active.is_some();
    let state = match active {
        Some(view) => TimerState {
            phase: view.current.as_ref().map_or(Phase::Idle, |c| Phase::of(c.kind)),
            run_id: view.run_id,
            segment_id: view.current.as_ref().and_then(|c| c.segment_id),
            set_index: view.set_index,
            sets_total: view.sets_total,
            planned_end_ms: Some(view.current.as_ref().map_or(view.planned_end_ms, |c| c.planned_end_ms)),
            lock_held: true,
            now_ms: now,
            next,
        },
        None => TimerState {
            phase: Phase::Idle,
            run_id: None,
            segment_id: None,
            set_index: 0,
            sets_total: 0,
            planned_end_ms: None,
            lock_held: false,
            now_ms: now,
            next,
        },
    };
    Ok(Derived { deltas, lock_target, state })
}
