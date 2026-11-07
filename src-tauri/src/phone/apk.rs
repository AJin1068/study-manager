//! "Install the phone app": the APK download link (a setting) and its QR code. The QR carries only
//! the public link; the PC never serves the APK, so it is not exposed on the LAN.

use rusqlite::Connection;
use serde::Serialize;

use super::blocker::{BlockerStatus, ScreenTimeBlocker};
use crate::error::AppResult;
use crate::remote::qr;
use crate::settings::{self, FreeShape, SettingSpec, SettingValue, UpdateOutcome, ValueKind};

pub const DEFAULT_APK_LINK: &str = "https://github.com/AJin1068/study-manager/releases/latest";

pub const APK_LINK: SettingSpec = SettingSpec {
    name: "phone.apk_link",
    category: "phone",
    kind: ValueKind::Free {
        max_len: 300,
        shape: FreeShape::Endpoint,
    },
    default: DEFAULT_APK_LINK,
    tighten_to: None,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneInstallView {
    /// The link the QR encodes (an empty setting falls back to the default).
    pub link: String,
    pub default_link: String,
    pub qr_svg: Option<String>,
    /// iPhone support is always `unsupported`; the card explains the alternatives.
    pub ios: BlockerStatus,
}

fn effective_link(stored: &str) -> &str {
    if stored.is_empty() {
        DEFAULT_APK_LINK
    } else {
        stored
    }
}

pub fn view(conn: &Connection) -> AppResult<PhoneInstallView> {
    let stored = match settings::read_value(conn, &APK_LINK)? {
        SettingValue::Text(text) => text,
        other => {
            return Err(crate::error::AppError::Internal(format!(
                "phone.apk_link is not text: {other:?}"
            )));
        }
    };
    let link = effective_link(&stored).to_string();
    let qr_svg = qr::render_svg(&link);
    Ok(PhoneInstallView {
        link,
        default_link: DEFAULT_APK_LINK.to_string(),
        qr_svg,
        ios: ScreenTimeBlocker.status(),
    })
}

pub fn update_link(conn: &mut Connection, link: &str, at_ms: i64) -> AppResult<UpdateOutcome> {
    settings::apply_setting(
        conn,
        &APK_LINK,
        &SettingValue::Text(link.trim().to_string()),
        at_ms,
        || Ok(()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn the_default_view_encodes_the_releases_link_and_marks_ios_unsupported() {
        let conn = open_in_memory().expect("db");
        let view = view(&conn).expect("view");
        assert_eq!(view.link, DEFAULT_APK_LINK);
        let svg = view.qr_svg.expect("qr");
        assert!(svg.contains("<svg") && !svg.contains("<script"));
        assert_eq!(Some(svg), qr::render_svg(DEFAULT_APK_LINK));
        assert!(matches!(view.ios, BlockerStatus::Unsupported { .. }));
    }

    #[test]
    fn a_custom_https_link_is_stored_and_encoded_and_empty_restores_the_default() {
        let mut conn = open_in_memory().expect("db");
        let custom = "https://example.org/app.apk";
        let outcome = update_link(&mut conn, custom, 1).expect("update");
        assert_eq!(
            outcome,
            UpdateOutcome::Applied {
                key: "phone.apk_link".into()
            }
        );
        let changed = view(&conn).expect("view");
        assert_eq!(changed.link, custom);
        assert_eq!(changed.qr_svg, qr::render_svg(custom));
        update_link(&mut conn, "", 2).expect("reset");
        assert_eq!(view(&conn).expect("view").link, DEFAULT_APK_LINK);
    }

    #[test]
    fn unsafe_links_are_rejected_and_the_stored_link_is_unchanged() {
        let mut conn = open_in_memory().expect("db");
        for bad in [
            "http://example.org/a.apk",
            "javascript:alert(1)",
            "https://user:pw@example.org/a",
            "https://example.org/a?x=1",
            "not a url",
        ] {
            assert!(update_link(&mut conn, bad, 1).is_err(), "{bad}");
        }
        assert_eq!(view(&conn).expect("view").link, DEFAULT_APK_LINK);
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn the_view_wire_shape_matches_the_shared_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/api/fixtures/phone-install-contract.json"
        );
        let expected: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
        let mut actual = serde_json::to_value(view(&open_in_memory().expect("db")).expect("view"))
            .expect("json");
        // The QR picture itself is checked by the tests above; the contract pins the field set.
        assert!(
            actual["qrSvg"]
                .as_str()
                .is_some_and(|svg| svg.contains("<svg"))
        );
        actual["qrSvg"] = serde_json::Value::String("<svg></svg>".into());
        assert_eq!(actual, expected);
    }
}
