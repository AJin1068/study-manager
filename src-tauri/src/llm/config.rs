//! `llm.*` settings, stored through the same locked, history-recorded path as every other
//! setting. The API key is not here: it is the `anthropic_api_key` secret.
//!
//! Turning the LLM or the judge consent off narrows what leaves the device, so both stay allowed
//! during a planned run (privacy tightening). Turning them on, changing the model, or the budget
//! is locked like any setting.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::settings::{self, FreeShape, SettingSpec, SettingValue, UpdateOutcome, ValueKind};

const CATEGORY: &str = "llm";

/// Default model: a Haiku-class alias. Unverified against the live API (docs/stage-3 V3-1); the
/// name is a setting precisely so a wrong or retired id is fixed without a release.
pub const DEFAULT_MODEL: &str = "claude-3-5-haiku-latest";
pub const DEFAULT_DAILY_BUDGET: i64 = 200_000;

pub const ENABLED: SettingSpec = SettingSpec {
    name: "llm.enabled",
    category: CATEGORY,
    kind: ValueKind::Bool,
    default: "false",
    tighten_to: Some("false"),
};
pub const JUDGE_ENABLED: SettingSpec = SettingSpec {
    name: "llm.judge_enabled",
    category: CATEGORY,
    kind: ValueKind::Bool,
    default: "false",
    tighten_to: Some("false"),
};
pub const MODEL: SettingSpec = SettingSpec {
    name: "llm.model",
    category: CATEGORY,
    kind: ValueKind::Free {
        max_len: 80,
        shape: FreeShape::Token,
    },
    default: DEFAULT_MODEL,
    tighten_to: None,
};
/// Folder holding one sub-folder per subject. Empty means `~/StudyWiki`.
pub const WIKI_DIR: SettingSpec = SettingSpec {
    name: "llm.wiki_dir",
    category: CATEGORY,
    kind: ValueKind::Free {
        max_len: 400,
        shape: FreeShape::Path,
    },
    default: "",
    tighten_to: None,
};
pub const DAILY_BUDGET_TOKENS: SettingSpec = SettingSpec {
    name: "llm.daily_budget_tokens",
    category: CATEGORY,
    kind: ValueKind::Int {
        min: 1_000,
        max: 5_000_000,
    },
    default: "200000",
    tighten_to: None,
};

/// The settings the LLM card edits. API name = camelCase of the variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LlmSettingKey {
    Enabled,
    JudgeEnabled,
    Model,
    WikiDir,
    DailyBudgetTokens,
}

impl LlmSettingKey {
    pub fn spec(self) -> SettingSpec {
        match self {
            LlmSettingKey::Enabled => ENABLED,
            LlmSettingKey::JudgeEnabled => JUDGE_ENABLED,
            LlmSettingKey::Model => MODEL,
            LlmSettingKey::WikiDir => WIKI_DIR,
            LlmSettingKey::DailyBudgetTokens => DAILY_BUDGET_TOKENS,
        }
    }
}

pub fn update_setting(
    conn: &mut Connection,
    key: LlmSettingKey,
    value: SettingValue,
    at_ms: i64,
) -> AppResult<UpdateOutcome> {
    settings::apply_setting(conn, &key.spec(), &value, at_ms, || Ok(()))
}

fn wrong_type(spec: &SettingSpec, value: &SettingValue) -> AppError {
    AppError::Internal(format!("{} has the wrong type: {value:?}", spec.name))
}

fn read_bool(conn: &Connection, spec: &SettingSpec) -> AppResult<bool> {
    match settings::read_value(conn, spec)? {
        SettingValue::Bool(v) => Ok(v),
        other => Err(wrong_type(spec, &other)),
    }
}

fn read_text(conn: &Connection, spec: &SettingSpec) -> AppResult<String> {
    match settings::read_value(conn, spec)? {
        SettingValue::Text(v) => Ok(v),
        other => Err(wrong_type(spec, &other)),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfig {
    pub enabled: bool,
    pub judge_enabled: bool,
    pub model: String,
    /// As stored: empty means the default folder.
    pub wiki_dir: String,
    pub daily_budget_tokens: i64,
}

impl LlmConfig {
    pub fn load(conn: &Connection) -> AppResult<Self> {
        let budget = match settings::read_value(conn, &DAILY_BUDGET_TOKENS)? {
            SettingValue::Int(v) => v,
            other => return Err(wrong_type(&DAILY_BUDGET_TOKENS, &other)),
        };
        Ok(Self {
            enabled: read_bool(conn, &ENABLED)?,
            judge_enabled: read_bool(conn, &JUDGE_ENABLED)?,
            model: read_text(conn, &MODEL)?,
            wiki_dir: read_text(conn, &WIKI_DIR)?,
            daily_budget_tokens: budget,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::testutil::set_lock_for_test;

    #[test]
    fn defaults_are_conservative() {
        let conn = open_in_memory().expect("db");
        let config = LlmConfig::load(&conn).expect("load");
        assert_eq!(
            config,
            LlmConfig {
                enabled: false,
                judge_enabled: false,
                model: DEFAULT_MODEL.into(),
                wiki_dir: String::new(),
                daily_budget_tokens: 200_000
            }
        );
    }

    #[test]
    fn values_are_validated_and_errors_do_not_echo_the_text() {
        let mut conn = open_in_memory().expect("db");
        for (key, bad) in [
            (LlmSettingKey::Model, SettingValue::Text("has space".into())),
            (
                LlmSettingKey::WikiDir,
                SettingValue::Text("relative/dir".into()),
            ),
            (
                LlmSettingKey::WikiDir,
                SettingValue::Text("/home/a/../../etc".into()),
            ),
            (LlmSettingKey::DailyBudgetTokens, SettingValue::Int(10)),
            (
                LlmSettingKey::DailyBudgetTokens,
                SettingValue::Int(9_000_000),
            ),
            (LlmSettingKey::Enabled, SettingValue::Text("yes".into())),
        ] {
            assert!(update_setting(&mut conn, key, bad, 1).is_err(), "{key:?}");
        }
        // `Path::is_absolute` needs a drive letter on Windows, so the valid sample is per-OS.
        let good_dir = if cfg!(windows) {
            "C:\\Users\\a\\StudyWiki"
        } else {
            "/Users/a/StudyWiki"
        };
        let ok = update_setting(
            &mut conn,
            LlmSettingKey::WikiDir,
            SettingValue::Text(good_dir.into()),
            1,
        )
        .expect("ok");
        assert!(matches!(ok, UpdateOutcome::Applied { .. }));
        assert_eq!(LlmConfig::load(&conn).expect("load").wiki_dir, good_dir);
    }

    #[test]
    fn during_a_run_only_switching_off_is_allowed_and_recorded_as_a_tightening() {
        let mut conn = open_in_memory().expect("db");
        update_setting(
            &mut conn,
            LlmSettingKey::Enabled,
            SettingValue::Bool(true),
            1,
        )
        .expect("on");
        update_setting(
            &mut conn,
            LlmSettingKey::JudgeEnabled,
            SettingValue::Bool(true),
            1,
        )
        .expect("on");
        set_lock_for_test(&conn, true);

        let widen = update_setting(
            &mut conn,
            LlmSettingKey::DailyBudgetTokens,
            SettingValue::Int(300_000),
            2,
        )
        .expect("outcome");
        assert_eq!(
            widen,
            UpdateOutcome::RejectedLocked {
                key: "llm.daily_budget_tokens".into()
            }
        );
        let model = update_setting(
            &mut conn,
            LlmSettingKey::Model,
            SettingValue::Text("other".into()),
            2,
        )
        .expect("outcome");
        assert!(matches!(model, UpdateOutcome::RejectedLocked { .. }));
        for key in [LlmSettingKey::Enabled, LlmSettingKey::JudgeEnabled] {
            let off = update_setting(&mut conn, key, SettingValue::Bool(false), 3).expect("off");
            assert!(matches!(off, UpdateOutcome::Applied { .. }), "{key:?}");
        }
        assert!(!LlmConfig::load(&conn).expect("load").enabled);
        update_setting(
            &mut conn,
            LlmSettingKey::Enabled,
            SettingValue::Bool(true),
            4,
        )
        .expect("outcome");
        assert!(
            !LlmConfig::load(&conn).expect("load").enabled,
            "turning it back on is locked"
        );

        let history = settings::read_history(&conn, 20).expect("history");
        let outcomes: Vec<(&str, &str)> = history
            .iter()
            .map(|h| (h.key.as_str(), h.outcome.as_str()))
            .collect();
        assert!(outcomes.contains(&("llm.enabled", "applied_privacy_tighten")));
        assert!(outcomes.contains(&("llm.judge_enabled", "applied_privacy_tighten")));
        assert!(outcomes.contains(&("llm.enabled", "rejected_locked")));
    }
}
