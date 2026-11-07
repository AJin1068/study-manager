//! The two ports the workers talk through, and the adapter that serves both mock and live mode.
//!
//! `RestSync` speaks to a `SupabaseRest`. In mock mode that adapter sits on the in-memory
//! `FakeBackend`, in live mode on the real transport, so the same code path is exercised either
//! way. Tests substitute their own `PlanMirror` / `PresencePort` to inject failures.

use std::sync::Arc;

use serde_json::Value;

use super::snapshot::Snapshot;
use crate::plan::PlanHistoryEntry;
use crate::remote::payload::{
    DeviceRegistration, NotifyChannelRow, PlanHistoryBatch, PlanHistoryRow, PresenceBeat,
    SnapshotArgs,
};
use crate::remote::supabase::{RemoteError, SupabaseRest};

/// Why a push or beat did not happen. `code()` is the only text that reaches the UI or a log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SyncError {
    #[error("{0}")]
    Remote(RemoteError),
    /// The server answered but refused: `device_not_owned`, `invalid_input`.
    #[error("{0}")]
    Rejected(&'static str),
}

pub const DEVICE_NOT_OWNED: &str = "device_not_owned";

impl SyncError {
    pub fn code(self) -> &'static str {
        match self {
            SyncError::Remote(err) => err.code(),
            SyncError::Rejected(code) => code,
        }
    }

    /// The server does not know this device (signed in again, or the registration never landed).
    pub fn is_device_missing(self) -> bool {
        self == SyncError::Rejected(DEVICE_NOT_OWNED)
    }
}

/// This installation as the server's `devices` table knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub id: String,
    pub app_version: String,
    pub platform: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotOutcome {
    Replaced,
    /// The server already holds this exact snapshot.
    Unchanged,
}

pub trait PlanMirror: Send + Sync {
    /// Idempotent: registering a known device is a no-op.
    fn register_device(&self, device: &DeviceIdentity) -> Result<(), SyncError>;
    fn push_snapshot(
        &self,
        device_id: &str,
        snapshot: &Snapshot,
    ) -> Result<SnapshotOutcome, SyncError>;
    /// Idempotent per `(device, local id)`: a repeated batch adds nothing.
    fn push_history(&self, device_id: &str, rows: &[PlanHistoryEntry]) -> Result<(), SyncError>;
    fn push_notify_channel(&self, row: &NotifyChannelRow) -> Result<(), SyncError>;
}

pub trait PresencePort: Send + Sync {
    fn beat(&self, beat: &PresenceBeat) -> Result<(), SyncError>;
}

pub struct RestSync {
    rest: Arc<SupabaseRest>,
}

impl RestSync {
    pub fn new(rest: Arc<SupabaseRest>) -> Self {
        Self { rest }
    }
}

fn remote(err: RemoteError) -> SyncError {
    SyncError::Remote(err)
}

/// RPCs return their outcome as a JSON string (see `fixtures/sync/rpc-contract.json`).
fn rpc_code(value: &Value) -> Result<&str, SyncError> {
    value.as_str().ok_or(SyncError::Remote(RemoteError::Decode))
}

impl PlanMirror for RestSync {
    fn register_device(&self, device: &DeviceIdentity) -> Result<(), SyncError> {
        let registration = DeviceRegistration {
            id: device.id.clone(),
            app_version: device.app_version.clone(),
            platform: device.platform,
        };
        self.rest
            .insert("device_register", "devices", &registration, true)
            .map_err(remote)
    }

    fn push_snapshot(
        &self,
        device_id: &str,
        snapshot: &Snapshot,
    ) -> Result<SnapshotOutcome, SyncError> {
        let args = SnapshotArgs {
            device_id: device_id.to_string(),
            snapshot_hash: snapshot.hash.clone(),
            generated_at_ms: snapshot.generated_at_ms,
            horizon_days: snapshot.horizon_days,
            occurrences: snapshot.occurrences.clone(),
        };
        let answer = self
            .rest
            .rpc("rpc_replace_plan_snapshot", "replace_plan_snapshot", &args)
            .map_err(remote)?;
        match rpc_code(&answer)? {
            "ok" => Ok(SnapshotOutcome::Replaced),
            "unchanged" => Ok(SnapshotOutcome::Unchanged),
            "device_not_owned" => Err(SyncError::Rejected(DEVICE_NOT_OWNED)),
            "invalid_input" => Err(SyncError::Rejected("invalid_input")),
            _ => Err(SyncError::Remote(RemoteError::Decode)),
        }
    }

    fn push_history(&self, device_id: &str, rows: &[PlanHistoryEntry]) -> Result<(), SyncError> {
        let batch = PlanHistoryBatch(
            rows.iter()
                .map(|entry| PlanHistoryRow {
                    device_id: device_id.to_string(),
                    local_id: entry.id,
                    plan_local_id: entry.plan_id,
                    at_ms: entry.at_ms,
                    action: entry.action.clone(),
                    reason_code: entry.reason_code.clone(),
                    reason_text: entry.reason_text.clone(),
                    before_json: entry.before_json.clone(),
                    after_json: entry.after_json.clone(),
                    outcome: entry.outcome.clone(),
                    batch_id: entry.batch_id.clone(),
                })
                .collect(),
        );
        // ignore-duplicates: a batch that was half-delivered before a crash is simply sent again.
        self.rest
            .insert("insert_plan_history", "plan_history_mirror", &batch, true)
            .map_err(|err| match err {
                // The insert policy only admits rows for a device the caller owns, so a refusal means
                // the server no longer knows this device.
                RemoteError::Http { status: 403 } => SyncError::Rejected(DEVICE_NOT_OWNED),
                other => remote(other),
            })
    }

    fn push_notify_channel(&self, row: &NotifyChannelRow) -> Result<(), SyncError> {
        self.rest
            .upsert("upsert_notify_channel", "notify_channels", row)
            .map_err(remote)
    }
}

impl PresencePort for RestSync {
    fn beat(&self, beat: &PresenceBeat) -> Result<(), SyncError> {
        let answer = self
            .rest
            .rpc("rpc_heartbeat", "heartbeat", beat)
            .map_err(remote)?;
        match rpc_code(&answer)? {
            "ok" => Ok(()),
            "device_not_owned" => Err(SyncError::Rejected(DEVICE_NOT_OWNED)),
            _ => Err(SyncError::Remote(RemoteError::Decode)),
        }
    }
}
