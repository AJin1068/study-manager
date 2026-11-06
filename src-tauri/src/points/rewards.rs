//! Every number of the reward system in one place (D-27 point values, D-28 level curve). The
//! balance of these values needs about a week of real use to judge, so they are plain constants
//! to tune here and nowhere else.

/// Recognized study time that makes one reward window.
pub const WINDOW_MS: i64 = 30 * 60 * 1000;
/// Paid for a window with no away time, no zoning out, and no distraction attempt.
pub const WINDOW_POINTS: i64 = 20;
/// Paid once per run that started within [`START_TOLERANCE_MS`] of its planned start.
pub const START_BONUS_POINTS: i64 = 10;
pub const START_TOLERANCE_MS: i64 = 5 * 60 * 1000;
/// Deducted for an emergency pass (recorded in `study_passes.penalty_points`).
pub const EMERGENCY_PASS_PENALTY: i64 = 30;
/// Deducted for a run ended by early release (`plan_runs.status = 'aborted'`).
pub const ABORT_PENALTY: i64 = 30;

pub const REASON_FOCUS_WINDOW: &str = "focus_window";
pub const REASON_START_BONUS: &str = "start_bonus";
pub const REASON_EMERGENCY_PASS: &str = "emergency_pass";
pub const REASON_RUN_ABORTED: &str = "run_aborted";
/// Paid for a right answer to a review question (never for the early-release proof). Ledger rows
/// are keyed `quiz:attempt:<id>`, so one attempt pays once.
pub const QUIZ_BONUS_POINTS: i64 = 2;
/// Right answers per local day that pay the bonus; later ones pay nothing.
pub const QUIZ_BONUS_DAILY_CAP: i64 = 5;
pub const REASON_QUIZ_BONUS: &str = "quiz_bonus";
/// Spent in the character shop: a negative ledger row keyed `buy:<item>:<n>`. The level reads
/// earned points only, so spending never lowers it.
pub const REASON_SHOP_PURCHASE: &str = "shop_purchase";
/// Spent on a gift (negative row keyed `gift:<uuid>`) and returned when the gift certainly did not
/// arrive (positive row keyed `gift_refund:<uuid>`). The refund is not counted as earned.
pub const REASON_GIFT_SENT: &str = "gift_sent";
pub const REASON_GIFT_REFUND: &str = "gift_refund";

/// Points earned in total needed to reach level `index + 1`. Early levels come fast, later ones
/// slowly. Level 1 needs nothing.
pub const LEVEL_THRESHOLDS: [i64; 10] = [0, 20, 50, 90, 140, 200, 280, 380, 500, 640];

/// Growth stages and the first level of each, in order.
pub const STAGE_FIRST_LEVELS: [(Stage, i64); 4] =
    [(Stage::Seed, 1), (Stage::Sprout, 3), (Stage::SmallTree, 6), (Stage::ForestSpirit, 9)];

#[derive(serde::Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Seed,
    Sprout,
    SmallTree,
    ForestSpirit,
}

impl Stage {
    /// 씨앗 → 새싹 → 작은 나무 → 숲의 정령.
    pub fn label(self) -> &'static str {
        match self {
            Stage::Seed => "씨앗",
            Stage::Sprout => "새싹",
            Stage::SmallTree => "작은 나무",
            Stage::ForestSpirit => "숲의 정령",
        }
    }
}

/// Level for `earned` total points (never below 1; capped at the last threshold).
pub fn level_for(earned: i64) -> i64 {
    let reached = LEVEL_THRESHOLDS.iter().take_while(|needed| earned >= **needed).count();
    i64::try_from(reached.max(1)).unwrap_or(1)
}

pub fn stage_for_level(level: i64) -> Stage {
    STAGE_FIRST_LEVELS
        .iter()
        .rev()
        .find(|(_, first)| level >= *first)
        .map_or(Stage::Seed, |(stage, _)| *stage)
}

/// Total points at which the next level starts; `None` at the top level.
pub fn next_level_at(level: i64) -> Option<i64> {
    usize::try_from(level).ok().and_then(|index| LEVEL_THRESHOLDS.get(index).copied())
}
