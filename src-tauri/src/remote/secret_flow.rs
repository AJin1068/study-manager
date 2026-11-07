//! Secret changes under the settings lock. The value goes to the `SecretStore`; the history row
//! records only that a secret was set or cleared (`secret_set` / `secret_cleared`, before/after
//! as `true`/`false`), never the value or any hash of it.
//!
//! During a planned run, entering a secret is refused and recorded (`rejected_locked`). Removing
//! one is allowed and recorded as `applied_privacy_tighten`: it narrows what can leave the device
//! and cannot weaken blocking.

use rusqlite::{Connection, TransactionBehavior};

use super::secrets::{SecretError, SecretName, SecretStore};
use crate::error::{AppError, AppResult};
use crate::settings::{Outcome, Reason, UpdateOutcome};
use crate::{store, study};

fn secret_error(err: SecretError) -> AppError {
    match err {
        // Never echo the value: the text may be a key.
        SecretError::Invalid | SecretError::TooLarge => {
            AppError::Invalid("the secret is empty, too long, or has control characters".into())
        }
        SecretError::Unavailable => AppError::Remote("secret_store_unavailable".into()),
    }
}

fn flag(set: bool) -> &'static str {
    if set { "true" } else { "false" }
}

fn is_set(store: &dyn SecretStore, name: SecretName) -> AppResult<bool> {
    Ok(store.get(name).map_err(secret_error)?.is_some())
}

/// Stores a user-entered secret unless a planned run holds the lock.
pub fn save_secret(
    conn: &mut Connection,
    secrets: &dyn SecretStore,
    name: SecretName,
    value: &str,
    at_ms: i64,
) -> AppResult<UpdateOutcome> {
    if !name.user_enterable() {
        return Err(AppError::Invalid(format!(
            "{} cannot be entered by hand",
            name.as_str()
        )));
    }
    super::secrets::validate_value(value).map_err(secret_error)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let before = flag(is_set(secrets, name)?);
    if study::lock_held(&tx)? {
        store::append_history(
            &tx,
            at_ms,
            name.as_str(),
            Some(before),
            "true",
            Reason::User,
            Outcome::RejectedLocked,
        )?;
        tx.commit()?;
        return Ok(UpdateOutcome::RejectedLocked {
            key: name.as_str().to_string(),
        });
    }
    secrets.set(name, value).map_err(secret_error)?;
    store::append_history(
        &tx,
        at_ms,
        name.as_str(),
        Some(before),
        "true",
        Reason::SecretSet,
        Outcome::Applied,
    )?;
    tx.commit()?;
    Ok(UpdateOutcome::Applied {
        key: name.as_str().to_string(),
    })
}

/// Removes a secret. Allowed during a run (privacy tightening).
pub fn clear_secret(
    conn: &mut Connection,
    secrets: &dyn SecretStore,
    name: SecretName,
    at_ms: i64,
) -> AppResult<UpdateOutcome> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let before = flag(is_set(secrets, name)?);
    let outcome = if study::lock_held(&tx)? {
        Outcome::AppliedPrivacyTighten
    } else {
        Outcome::Applied
    };
    secrets.delete(name).map_err(secret_error)?;
    store::append_history(
        &tx,
        at_ms,
        name.as_str(),
        Some(before),
        "false",
        Reason::SecretCleared,
        outcome,
    )?;
    tx.commit()?;
    Ok(UpdateOutcome::Applied {
        key: name.as_str().to_string(),
    })
}

/// Sign-in stores a refresh token the user did not type, but it still widens what leaves the
/// device, so it follows the same lock rule. Returns `true` (and records the refusal) when the
/// lock is held; the caller must not start the sign-in then.
pub fn refuse_entry_if_locked(
    conn: &mut Connection,
    name: SecretName,
    at_ms: i64,
) -> AppResult<bool> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !study::lock_held(&tx)? {
        return Ok(false);
    }
    store::append_history(
        &tx,
        at_ms,
        name.as_str(),
        None,
        "true",
        Reason::User,
        Outcome::RejectedLocked,
    )?;
    tx.commit()?;
    Ok(true)
}

/// History row for a secret the app stored on its own (the refresh token after a sign-in).
pub fn record_set(conn: &Connection, name: SecretName, at_ms: i64) -> AppResult<()> {
    store::append_history(
        conn,
        at_ms,
        name.as_str(),
        Some("false"),
        "true",
        Reason::SecretSet,
        Outcome::Applied,
    )
}

/// History row for a secret the app removed (sign-out). `tighten` is true when it happened under
/// a held lock.
pub fn record_cleared(
    conn: &Connection,
    name: SecretName,
    tighten: bool,
    at_ms: i64,
) -> AppResult<()> {
    let outcome = if tighten {
        Outcome::AppliedPrivacyTighten
    } else {
        Outcome::Applied
    };
    store::append_history(
        conn,
        at_ms,
        name.as_str(),
        Some("true"),
        "false",
        Reason::SecretCleared,
        outcome,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::remote::secrets::MemorySecretStore;
    use crate::settings::read_history;
    use crate::testutil::{capture_logs, set_lock_for_test};

    const SECRET: &str = "sk-ant-TOPSECRET-1234567890";

    fn dump(conn: &Connection) -> String {
        let mut out = String::new();
        for table in ["settings", "setting_history", "remote_events"] {
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM {table}"))
                .expect("prepare");
            let columns = stmt.column_count();
            let rows = stmt
                .query_map([], |row| {
                    (0..columns)
                        .map(|i| row.get_ref(i).map(|v| format!("{v:?}")))
                        .collect::<Result<Vec<_>, _>>()
                })
                .expect("query");
            for row in rows {
                out.push_str(&row.expect("row").join("|"));
                out.push('\n');
            }
        }
        out
    }

    #[test]
    fn saved_secret_value_appears_nowhere_in_the_database_or_logs() {
        let mut conn = open_in_memory().expect("db");
        let secrets = MemorySecretStore::in_memory();
        let logs = capture_logs(|| {
            let outcome = save_secret(&mut conn, &secrets, SecretName::AnthropicApiKey, SECRET, 5)
                .expect("save");
            assert_eq!(
                outcome,
                UpdateOutcome::Applied {
                    key: "anthropic_api_key".into()
                }
            );
        });
        assert!(!logs.contains("TOPSECRET"), "{logs}");
        assert!(!dump(&conn).contains("TOPSECRET"));
        assert!(!dump(&conn).contains("sk-ant"));
        assert_eq!(
            secrets
                .get(SecretName::AnthropicApiKey)
                .expect("get")
                .expect("some")
                .expose(),
            SECRET
        );
        let history = read_history(&conn, 5).expect("history");
        assert_eq!(
            (
                history[0].key.as_str(),
                history[0].before.as_deref(),
                history[0].after.as_str(),
                history[0].reason.as_str(),
                history[0].outcome.as_str()
            ),
            (
                "anthropic_api_key",
                Some("false"),
                "true",
                "secret_set",
                "applied"
            )
        );
    }

    #[test]
    fn key_entry_during_a_run_is_refused_recorded_and_changes_nothing() {
        let mut conn = open_in_memory().expect("db");
        let secrets = MemorySecretStore::in_memory();
        set_lock_for_test(&conn, true);
        let outcome = save_secret(&mut conn, &secrets, SecretName::AnthropicApiKey, SECRET, 5)
            .expect("outcome");
        assert_eq!(
            outcome,
            UpdateOutcome::RejectedLocked {
                key: "anthropic_api_key".into()
            }
        );
        assert_eq!(secrets.get(SecretName::AnthropicApiKey), Ok(None));
        let history = read_history(&conn, 5).expect("history");
        assert_eq!(
            (history[0].outcome.as_str(), history[0].reason.as_str()),
            ("rejected_locked", "user")
        );
        assert!(!dump(&conn).contains("TOPSECRET"));
    }

    #[test]
    fn clearing_during_a_run_is_a_recorded_privacy_tightening() {
        let mut conn = open_in_memory().expect("db");
        let secrets = MemorySecretStore::in_memory();
        save_secret(&mut conn, &secrets, SecretName::AnthropicApiKey, SECRET, 1).expect("save");
        set_lock_for_test(&conn, true);
        let outcome =
            clear_secret(&mut conn, &secrets, SecretName::AnthropicApiKey, 2).expect("clear");
        assert!(matches!(outcome, UpdateOutcome::Applied { .. }));
        assert_eq!(secrets.get(SecretName::AnthropicApiKey), Ok(None));
        let history = read_history(&conn, 1).expect("history");
        assert_eq!(
            (
                history[0].reason.as_str(),
                history[0].outcome.as_str(),
                history[0].after.as_str()
            ),
            ("secret_cleared", "applied_privacy_tighten", "false")
        );
    }

    #[test]
    fn invalid_values_error_without_echoing_and_record_nothing() {
        let mut conn = open_in_memory().expect("db");
        let secrets = MemorySecretStore::in_memory();
        let err = save_secret(
            &mut conn,
            &secrets,
            SecretName::NtfyTopic,
            "bad\nSECRETPART",
            1,
        )
        .expect_err("rejected");
        assert!(!err.to_string().contains("SECRETPART"));
        assert!(read_history(&conn, 5).expect("history").is_empty());
        assert!(
            save_secret(
                &mut conn,
                &secrets,
                SecretName::SupabaseRefreshToken,
                "x",
                1
            )
            .is_err(),
            "login-only secret"
        );
    }

    #[test]
    fn sign_in_refusal_during_a_run_is_recorded() {
        let mut conn = open_in_memory().expect("db");
        assert!(
            !refuse_entry_if_locked(&mut conn, SecretName::SupabaseRefreshToken, 1).expect("free")
        );
        set_lock_for_test(&conn, true);
        assert!(
            refuse_entry_if_locked(&mut conn, SecretName::SupabaseRefreshToken, 2).expect("locked")
        );
        let history = read_history(&conn, 5).expect("history");
        assert_eq!(
            (history[0].key.as_str(), history[0].outcome.as_str()),
            ("supabase_refresh_token", "rejected_locked")
        );
    }
}
