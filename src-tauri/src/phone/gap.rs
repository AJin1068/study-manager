//! Gap detection (pure): the stretches of a locked study segment in which the phone was not under
//! the lock. The publisher feeds it one observation per look and it answers with the gaps that
//! just ended; nothing here touches the network, the database or the clock.
//!
//! Three kinds (`phone_gap.kind`):
//! - `unreachable`: no heartbeat for [`UNREACHABLE_AFTER_MS`] or more (counted from the last beat,
//!   or from the segment start when the phone never beat in it);
//! - `service_disabled`: the phone's freshest heartbeat says its blocking service is off;
//! - `lease_expired`: the desktop app wants the phone locked but no published lease covers it,
//!   for [`UNREACHABLE_AFTER_MS`] or more (the server could not be reached with the lock).
//!
//! A gap is written once, when it is over, so its row is complete: start, end, kind. Every time is
//! on this PC's clock (the publisher converts the server's beat time before it gets here). The
//! start of a gap is derived from the beat/lease times, not from when it was noticed, so a
//! restart in the middle of one finds the same start and the unique key keeps it to one row.

use crate::store::phone::GapRow;

/// A heartbeat is due every 15 s; silence this long is a gap (decision: 90 s).
pub const UNREACHABLE_AFTER_MS: i64 = 90_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GapKind {
    Unreachable,
    LeaseExpired,
    ServiceDisabled,
}

impl GapKind {
    pub const ALL: [GapKind; 3] = [GapKind::Unreachable, GapKind::LeaseExpired, GapKind::ServiceDisabled];

    /// The value stored in `phone_gap.kind` (matches the CHECK list).
    pub fn as_str(self) -> &'static str {
        match self {
            GapKind::Unreachable => "unreachable",
            GapKind::LeaseExpired => "lease_expired",
            GapKind::ServiceDisabled => "service_disabled",
        }
    }

    fn slot(self) -> usize {
        match self {
            GapKind::Unreachable => 0,
            GapKind::LeaseExpired => 1,
            GapKind::ServiceDisabled => 2,
        }
    }
}

/// The locked study segment being watched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watch {
    pub run_id: i64,
    pub segment_started_ms: i64,
    pub segment_end_ms: i64,
}

/// What the phone's last heartbeat said, on this PC's clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heard {
    pub last_beat_ms: Option<i64>,
    pub service_enabled: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    pub now_ms: i64,
    /// `None` when no study segment is locked right now.
    pub watch: Option<Watch>,
    /// `None` when the server could not be asked: then nothing is known about the phone, and
    /// neither `unreachable` nor `service_disabled` changes.
    pub heard: Option<Heard>,
    /// Until when the lease the server holds for this segment covers the phone; `None` when no
    /// lease for it was ever acknowledged.
    pub covered_until_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenGap {
    pub kind: GapKind,
    pub since_ms: i64,
}

#[derive(Debug, Default)]
pub struct GapTracker {
    last_watch: Option<Watch>,
    open: [Option<i64>; 3],
}

impl GapTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// The gaps that are happening now (not yet written).
    pub fn open_gaps(&self) -> Vec<OpenGap> {
        GapKind::ALL.iter().filter_map(|kind| self.open[kind.slot()].map(|since_ms| OpenGap { kind: *kind, since_ms })).collect()
    }

    /// Takes one look and returns the gaps that ended with it, ready to append.
    pub fn observe(&mut self, obs: &Observation) -> Vec<GapRow> {
        let mut done = Vec::new();
        // The watched segment changed or ended: whatever was open ended with the old segment.
        if let Some(old) = self.last_watch {
            if obs.watch != Some(old) {
                let cap = obs.now_ms.min(old.segment_end_ms);
                for kind in GapKind::ALL {
                    self.close(kind, old.run_id, cap, &mut done);
                }
                self.last_watch = None;
            }
        }
        let Some(watch) = obs.watch else { return done };
        self.last_watch = Some(watch);

        let silent_since = obs.heard.map(|heard| {
            let reference = heard.last_beat_ms.unwrap_or(i64::MIN).max(watch.segment_started_ms);
            (reference, obs.now_ms - reference >= UNREACHABLE_AFTER_MS)
        });
        let fresh_beat = obs.heard.and_then(|heard| heard.last_beat_ms).filter(|beat| obs.now_ms - beat < UNREACHABLE_AFTER_MS);

        // None = nothing is known: leave the gap as it is.
        let unreachable = silent_since.map(|(reference, silent)| silent.then_some(reference));
        let service_disabled = obs.heard.map(|heard| match (fresh_beat, heard.service_enabled) {
            (Some(beat), Some(false)) if beat >= watch.segment_started_ms => Some(beat),
            _ => None,
        });
        let uncovered_start = obs.covered_until_ms.map_or(watch.segment_started_ms, |until| until.max(watch.segment_started_ms));
        let lease_expired = Some((obs.now_ms - uncovered_start >= UNREACHABLE_AFTER_MS && obs.now_ms < watch.segment_end_ms).then_some(uncovered_start));

        for (kind, active) in [
            (GapKind::Unreachable, unreachable),
            (GapKind::ServiceDisabled, service_disabled),
            (GapKind::LeaseExpired, lease_expired),
        ] {
            let Some(active) = active else { continue };
            match (self.open[kind.slot()], active) {
                (None, Some(start)) => self.open[kind.slot()] = Some(start),
                (Some(_), None) => {
                    // It ended when the phone was last known to be fine: the beat that broke the
                    // silence, or now.
                    let end = match kind {
                        GapKind::Unreachable => fresh_beat.unwrap_or(obs.now_ms),
                        GapKind::ServiceDisabled => obs.heard.and_then(|h| h.last_beat_ms).unwrap_or(obs.now_ms),
                        GapKind::LeaseExpired => obs.now_ms,
                    };
                    self.close(kind, watch.run_id, end.min(obs.now_ms), &mut done);
                }
                _ => {}
            }
        }
        done
    }

    fn close(&mut self, kind: GapKind, run_id: i64, end_ms: i64, out: &mut Vec<GapRow>) {
        if let Some(start_ms) = self.open[kind.slot()].take() {
            out.push(GapRow { run_id, start_ms, end_ms: end_ms.max(start_ms), kind: kind.as_str().to_string() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000;
    const WATCH: Watch = Watch { run_id: 7, segment_started_ms: 0, segment_end_ms: 3_000 * S };

    fn look(now_s: i64, last_beat_s: Option<i64>, service: Option<bool>) -> Observation {
        Observation {
            now_ms: now_s * S,
            watch: Some(WATCH),
            heard: Some(Heard { last_beat_ms: last_beat_s.map(|s| s * S), service_enabled: service }),
            covered_until_ms: Some(WATCH.segment_end_ms + 60 * S),
        }
    }

    #[test]
    fn kinds_match_the_check_list_of_the_table() {
        let sql = include_str!("../db/migrations/m026_phone_gap.sql");
        for kind in GapKind::ALL {
            assert!(sql.contains(&format!("'{}'", kind.as_str())), "{kind:?}");
        }
    }

    #[test]
    fn a_ninety_second_silence_is_one_gap_written_when_the_beats_return() {
        let mut tracker = GapTracker::new();
        // Beats every 15 s up to t=100 s, then silence.
        assert!(tracker.observe(&look(100, Some(100), Some(true))).is_empty());
        assert!(tracker.observe(&look(189, Some(100), Some(true))).is_empty(), "89.0 s of silence is not a gap yet");
        assert!(tracker.open_gaps().is_empty());
        assert!(tracker.observe(&look(190, Some(100), Some(true))).is_empty(), "90 s opens it; nothing is written yet");
        assert_eq!(tracker.open_gaps(), vec![OpenGap { kind: GapKind::Unreachable, since_ms: 100 * S }]);
        assert!(tracker.observe(&look(250, Some(100), Some(true))).is_empty(), "still open, still no row");
        // The phone is back: its newest beat is at t=262 s.
        let rows = tracker.observe(&look(265, Some(262), Some(true)));
        assert_eq!(rows, vec![GapRow { run_id: 7, start_ms: 100 * S, end_ms: 262 * S, kind: "unreachable".into() }]);
        assert!(tracker.open_gaps().is_empty());
        assert!(tracker.observe(&look(280, Some(277), Some(true))).is_empty(), "no second row");
    }

    #[test]
    fn a_gap_still_open_when_the_segment_ends_is_closed_at_its_end() {
        let mut tracker = GapTracker::new();
        tracker.observe(&look(100, Some(100), Some(true)));
        tracker.observe(&look(300, Some(100), Some(true)));
        let after = Observation { now_ms: 3_040 * S, watch: None, heard: None, covered_until_ms: None };
        let rows = tracker.observe(&after);
        assert_eq!(rows, vec![GapRow { run_id: 7, start_ms: 100 * S, end_ms: 3_000 * S, kind: "unreachable".into() }]);
    }

    #[test]
    fn a_phone_that_never_beat_is_counted_from_the_segment_start() {
        let mut tracker = GapTracker::new();
        tracker.observe(&look(89, None, None));
        assert!(tracker.open_gaps().is_empty());
        tracker.observe(&look(90, None, None));
        assert_eq!(tracker.open_gaps(), vec![OpenGap { kind: GapKind::Unreachable, since_ms: 0 }]);
        // A beat from before the segment began does not move the start earlier.
        let mut late = GapTracker::new();
        let obs = Observation { watch: Some(Watch { segment_started_ms: 50 * S, ..WATCH }), ..look(200, Some(10), None) };
        late.observe(&obs);
        assert_eq!(late.open_gaps(), vec![OpenGap { kind: GapKind::Unreachable, since_ms: 50 * S }]);
    }

    #[test]
    fn a_server_that_cannot_be_asked_changes_nothing_about_the_phone() {
        let mut tracker = GapTracker::new();
        tracker.observe(&look(100, Some(100), Some(true)));
        let unknown = Observation { heard: None, ..look(500, None, None) };
        assert!(tracker.observe(&unknown).is_empty());
        assert!(tracker.open_gaps().is_empty(), "no silence is claimed from a failed read");
        tracker.observe(&look(300, Some(100), Some(true)));
        assert_eq!(tracker.open_gaps().len(), 1);
        assert!(tracker.observe(&unknown).is_empty());
        assert_eq!(tracker.open_gaps().len(), 1, "an open gap stays open across a failed read");
    }

    #[test]
    fn a_switched_off_service_is_a_gap_while_the_beats_keep_coming() {
        let mut tracker = GapTracker::new();
        tracker.observe(&look(100, Some(100), Some(true)));
        tracker.observe(&look(130, Some(130), Some(false)));
        assert_eq!(tracker.open_gaps(), vec![OpenGap { kind: GapKind::ServiceDisabled, since_ms: 130 * S }]);
        tracker.observe(&look(160, Some(160), Some(false)));
        let rows = tracker.observe(&look(190, Some(190), Some(true)));
        assert_eq!(rows, vec![GapRow { run_id: 7, start_ms: 130 * S, end_ms: 190 * S, kind: "service_disabled".into() }]);
    }

    #[test]
    fn a_lock_the_server_never_got_is_a_lease_gap_after_ninety_seconds() {
        let mut tracker = GapTracker::new();
        let uncovered = |now_s: i64| Observation { covered_until_ms: None, ..look(now_s, Some(now_s), Some(true)) };
        tracker.observe(&uncovered(89));
        assert!(tracker.open_gaps().is_empty());
        tracker.observe(&uncovered(95));
        assert_eq!(tracker.open_gaps(), vec![OpenGap { kind: GapKind::LeaseExpired, since_ms: 0 }]);
        // The lease finally got through at t=140 s.
        let rows = tracker.observe(&look(140, Some(140), Some(true)));
        assert_eq!(rows, vec![GapRow { run_id: 7, start_ms: 0, end_ms: 140 * S, kind: "lease_expired".into() }]);
    }

    #[test]
    fn an_expired_lease_counts_from_its_end() {
        let mut tracker = GapTracker::new();
        let expired = Observation { covered_until_ms: Some(200 * S), ..look(291, Some(291), Some(true)) };
        tracker.observe(&expired);
        assert_eq!(tracker.open_gaps(), vec![OpenGap { kind: GapKind::LeaseExpired, since_ms: 200 * S }]);
    }

    #[test]
    fn a_new_segment_ends_the_old_segments_gaps() {
        let mut tracker = GapTracker::new();
        tracker.observe(&look(100, Some(100), Some(true)));
        tracker.observe(&look(300, Some(100), Some(true)));
        let next = Watch { run_id: 7, segment_started_ms: 3_900 * S, segment_end_ms: 5_000 * S };
        let rows = tracker.observe(&Observation { now_ms: 3_901 * S, watch: Some(next), heard: Some(Heard { last_beat_ms: Some(3_900 * S), service_enabled: Some(true) }), covered_until_ms: Some(5_060 * S) });
        assert_eq!(rows, vec![GapRow { run_id: 7, start_ms: 100 * S, end_ms: 3_000 * S, kind: "unreachable".into() }]);
    }
}
