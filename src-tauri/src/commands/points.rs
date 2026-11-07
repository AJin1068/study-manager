//! Point and level commands. Reading settles first, so the ledger is current whenever the UI asks.

use tauri::State;

use super::{AppState, lock_db};
use crate::clock::now_ms;
use crate::error::AppResult;
use crate::points::{self, LedgerEntry, PointsStatus};
use crate::store;

const MAX_LEDGER_ROWS: i64 = 500;

#[tauri::command]
pub fn get_points_status(state: State<'_, AppState>) -> AppResult<PointsStatus> {
    let mut conn = lock_db(&state.db)?;
    points::settled_status(&mut conn, now_ms()?)
}

/// Newest first.
#[tauri::command]
pub fn list_point_ledger(state: State<'_, AppState>, limit: i64) -> AppResult<Vec<LedgerEntry>> {
    let mut conn = lock_db(&state.db)?;
    points::settle(&mut conn, now_ms()?)?;
    store::list_ledger(&conn, limit.clamp(1, MAX_LEDGER_ROWS))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::points::rewards::Stage;
    use crate::points::{self, LedgerEntry};

    #[test]
    fn points_wire_shapes_match_the_shared_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/api/fixtures/points-contract.json"
        );
        let expected: Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let actual = json!({
            "status": points::status_for(60, 90),
            "entry": LedgerEntry {
                id: 3,
                at_ms: 1_773_000_000_000,
                delta: 20,
                reason_code: "focus_window".into(),
                ref_key: "focus30:run:12:win:0".into(),
            },
            "stages": [Stage::Seed, Stage::Sprout, Stage::SmallTree, Stage::ForestSpirit],
        });
        assert_eq!(actual, expected);
    }
}
