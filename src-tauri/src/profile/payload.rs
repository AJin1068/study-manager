//! Argument structs of the profile and gift RPCs, reviewed like `remote::payload`. Each field name
//! is the RPC parameter name. `Debug` hides every value.
//!
//! What can leave: the display name the user typed for the profile, scopes, the level, achievement
//! badge ids, owned furniture ids, wiki page NAMES the user chose to show, the current subject,
//! per-day study minutes, a guestbook line, and a gift (item id, short message, recipient id).
//! There is no field for attempts, apps, window titles, URLs, typed text, wiki bodies or points.

use serde::Serialize;

use crate::remote::payload::egress_payload;

/// `set_profile_settings`. Built only by `publish::settings_args`, which empties every section
/// whose scope is private.
#[derive(Serialize, Clone, PartialEq, Eq)]
pub struct SetProfileArgs {
    #[serde(rename = "p_display_name")]
    pub display_name: String,
    #[serde(rename = "p_vis_room")]
    pub vis_room: &'static str,
    #[serde(rename = "p_vis_grass")]
    pub vis_grass: &'static str,
    #[serde(rename = "p_vis_level")]
    pub vis_level: &'static str,
    #[serde(rename = "p_vis_badges")]
    pub vis_badges: &'static str,
    #[serde(rename = "p_vis_guestbook")]
    pub vis_guestbook: &'static str,
    #[serde(rename = "p_vis_visitors")]
    pub vis_visitors: &'static str,
    #[serde(rename = "p_vis_wiki")]
    pub vis_wiki: &'static str,
    #[serde(rename = "p_vis_subject")]
    pub vis_subject: &'static str,
    #[serde(rename = "p_vis_gifts")]
    pub vis_gifts: &'static str,
    #[serde(rename = "p_level")]
    pub level: Option<i64>,
    #[serde(rename = "p_badges")]
    pub badges: Vec<String>,
    #[serde(rename = "p_furniture")]
    pub furniture: Vec<String>,
    #[serde(rename = "p_wiki_titles")]
    pub wiki_titles: Vec<String>,
    #[serde(rename = "p_subject_now")]
    pub subject_now: Option<String>,
}
egress_payload!(SetProfileArgs);

/// One local day of study for the year grass.
#[derive(Serialize, Clone, PartialEq, Eq)]
pub struct GrassEntry {
    pub day: String,
    pub study_min: i64,
}

impl std::fmt::Debug for GrassEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GrassEntry(..)")
    }
}

#[derive(Serialize)]
pub struct GrassArgs {
    #[serde(rename = "p_entries")]
    pub entries: Vec<GrassEntry>,
}
egress_payload!(GrassArgs);

#[derive(Serialize)]
pub struct MemberArgs {
    #[serde(rename = "p_user")]
    pub user: String,
}
egress_payload!(MemberArgs);

#[derive(Serialize)]
pub struct SignGuestbookArgs {
    #[serde(rename = "p_owner")]
    pub owner: String,
    #[serde(rename = "p_body")]
    pub body: String,
}
egress_payload!(SignGuestbookArgs);

#[derive(Serialize)]
pub struct EntryIdArgs {
    #[serde(rename = "p_id")]
    pub id: i64,
}
egress_payload!(EntryIdArgs);

#[derive(Serialize)]
pub struct SendGiftArgs {
    #[serde(rename = "p_idem_key")]
    pub idem_key: String,
    #[serde(rename = "p_recipient")]
    pub recipient: String,
    #[serde(rename = "p_item_id")]
    pub item_id: String,
    #[serde(rename = "p_message")]
    pub message: String,
}
egress_payload!(SendGiftArgs);

#[derive(Serialize)]
pub struct GiftKeyArgs {
    #[serde(rename = "p_idem_key")]
    pub idem_key: String,
}
egress_payload!(GiftKeyArgs);

#[derive(Serialize)]
pub struct GiftIdArgs {
    #[serde(rename = "p_gift_id")]
    pub gift_id: String,
}
egress_payload!(GiftIdArgs);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::payload::{EgressPayload, forbidden_key};

    fn json<P: EgressPayload>(p: &P) -> serde_json::Value {
        serde_json::to_value(p).expect("serialize")
    }

    fn keys(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    out.push(k.clone());
                    keys(v, out);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| keys(v, out)),
            _ => {}
        }
    }

    fn sample_settings() -> SetProfileArgs {
        SetProfileArgs {
            display_name: "n".into(),
            vis_room: "private",
            vis_grass: "private",
            vis_level: "private",
            vis_badges: "private",
            vis_guestbook: "private",
            vis_visitors: "private",
            vis_wiki: "private",
            vis_subject: "private",
            vis_gifts: "private",
            level: None,
            badges: vec![],
            furniture: vec![],
            wiki_titles: vec![],
            subject_now: None,
        }
    }

    #[test]
    fn no_payload_has_a_forbidden_or_deviation_key() {
        let samples = [
            json(&sample_settings()),
            json(&GrassArgs {
                entries: vec![GrassEntry {
                    day: "2026-11-06".into(),
                    study_min: 5,
                }],
            }),
            json(&MemberArgs { user: "u".into() }),
            json(&SignGuestbookArgs {
                owner: "u".into(),
                body: "hi".into(),
            }),
            json(&EntryIdArgs { id: 1 }),
            json(&SendGiftArgs {
                idem_key: "k".into(),
                recipient: "u".into(),
                item_id: "hat_beanie".into(),
                message: "m".into(),
            }),
            json(&GiftKeyArgs {
                idem_key: "k".into(),
            }),
            json(&GiftIdArgs {
                gift_id: "g".into(),
            }),
        ];
        for sample in samples {
            let mut found = Vec::new();
            keys(&sample, &mut found);
            assert!(!found.is_empty());
            for key in found {
                let lower = key.to_ascii_lowercase();
                assert!(!forbidden_key(&key), "{key}");
                assert!(
                    !lower.contains("attempt")
                        && !lower.contains("deviation")
                        && !lower.contains("distraction")
                        && !lower.contains("app_name"),
                    "{key}"
                );
                assert!(!lower.contains("point"), "points never travel: {key}");
            }
        }
    }

    #[test]
    fn every_settings_argument_is_an_rpc_parameter() {
        let value = json(&sample_settings());
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/profile/rpc-contract.json"))
                .expect("contract");
        let params: Vec<&str> = contract["rpcs"]
            .as_array()
            .and_then(|r| r.iter().find(|x| x["name"] == "set_profile_settings"))
            .and_then(|x| x["params"].as_array())
            .map(|p| p.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let mut sent: Vec<&str> = value
            .as_object()
            .map(|m| m.keys().map(String::as_str).collect())
            .unwrap_or_default();
        sent.sort_unstable();
        let mut expected = params.clone();
        expected.sort_unstable();
        assert_eq!(sent, expected);
    }

    #[test]
    fn debug_hides_values() {
        let args = SignGuestbookArgs {
            owner: "u".into(),
            body: "SECRET-LINE".into(),
        };
        assert!(!format!("{args:?}").contains("SECRET-LINE"));
    }
}
