//! Release challenge: the user types a long sentence the Rust side chose, and only an exact match
//! applies the release (`emergency_pass`, `skip_today`, `early_release`; effects in
//! `study_pass`). The challenge and the comparison live here, not in the UI (the UI only blocks
//! paste as a convenience, D-14). Nothing the user types is stored or logged: only the sentence
//! length, how long typing took, and the number of wrong attempts.
//!
//! It reaches the database through `study_pass` directly, so it works while the engine thread is
//! down; this is the emergency exit that was `abort_run` in stage 1-2.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::Mutex;

use chrono::TimeZone;
use rusqlite::Connection;
use serde::Serialize;

use crate::error::{AppError, AppResult};

use super::study_pass::{self, Grant, PassKind, Typing};

/// Each sentence is at least [`MIN_SENTENCE_CHARS`] characters (checked by a test).
const SENTENCES: &[&str] = &[
    "나는 지금 계획한 공부를 끝내지 않고 멈추려 한다. 이 선택이 오늘의 약속을 깨뜨린다는 것을 알고 있으며, 그래도 중단하는 이유가 정당하다고 스스로 확인한다.",
    "지금 공부를 그만두면 오늘 세운 계획은 중단 기록으로 남는다. 나는 이 기록을 받아들이고, 정말 필요한 때에만 이 방법을 쓰겠다고 다시 한 번 다짐한다.",
    "잠깐 쉬고 싶은 마음과 정말 멈춰야 하는 사정은 서로 다르다. 나는 지금 둘 중 어느 쪽인지 차분히 생각해 보았고, 멈춰야 한다고 판단하여 이 문장을 직접 입력한다.",
    "계획은 나 자신과 한 약속이다. 약속을 중간에 끊는 데에는 번거로운 절차가 따르는 것이 마땅하며, 나는 그 절차를 끝까지 따라 이 문장을 한 글자씩 정확하게 입력한다.",
    "이 문장을 모두 입력하면 내가 고른 해제 절차가 적용되고 그 사용은 기록과 통계에 그대로 남는다. 이후의 계획은 정해진 시각에 다시 자동으로 시작될 수 있다는 점도 기억하겠다.",
    "급한 일이 생겨 공부를 멈추어야 한다면 지금 멈추는 것이 맞다. 다만 중단은 통계에 그대로 남으므로, 나는 이 사실을 알면서 천천히 이 긴 문장을 직접 타이핑한다.",
    "오늘 하루의 공부를 지켜 내는 일은 작은 선택이 쌓여 만들어진다. 그럼에도 오늘은 멈추기로 했으니, 내일 다시 계획대로 돌아오겠다는 마음으로 이 문장을 입력한다.",
    "붙여넣기 없이 손으로 직접 입력해야 이 문장은 비로소 의미를 갖는다. 나는 서두르지 않고 모든 글자를 맞추어 입력하며 공부 중단을 스스로 책임지겠다.",
];

#[cfg(test)]
pub const MIN_SENTENCE_CHARS: usize = 80;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseChallenge {
    pub challenge_id: u64,
    pub kind: PassKind,
    pub sentence: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum ReleaseOutcome {
    /// The release took effect and was recorded. `expires_at_ms` is set for an emergency pass.
    #[serde(rename_all = "camelCase")]
    Released { kind: PassKind, expires_at_ms: Option<i64> },
    #[serde(rename_all = "camelCase")]
    Mismatch { attempts: u32 },
    /// No challenge was issued or it was replaced; ask for a new one.
    NoChallenge,
    NothingToRelease,
    /// The daily emergency-pass limit is spent.
    #[serde(rename_all = "camelCase")]
    LimitReached { limit: i64 },
    /// An emergency pass is already running.
    #[serde(rename_all = "camelCase")]
    AlreadyActive { expires_at_ms: i64 },
}

struct Pending {
    id: u64,
    kind: PassKind,
    sentence: &'static str,
    issued_at_ms: i64,
}

#[derive(Default)]
struct DeskState {
    pending: Option<Pending>,
    next_id: u64,
    wrong_attempts: u32,
}

/// Holds the one outstanding challenge. A new request replaces the old one.
#[derive(Default)]
pub struct ReleaseDesk {
    state: Mutex<DeskState>,
}

fn lock(desk: &ReleaseDesk) -> std::sync::MutexGuard<'_, DeskState> {
    // The state is plain data, so a poisoned guard is still consistent.
    desk.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn random_index(len: usize) -> usize {
    let seed = RandomState::new().build_hasher().finish();
    usize::try_from(seed % u64::try_from(len).unwrap_or(1)).unwrap_or(0)
}

fn refusal_message(refusal: Grant) -> String {
    match refusal {
        Grant::NothingToRelease => "there is nothing to release".into(),
        Grant::LimitReached { limit } => format!("the daily limit of {limit} emergency pass(es) is spent"),
        Grant::AlreadyActive { .. } => "an emergency pass is already running".into(),
        Grant::Granted { .. } => "granted".into(),
    }
}

impl ReleaseDesk {
    /// Issues a challenge for `kind`. Refused up front when the release could not be granted, so
    /// the user is not made to type a long sentence for nothing.
    pub fn begin<Tz: TimeZone>(&self, conn: &Connection, tz: &Tz, kind: PassKind, now_ms: i64) -> AppResult<ReleaseChallenge> {
        if let Some(refusal) = study_pass::refusal(conn, tz, kind, now_ms)? {
            return Err(AppError::Invalid(refusal_message(refusal)));
        }
        let mut state = lock(self);
        state.next_id += 1;
        state.wrong_attempts = 0;
        let sentence = SENTENCES[random_index(SENTENCES.len())];
        let id = state.next_id;
        state.pending = Some(Pending { id, kind, sentence, issued_at_ms: now_ms });
        Ok(ReleaseChallenge { challenge_id: id, kind, sentence: sentence.to_string() })
    }

    /// Applies the release only if `typed` equals the issued sentence exactly.
    pub fn submit<Tz: TimeZone>(
        &self,
        conn: &mut Connection,
        tz: &Tz,
        challenge_id: u64,
        typed: &str,
        at_ms: i64,
    ) -> AppResult<ReleaseOutcome> {
        let mut state = lock(self);
        let Some(pending) = state.pending.as_ref().filter(|p| p.id == challenge_id) else {
            return Ok(ReleaseOutcome::NoChallenge);
        };
        let (kind, sentence, issued_at_ms) = (pending.kind, pending.sentence, pending.issued_at_ms);
        if typed != sentence {
            state.wrong_attempts += 1;
            let attempts = state.wrong_attempts;
            tracing::info!(event = "release_rejected", kind = kind.as_str(), attempts);
            return Ok(ReleaseOutcome::Mismatch { attempts });
        }
        state.pending = None;
        drop(state);
        let typing = Typing {
            challenge_len: i64::try_from(sentence.chars().count()).unwrap_or(i64::MAX),
            typing_ms: (at_ms - issued_at_ms).max(0),
        };
        Ok(match study_pass::grant(conn, tz, kind, at_ms, typing)? {
            Grant::Granted { expires_at_ms } => ReleaseOutcome::Released { kind, expires_at_ms },
            Grant::NothingToRelease => ReleaseOutcome::NothingToRelease,
            Grant::LimitReached { limit } => ReleaseOutcome::LimitReached { limit },
            Grant::AlreadyActive { expires_at_ms } => ReleaseOutcome::AlreadyActive { expires_at_ms },
        })
    }
}

#[cfg(test)]
pub fn sentences() -> &'static [&'static str] {
    SENTENCES
}
