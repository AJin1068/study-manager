//! Secret storage port. Values live in the OS keychain (`KeyringSecretStore`); when the keychain
//! cannot be used the app falls back to memory for the session (`SessionOnly`) and says so with a
//! visible Degraded state. Secrets never go to the settings table, `setting_history`, logs, or
//! `Debug` output.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// Per-name value cap. Windows Credential Manager blobs hold roughly 2.5 KB, and a refresh token
/// or API key is far smaller, so a bigger value is a mistake (or a whole session JSON).
pub const MAX_SECRET_BYTES: usize = 2048;

const SERVICE: &str = "study-manager";

/// Secrets the app keeps. Public settings (endpoint, anon key) are not secrets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretName {
    AnthropicApiKey,
    SupabaseRefreshToken,
    NtfyTopic,
}

impl SecretName {
    pub const ALL: [SecretName; 3] = [
        SecretName::AnthropicApiKey,
        SecretName::SupabaseRefreshToken,
        SecretName::NtfyTopic,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SecretName::AnthropicApiKey => "anthropic_api_key",
            SecretName::SupabaseRefreshToken => "supabase_refresh_token",
            SecretName::NtfyTopic => "ntfy_topic",
        }
    }

    /// May the user type this value in the integrations screen? The refresh token only ever comes
    /// from a login.
    pub fn user_enterable(self) -> bool {
        !matches!(self, SecretName::SupabaseRefreshToken)
    }
}

/// A secret value. `Debug` is redacted; reading the text takes an explicit call.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(..)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("secret value is empty or contains control characters")]
    Invalid,
    #[error("secret value is larger than {MAX_SECRET_BYTES} bytes")]
    TooLarge,
    /// The backing store failed. The cause text is dropped on purpose: platform errors may echo
    /// account names.
    #[error("secret store unavailable")]
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SecretStoreKind {
    Keyring,
    /// Process memory only; nothing persists. Used by tests and by mock mode.
    InMemory,
    /// Keychain unusable: memory only, and the UI says the key vanishes on restart.
    SessionOnly,
}

pub trait SecretStore: Send + Sync {
    fn kind(&self) -> SecretStoreKind;
    fn get(&self, name: SecretName) -> Result<Option<SecretValue>, SecretError>;
    fn set(&self, name: SecretName, value: &str) -> Result<(), SecretError>;
    /// Deleting a missing secret succeeds.
    fn delete(&self, name: SecretName) -> Result<(), SecretError>;
}

pub fn validate_value(value: &str) -> Result<(), SecretError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(SecretError::Invalid);
    }
    if value.len() > MAX_SECRET_BYTES {
        return Err(SecretError::TooLarge);
    }
    Ok(())
}

/// Memory-backed store: the test/mock adapter (`InMemory`) and the keychain fallback (`SessionOnly`).
pub struct MemorySecretStore {
    kind: SecretStoreKind,
    values: Mutex<HashMap<SecretName, String>>,
}

impl MemorySecretStore {
    pub fn in_memory() -> Self {
        Self {
            kind: SecretStoreKind::InMemory,
            values: Mutex::new(HashMap::new()),
        }
    }

    pub fn session_only() -> Self {
        Self {
            kind: SecretStoreKind::SessionOnly,
            values: Mutex::new(HashMap::new()),
        }
    }
}

impl SecretStore for MemorySecretStore {
    fn kind(&self) -> SecretStoreKind {
        self.kind
    }

    fn get(&self, name: SecretName) -> Result<Option<SecretValue>, SecretError> {
        let values = self.values.lock().map_err(|_| SecretError::Unavailable)?;
        Ok(values.get(&name).cloned().map(SecretValue))
    }

    fn set(&self, name: SecretName, value: &str) -> Result<(), SecretError> {
        validate_value(value)?;
        self.values
            .lock()
            .map_err(|_| SecretError::Unavailable)?
            .insert(name, value.to_string());
        Ok(())
    }

    fn delete(&self, name: SecretName) -> Result<(), SecretError> {
        self.values
            .lock()
            .map_err(|_| SecretError::Unavailable)?
            .remove(&name);
        Ok(())
    }
}

/// OS keychain (macOS Keychain, Windows Credential Manager) through the `keyring` crate. A fresh
/// `Entry` per call keeps the adapter stateless.
#[derive(Default)]
pub struct KeyringSecretStore;

impl KeyringSecretStore {
    fn entry(name: SecretName) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(SERVICE, name.as_str()).map_err(|_| SecretError::Unavailable)
    }
}

impl SecretStore for KeyringSecretStore {
    fn kind(&self) -> SecretStoreKind {
        SecretStoreKind::Keyring
    }

    fn get(&self, name: SecretName) -> Result<Option<SecretValue>, SecretError> {
        match Self::entry(name)?.get_password() {
            Ok(value) => Ok(Some(SecretValue(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(SecretError::Unavailable),
        }
    }

    fn set(&self, name: SecretName, value: &str) -> Result<(), SecretError> {
        validate_value(value)?;
        Self::entry(name)?
            .set_password(value)
            .map_err(|_| SecretError::Unavailable)
    }

    fn delete(&self, name: SecretName) -> Result<(), SecretError> {
        match Self::entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(SecretError::Unavailable),
        }
    }
}

/// Health code shown when the keychain was unusable at startup.
pub const DEGRADED_SESSION_ONLY: &str = "secret_store_session_only";

/// The store the app runs with, and whether that is a degraded choice.
pub struct SelectedStore {
    pub store: Arc<dyn SecretStore>,
    pub degraded: Option<&'static str>,
}

/// Write/read/delete round trip through the real backend path. It borrows the NtfyTopic slot only
/// while that slot is empty; when a topic already exists, a successful read proves the backend works
/// and nothing is written (a real secret is never overwritten or deleted by the check).
fn probe(primary: &dyn SecretStore) -> bool {
    let slot = SecretName::NtfyTopic;
    match primary.get(slot) {
        Ok(Some(_)) => return true,
        Ok(None) => {}
        Err(_) => return false,
    }
    let marker = "self-check-value";
    if primary.set(slot, marker).is_err() {
        return false;
    }
    let read_back = matches!(primary.get(slot), Ok(Some(v)) if v.expose() == marker);
    let deleted = primary.delete(slot).is_ok() && matches!(primary.get(slot), Ok(None));
    read_back && deleted
}

/// Startup self-check. `keyring` falls back to a throwaway mock store when no native backend is
/// compiled in, and a locked or missing keychain fails at the first write, so the check writes,
/// reads back, and deletes. Any failure selects `SessionOnly` with a Degraded code (no silent
/// fallback).
pub fn select_secret_store(primary: Arc<dyn SecretStore>) -> SelectedStore {
    if probe(primary.as_ref()) {
        return SelectedStore {
            store: primary,
            degraded: None,
        };
    }
    tracing::warn!(
        event = "secret_store_degraded",
        code = DEGRADED_SESSION_ONLY
    );
    SelectedStore {
        store: Arc::new(MemorySecretStore::session_only()),
        degraded: Some(DEGRADED_SESSION_ONLY),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A store that fails in a chosen way, to drive the startup self-check.
    pub(crate) struct FaultyStore {
        pub fail_set: bool,
        pub forget_on_read: bool,
        inner: MemorySecretStore,
    }

    impl FaultyStore {
        pub fn new(fail_set: bool, forget_on_read: bool) -> Self {
            Self {
                fail_set,
                forget_on_read,
                inner: MemorySecretStore::in_memory(),
            }
        }
    }

    impl SecretStore for FaultyStore {
        fn kind(&self) -> SecretStoreKind {
            SecretStoreKind::Keyring
        }
        fn get(&self, name: SecretName) -> Result<Option<SecretValue>, SecretError> {
            if self.forget_on_read {
                return Ok(None);
            }
            self.inner.get(name)
        }
        fn set(&self, name: SecretName, value: &str) -> Result<(), SecretError> {
            if self.fail_set {
                return Err(SecretError::Unavailable);
            }
            self.inner.set(name, value)
        }
        fn delete(&self, name: SecretName) -> Result<(), SecretError> {
            self.inner.delete(name)
        }
    }

    #[test]
    fn memory_store_round_trips_and_delete_is_idempotent() {
        let store = MemorySecretStore::in_memory();
        assert_eq!(store.get(SecretName::AnthropicApiKey), Ok(None));
        store
            .set(SecretName::AnthropicApiKey, "sk-test-1")
            .expect("set");
        assert_eq!(
            store
                .get(SecretName::AnthropicApiKey)
                .expect("get")
                .expect("some")
                .expose(),
            "sk-test-1"
        );
        store.delete(SecretName::AnthropicApiKey).expect("delete");
        store
            .delete(SecretName::AnthropicApiKey)
            .expect("delete again");
        assert_eq!(store.get(SecretName::AnthropicApiKey), Ok(None));
    }

    #[test]
    fn values_are_size_and_shape_checked() {
        let store = MemorySecretStore::in_memory();
        assert_eq!(
            store.set(SecretName::NtfyTopic, ""),
            Err(SecretError::Invalid)
        );
        assert_eq!(
            store.set(SecretName::NtfyTopic, "a\nb"),
            Err(SecretError::Invalid)
        );
        assert_eq!(
            store.set(SecretName::NtfyTopic, &"x".repeat(MAX_SECRET_BYTES + 1)),
            Err(SecretError::TooLarge)
        );
        assert!(
            store
                .set(SecretName::NtfyTopic, &"x".repeat(MAX_SECRET_BYTES))
                .is_ok()
        );
    }

    #[test]
    fn healthy_primary_is_kept_and_probe_leaves_nothing_behind() {
        let primary: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::in_memory());
        let selected = select_secret_store(Arc::clone(&primary));
        assert_eq!(selected.degraded, None);
        assert_eq!(selected.store.kind(), SecretStoreKind::InMemory);
        assert_eq!(primary.get(SecretName::NtfyTopic), Ok(None));
    }

    #[test]
    fn probe_does_not_disturb_an_existing_secret() {
        let primary: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::in_memory());
        primary
            .set(SecretName::NtfyTopic, "real-topic")
            .expect("set");
        let selected = select_secret_store(Arc::clone(&primary));
        assert_eq!(selected.degraded, None);
        assert_eq!(
            primary
                .get(SecretName::NtfyTopic)
                .expect("get")
                .expect("some")
                .expose(),
            "real-topic"
        );
    }

    #[test]
    fn failed_write_selects_session_only_with_a_degraded_code() {
        let selected = select_secret_store(Arc::new(FaultyStore::new(true, false)));
        assert_eq!(selected.degraded, Some(DEGRADED_SESSION_ONLY));
        assert_eq!(selected.store.kind(), SecretStoreKind::SessionOnly);
        selected
            .store
            .set(SecretName::AnthropicApiKey, "k")
            .expect("session store works");
    }

    #[test]
    fn a_store_that_forgets_what_it_wrote_is_degraded_too() {
        // This is what keyring's silent mock backend looks like when no native feature is built.
        let selected = select_secret_store(Arc::new(FaultyStore::new(false, true)));
        assert_eq!(selected.degraded, Some(DEGRADED_SESSION_ONLY));
    }

    #[test]
    fn secret_value_debug_is_redacted() {
        assert_eq!(
            format!("{:?}", SecretValue::new("sk-live-abc")),
            "SecretValue(..)"
        );
    }

    #[test]
    fn only_non_login_secrets_are_user_enterable() {
        assert!(SecretName::AnthropicApiKey.user_enterable());
        assert!(SecretName::NtfyTopic.user_enterable());
        assert!(!SecretName::SupabaseRefreshToken.user_enterable());
    }
}
