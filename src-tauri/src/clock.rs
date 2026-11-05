use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{AppError, AppResult};

/// Current UTC time as epoch milliseconds. All stored timestamps use this unit.
/// A clock before 1970 is an error, not a value: writing a fabricated 0 into append-only
/// tables would be uncorrectable later.
pub fn now_ms() -> AppResult<i64> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| AppError::Internal(format!("system clock is before the unix epoch: {e}")))?;
    i64::try_from(elapsed.as_millis())
        .map_err(|_| AppError::Internal("system clock is out of range for i64 milliseconds".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_after_2020_and_positive() {
        let now = now_ms().expect("clock");
        assert!(now > 1_577_836_800_000, "expected a post-2020 timestamp, got {now}");
    }
}
