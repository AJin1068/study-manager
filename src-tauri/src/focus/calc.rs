//! The pure part of focus measurement: observations in, events and minute buckets out. Nothing
//! here reads a clock, the OS, or the database, so every rule is testable with made-up times.
//!
//! Rules (D-25, D-26):
//! - Only a switch from a study app to a non-study app is a deviation (`away`); moving between
//!   study apps, or sitting in a non-study app from the start, is not. `return` follows only an
//!   `away`.
//! - Recognized (credited) time is study app in front AND input idle under 10 minutes.
//! - Idle of 10 minutes or more is a zoning-out suspicion (`idle_start`/`idle_end`) and is not
//!   credited.
//! - Time is attributed from one observation to the next. A longer gap than `MAX_GAP_MS` (sleep,
//!   app not running) is attributed to nothing, and idle that accrued across it is not counted.

use super::model::{FocusEventKind, FocusEventRow, MinuteDelta};

pub const SAMPLE_INTERVAL_MS: i64 = 5_000;
/// Three missed samples: beyond this the machine was asleep or the app was not running.
pub const MAX_GAP_MS: i64 = 3 * SAMPLE_INTERVAL_MS;
pub const IDLE_SUSPECT_MS: i64 = 10 * 60_000;
const MINUTE_MS: i64 = 60_000;

/// What was in front, already classified against the study-app list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Foreground {
    Study,
    /// A non-study app, by id.
    Other(String),
    /// Nothing is in front (login window, locked screen): neither credited nor a deviation.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub at_ms: i64,
    pub foreground: Foreground,
    pub idle_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Credited,
    Away,
    Idle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingEvent {
    pub at_ms: i64,
    pub kind: FocusEventKind,
    pub app_id: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub events: Vec<PendingEvent>,
    pub minutes: Vec<MinuteDelta>,
}

#[derive(Clone, Copy, Debug)]
struct Prev {
    at_ms: i64,
    class: Option<Class>,
}

/// Follows one study segment. A new segment (or a restart) starts a new tracker: it carries no
/// memory of earlier switches, so nothing is counted for time it did not observe.
#[derive(Debug, Default)]
pub struct Tracker {
    prev: Option<Prev>,
    /// Whether the last known foreground was a study app; `None` before the first known one.
    last_in_study: Option<bool>,
    /// An `away` was recorded and no `return` has followed.
    deviated: bool,
    idle: bool,
    /// Idle time is only believed from this instant on (set after an unobserved gap).
    idle_origin_ms: Option<i64>,
}

impl Tracker {
    /// Feeds the next observation. `segment_start_ms` clips the first span so time before the
    /// segment began is never credited. An observation not later than the previous one is ignored.
    pub fn step(&mut self, obs: &Observation, segment_start_ms: i64) -> Step {
        let mut out = Step::default();
        if self.prev.is_some_and(|p| obs.at_ms <= p.at_ms) {
            return out;
        }
        let gap = self.prev.map(|p| obs.at_ms - p.at_ms);
        let gapped = gap.is_some_and(|g| g > MAX_GAP_MS);
        if gapped {
            self.idle_origin_ms = Some(obs.at_ms);
        }
        if let (Some(prev), false) = (self.prev, gapped) {
            if let Some(class) = prev.class {
                add_span(&mut out.minutes, prev.at_ms.max(segment_start_ms), obs.at_ms, class);
            }
        }

        let idle_ms = self.idle_origin_ms.map_or(obs.idle_ms, |origin| obs.idle_ms.min(obs.at_ms - origin)).max(0);
        let idle_now = idle_ms >= IDLE_SUSPECT_MS;

        match &obs.foreground {
            Foreground::Study => {
                if self.deviated {
                    out.events.push(PendingEvent { at_ms: obs.at_ms, kind: FocusEventKind::Return, app_id: None });
                    self.deviated = false;
                }
                self.last_in_study = Some(true);
            }
            Foreground::Other(app) => {
                if self.last_in_study == Some(true) {
                    out.events.push(PendingEvent {
                        at_ms: obs.at_ms,
                        kind: FocusEventKind::Away,
                        app_id: Some(app.clone()),
                    });
                    self.deviated = true;
                }
                self.last_in_study = Some(false);
            }
            Foreground::Unknown => {}
        }
        if idle_now != self.idle {
            let kind = if idle_now { FocusEventKind::IdleStart } else { FocusEventKind::IdleEnd };
            out.events.push(PendingEvent { at_ms: obs.at_ms, kind, app_id: None });
            self.idle = idle_now;
        }

        let class = match (&obs.foreground, idle_now) {
            (Foreground::Unknown, _) => None,
            (Foreground::Other(_), _) => Some(Class::Away),
            (Foreground::Study, true) => Some(Class::Idle),
            (Foreground::Study, false) => Some(Class::Credited),
        };
        self.prev = Some(Prev { at_ms: obs.at_ms, class });
        out
    }
}

/// Adds `start..end` to the minute buckets it covers, splitting at wall-clock minute boundaries.
fn add_span(minutes: &mut Vec<MinuteDelta>, start: i64, end: i64, class: Class) {
    let mut cursor = start;
    while cursor < end {
        let minute_start_ms = cursor.div_euclid(MINUTE_MS) * MINUTE_MS;
        let next = (minute_start_ms + MINUTE_MS).min(end);
        let ms = next - cursor;
        let index = match minutes.iter().position(|m| m.minute_start_ms == minute_start_ms) {
            Some(i) => i,
            None => {
                minutes.push(MinuteDelta { minute_start_ms, ..MinuteDelta::default() });
                minutes.len() - 1
            }
        };
        let bucket = &mut minutes[index];
        match class {
            Class::Credited => bucket.credited_ms += ms,
            Class::Away => bucket.away_ms += ms,
            Class::Idle => bucket.idle_ms += ms,
        }
        cursor = next;
    }
}

/// Session focus score, 0 to 100 (D-25): recognized time over study time. Block attempts do not
/// reduce it. `None` when there was no study time to judge.
pub fn focus_score(credited_ms: i64, studied_ms: i64) -> Option<i64> {
    if studied_ms <= 0 {
        return None;
    }
    let credited = credited_ms.clamp(0, studied_ms);
    Some((credited * 100 + studied_ms / 2) / studied_ms)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventMetrics {
    pub away_count: i64,
    pub idle_suspect_count: i64,
    /// Study start to the first deviation.
    pub first_away_after_ms: Option<i64>,
    /// First deviation to the first return after it.
    pub return_after_ms: Option<i64>,
}

/// Metrics of one segment from its events, oldest first.
pub fn event_metrics(events: &[FocusEventRow], segment_start_ms: i64) -> EventMetrics {
    let mut metrics = EventMetrics::default();
    let mut first_away: Option<i64> = None;
    for event in events {
        match event.kind {
            FocusEventKind::Away => {
                metrics.away_count += 1;
                if first_away.is_none() {
                    first_away = Some(event.at_ms);
                    metrics.first_away_after_ms = Some((event.at_ms - segment_start_ms).max(0));
                }
            }
            FocusEventKind::Return => {
                if let (Some(away_at), None) = (first_away, metrics.return_after_ms) {
                    metrics.return_after_ms = Some(event.at_ms - away_at);
                }
            }
            FocusEventKind::IdleStart => metrics.idle_suspect_count += 1,
            FocusEventKind::IdleEnd | FocusEventKind::SelfReport => {}
        }
    }
    metrics
}
