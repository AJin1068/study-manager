//! Argument structs of the shared-wiki RPCs, reviewed like `remote::payload`. Each field name is
//! the RPC parameter name. A page body can only be built from a page the user marked as shared:
//! `service` is the one caller. `Debug` hides every value.

use serde::Serialize;

use crate::remote::payload::egress_payload;

#[derive(Serialize)]
pub struct PublishSharedPageArgs {
    #[serde(rename = "p_room_id")]
    pub room_id: String,
    #[serde(rename = "p_title")]
    pub page_name: String,
    #[serde(rename = "p_body")]
    pub body: String,
    #[serde(rename = "p_base_revision")]
    pub base_revision: Option<String>,
    #[serde(rename = "p_allow_llm_merge")]
    pub allow_llm_merge: bool,
}
egress_payload!(PublishSharedPageArgs);

/// `withdraw_shared_page`.
#[derive(Serialize)]
pub struct WithdrawSharedPageArgs {
    #[serde(rename = "p_room_id")]
    pub room_id: String,
    #[serde(rename = "p_title")]
    pub page_name: String,
}
egress_payload!(WithdrawSharedPageArgs);

#[derive(Serialize)]
pub struct SaveMergedPageArgs {
    #[serde(rename = "p_room_id")]
    pub room_id: String,
    #[serde(rename = "p_title")]
    pub page_name: String,
    #[serde(rename = "p_body")]
    pub body: String,
    #[serde(rename = "p_source_revisions")]
    pub source_revisions: Vec<String>,
}
egress_payload!(SaveMergedPageArgs);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::payload::{CANARY_TITLE, CANARY_URL, EgressPayload, forbidden_key};

    fn json<P: EgressPayload>(p: &P) -> serde_json::Value {
        serde_json::to_value(p).expect("serialize")
    }

    #[test]
    fn every_argument_is_an_rpc_parameter_and_none_is_a_forbidden_key() {
        let samples = [
            json(&PublishSharedPageArgs {
                room_id: "r".into(),
                page_name: "p".into(),
                body: "b".into(),
                base_revision: None,
                allow_llm_merge: false,
            }),
            json(&WithdrawSharedPageArgs {
                room_id: "r".into(),
                page_name: "p".into(),
            }),
            json(&SaveMergedPageArgs {
                room_id: "r".into(),
                page_name: "p".into(),
                body: "b".into(),
                source_revisions: vec!["x".into()],
            }),
        ];
        for sample in samples {
            let map = sample.as_object().expect("object");
            assert!(!map.is_empty());
            for key in map.keys() {
                assert!(key.starts_with("p_") && !forbidden_key(key), "{key}");
            }
            assert!(
                !sample.to_string().contains(CANARY_TITLE)
                    && !sample.to_string().contains(CANARY_URL)
            );
        }
    }

    #[test]
    fn debug_hides_the_body() {
        let args = PublishSharedPageArgs {
            room_id: "r".into(),
            page_name: "p".into(),
            body: "SECRET-BODY".into(),
            base_revision: None,
            allow_llm_merge: true,
        };
        assert!(!format!("{args:?}").contains("SECRET-BODY"));
    }
}
