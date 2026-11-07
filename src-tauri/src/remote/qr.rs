//! "Subscribe on your phone" helper: the ntfy deep link for the configured server and topic, and
//! its QR code as an SVG string. Everything is computed locally; the topic never leaves the PC
//! through this path.

use qrcode::QrCode;
use qrcode::render::svg;
use serde::Serialize;

use super::http::host_of;

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NtfySubscription {
    pub deep_link: String,
    pub qr_svg: String,
}

/// `ntfy://<host>/<topic>` — the link the ntfy mobile app opens to subscribe.
pub fn deep_link(server: &str, topic: &str) -> Option<String> {
    let host = host_of(server)?;
    if topic.is_empty()
        || !topic
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    Some(format!("ntfy://{host}/{topic}"))
}

pub fn render_svg(text: &str) -> Option<String> {
    let code = QrCode::new(text.as_bytes()).ok()?;
    Some(
        code.render::<svg::Color>()
            .min_dimensions(200, 200)
            .quiet_zone(true)
            .build(),
    )
}

pub fn subscription(server: &str, topic: &str) -> Option<NtfySubscription> {
    let deep_link = deep_link(server, topic)?;
    let qr_svg = render_svg(&deep_link)?;
    Some(NtfySubscription { deep_link, qr_svg })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOPIC: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn the_link_names_the_server_host_and_topic() {
        assert_eq!(
            deep_link("https://ntfy.sh", TOPIC).as_deref(),
            Some("ntfy://ntfy.sh/0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            deep_link("https://ntfy.example.org/", TOPIC).as_deref(),
            Some("ntfy://ntfy.example.org/0123456789abcdef0123456789abcdef")
        );
    }

    #[test]
    fn a_missing_server_or_an_unsafe_topic_gives_no_link() {
        assert_eq!(deep_link("not a url", TOPIC), None);
        assert_eq!(deep_link("https://ntfy.sh", ""), None);
        assert_eq!(deep_link("https://ntfy.sh", "a/b"), None);
        assert_eq!(deep_link("https://ntfy.sh", "a b"), None);
    }

    #[test]
    fn the_qr_is_a_square_matrix_carrying_the_link_and_renders_as_svg() {
        let link = deep_link("https://ntfy.sh", TOPIC).expect("link");
        let code = QrCode::new(link.as_bytes()).expect("encodes");
        let width = code.width();
        assert!(width >= 21, "version 1 is 21 modules wide");
        assert_eq!(code.to_colors().len(), width * width);
        let svg = render_svg(&link).expect("svg");
        assert!(svg.contains("<svg") && svg.trim_end().ends_with("</svg>"));
        assert!(!svg.contains("<script"));
        // The same text always gives the same picture.
        assert_eq!(render_svg(&link).as_deref(), Some(svg.as_str()));
    }
}
