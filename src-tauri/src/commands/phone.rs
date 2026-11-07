//! Phone pairing commands. Calls that reach the server block, so they run on the blocking pool
//! with the database lock released.

use std::sync::Arc;

use tauri::State;

use crate::error::{AppError, AppResult};
use crate::phone::model::{PairStart, PairStatusView, PhoneAck};
use crate::phone::pairing::PairingService;
use crate::phone::publisher::{Diagnostics, PhoneLinkView};

type Service<'a> = State<'a, Arc<PairingService>>;

async fn blocking<T: Send + 'static>(
    service: &Service<'_>,
    work: impl FnOnce(&PairingService) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    let service = Arc::clone(service);
    tauri::async_runtime::spawn_blocking(move || work(&service))
        .await
        .map_err(|_| AppError::Internal("the phone job did not finish".into()))?
}

/// A new one-time code and its QR code.
#[tauri::command]
pub async fn phone_pair_start(service: Service<'_>) -> AppResult<PairStart> {
    blocking(&service, |s| s.start()).await
}

#[tauri::command]
pub async fn phone_pair_status(service: Service<'_>) -> AppResult<PairStatusView> {
    blocking(&service, |s| s.status()).await
}

/// Ends the pairing; refused (`run_locked`) while a study run holds the lock.
#[tauri::command]
pub async fn phone_pair_revoke(service: Service<'_>) -> AppResult<PhoneAck> {
    blocking(&service, |s| s.revoke()).await
}

/// The link while a lock is wanted: whether the lock reached the server, open gaps, the phone's
/// last beat. Read from the publisher's own bookkeeping; it makes no network call.
#[tauri::command]
pub fn get_phone_link(diagnostics: State<'_, Arc<Diagnostics>>) -> PhoneLinkView {
    diagnostics.view()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::phone::model::PhoneCode;

    fn fixture() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../src/api/fixtures/phone-contract.json"
        ))
        .expect("fixture")
    }

    fn value<T: serde::Serialize>(v: &T) -> serde_json::Value {
        serde_json::to_value(v).expect("json")
    }

    #[test]
    fn phone_wire_shapes_match_the_shared_fixture() {
        let f = fixture();
        let start = PairStart {
            code: PhoneCode::Ok,
            display_code: Some("ABCDE-FGHJK".into()),
            expires_at_ms: Some(1_793_975_700_000),
            qr_svg: Some("<svg></svg>".into()),
        };
        assert_eq!(value(&start), f["start"]);
        assert_eq!(
            value(&PairStart {
                code: PhoneCode::RateLimited,
                display_code: None,
                expires_at_ms: None,
                qr_svg: None
            }),
            f["refusedStart"]
        );
        let status = PairStatusView {
            paired: true,
            paired_at_ms: Some(1_793_975_000_000),
            last_beat_ms: Some(1_793_975_100_000),
            service_enabled: Some(true),
            online: true,
        };
        assert_eq!(value(&status), f["status"]);
        assert_eq!(
            value(&PairStatusView {
                paired: false,
                paired_at_ms: None,
                last_beat_ms: None,
                service_enabled: None,
                online: false
            }),
            f["unpaired"]
        );
        assert_eq!(
            value(&PhoneAck {
                code: PhoneCode::RunLocked
            }),
            f["ack"]
        );
        assert_eq!(
            json!(PhoneCode::PhoneAlreadyPaired.as_str()),
            json!("phone_already_paired")
        );
    }

    #[test]
    fn phone_link_wire_shape_matches_the_shared_fixture() {
        use crate::phone::publisher::{OpenGapView, PhoneLinkView, PublishedView};
        let f = fixture();
        let link = PhoneLinkView {
            paired: true,
            want_locked: true,
            published: Some(PublishedView {
                seq: 4,
                state: "locked",
                reason: None,
                at_ms: 1_793_975_400_000,
            }),
            publish_failing: false,
            last_error: None,
            open_gaps: vec![OpenGapView {
                kind: "unreachable",
                since_ms: 1_793_975_100_000,
            }],
            last_beat_ms: Some(1_793_975_100_000),
            service_enabled: Some(true),
        };
        assert_eq!(value(&link), f["link"]);
        assert_eq!(value(&PhoneLinkView::default()), f["linkIdle"]);
    }
}
