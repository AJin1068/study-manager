//! Character commands: candidates (may call the model), choosing, and the saved character.

use std::sync::Arc;

use tauri::{AppHandle, State};

use super::{lock_db, AppState};
use crate::character::service::{year_of, CharacterService};
use crate::character::{self, CandidateSet, CharacterView, ChooseOutcome, ChooseRequest, SetupInput};
use crate::clock::now_ms;
use crate::error::{AppError, AppResult};

type Service<'a> = State<'a, Arc<CharacterService>>;

#[tauri::command]
pub fn get_character(app: AppHandle, state: State<'_, AppState>) -> AppResult<Option<CharacterView>> {
    let view = character::view(&*lock_db(&state.db)?)?;
    character::refresh_tray(&app, &state.db);
    Ok(view)
}

/// Rolls candidates for `seed` (the UI counts up for "다시 뽑기"). Saves nothing.
#[tauri::command]
pub async fn roll_character_candidates(service: Service<'_>, input: SetupInput, seed: u32) -> AppResult<CandidateSet> {
    let service = Arc::clone(&service);
    tauri::async_runtime::spawn_blocking(move || service.candidates(&input, seed))
        .await
        .map_err(|_| AppError::Internal("the candidate job did not finish".into()))?
}

#[tauri::command]
pub fn choose_character(app: AppHandle, state: State<'_, AppState>, request: ChooseRequest) -> AppResult<ChooseOutcome> {
    let now = now_ms()?;
    let outcome = character::choose(&mut *lock_db(&state.db)?, &request, now, year_of(now))?;
    character::refresh_tray(&app, &state.db);
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crate::character::{CandidateSet, CandidateView, CharacterView, ChooseOutcome, Equipped, Look, LookLabels, NameSource};
    use crate::points::rewards::Stage;

    fn look() -> Look {
        Look { species: "fox".into(), palette: "honey".into(), expression: "smile".into(), prop: "book".into() }
    }

    fn labels() -> LookLabels {
        LookLabels { species: "여우".into(), palette: "꿀색".into(), expression: "미소".into(), prop: "책".into() }
    }

    fn view(stage: Stage, label: &str, level: i64, equipped: Equipped) -> CharacterView {
        CharacterView {
            name: "도토리".into(),
            blurb: "함께 공부해요.".into(),
            look: look(),
            labels: labels(),
            stage,
            stage_label: label.into(),
            level,
            equipped,
            svg: "<svg/>".into(),
            chosen_at_ms: 1_800_000_000_000,
        }
    }

    #[test]
    fn character_wire_shapes_match_the_shared_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/api/fixtures/character-contract.json");
        let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let worn = Equipped { hat: Some("hat_beanie".into()), top: Some("top_vest".into()), accessory: None, hand: None };
        assert_eq!(serde_json::to_value(view(Stage::Sprout, "새싹", 3, worn)).expect("view"), fixture["view"]);
        let set = CandidateSet {
            seed: 2,
            candidates: vec![CandidateView {
                look: Look { species: "owl".into(), palette: "sand".into(), expression: "calm".into(), prop: "flower".into() },
                labels: LookLabels { species: "부엉이".into(), palette: "모래색".into(), expression: "차분".into(), prop: "꽃".into() },
                starter_top: "top_vest".into(),
                name: "보리".into(),
                blurb: "손에는 꽃, 마음에는 차분함. 같이 천천히 공부해요.".into(),
                name_source: NameSource::Builtin,
                svg: "<svg/>".into(),
            }],
            fallback_reason: Some("llm_disabled".into()),
        };
        assert_eq!(serde_json::to_value(set).expect("set"), fixture["candidateSet"]);
        let outcome = ChooseOutcome {
            character: view(Stage::Seed, "씨앗", 1, Equipped { top: Some("top_vest".into()), ..Equipped::default() }),
            tone_applied: Some("gentle".into()),
        };
        assert_eq!(serde_json::to_value(outcome).expect("outcome"), fixture["outcome"]);
    }

    #[test]
    fn the_choose_request_reads_the_ui_shape_and_tolerates_skipped_answers() {
        let request: crate::character::ChooseRequest = serde_json::from_value(serde_json::json!({
            "look": { "species": "fox", "palette": "honey", "expression": "smile", "prop": "book" },
            "starterTop": "top_vest",
            "name": "도토리",
            "blurb": "함께 공부해요.",
            "input": { "mbti": null, "birthYear": 1999 }
        }))
        .expect("request");
        assert_eq!((request.input.mbti, request.input.birth_year, request.input.gender), (None, Some(1999), None));
        let bare: crate::character::SetupInput = serde_json::from_value(serde_json::json!({})).expect("empty input");
        assert!(bare.mbti.is_none() && bare.birth_year.is_none() && bare.gender.is_none());
    }
}
