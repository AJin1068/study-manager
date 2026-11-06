//! Wire format of the browser link, independent of how bytes travel. Frames are the native
//! messaging framing (4-byte native-endian length, then UTF-8 JSON) so the host can relay them
//! unchanged between Chrome's stdio and the local socket.
//!
//! Privacy: a tab URL is parsed into a domain right away and the rest is dropped. The page title
//! is read into memory only for the optional LLM tab judge (default off) and is dropped after the
//! verdict: it is never logged or stored. The message types print `<redacted>` for the URL (and
//! omit the title) in `Debug`.

use std::fmt;
use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
/// Largest frame the app or host accepts. Real tab events are a few hundred bytes.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
const MAX_VERSION_CHARS: usize = 32;
const MAX_DOMAIN_CHARS: usize = 253;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame larger than {MAX_FRAME_BYTES} bytes")]
    TooLarge,
    #[error("connection ended inside a frame")]
    Truncated,
    #[error("io error: {0}")]
    Io(io::Error),
}

/// Reads one frame. `Ok(None)` is a clean end of stream between frames. An oversized length is
/// rejected before any body byte is read.
pub fn read_frame<R: Read + ?Sized>(reader: &mut R) -> Result<Option<Vec<u8>>, FrameError> {
    let mut header = [0u8; 4];
    let mut got = 0;
    while got < header.len() {
        match reader.read(&mut header[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(FrameError::Truncated),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(FrameError::Io(e)),
        }
    }
    let len = u32::from_ne_bytes(header) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge);
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof => FrameError::Truncated,
        _ => FrameError::Io(e),
    })?;
    Ok(Some(body))
}

pub fn write_frame<W: Write + ?Sized>(writer: &mut W, body: &[u8]) -> io::Result<()> {
    let len = u32::try_from(body.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    // One write so a concurrent writer cannot interleave header and body.
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&len.to_ne_bytes());
    buf.extend_from_slice(body);
    writer.write_all(&buf)?;
    writer.flush()
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TabKind {
    Activated,
    Updated,
}

impl TabKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TabKind::Activated => "activated",
            TabKind::Updated => "updated",
        }
    }
}

/// Messages from the host or the extension to the app.
#[derive(Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Incoming {
    /// Sent by the host only, as the first frame: install token and the extension ID Chrome
    /// reported as the caller (`chrome-extension://<id>/` argument).
    Auth { token: String, origin_ext_id: String },
    /// First frame the extension sends.
    Hello { ext_id: String, version: String },
    /// `title` is optional and lives in memory only: the LLM tab judge may read a shortened form of
    /// it when the user switched that judge on; nothing stores or logs it.
    TabEvent {
        url: String,
        tab_id: i64,
        kind: TabKind,
        #[serde(default)]
        title: Option<String>,
    },
}

impl fmt::Debug for Incoming {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Incoming::Auth { .. } => f.write_str("Auth { <redacted> }"),
            Incoming::Hello { ext_id, .. } => write!(f, "Hello {{ ext_id: {ext_id} }}"),
            Incoming::TabEvent { tab_id, kind, .. } => {
                write!(f, "TabEvent {{ tab_id: {tab_id}, kind: {kind:?}, url: <redacted> }}")
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("malformed message")]
pub struct ParseError;

pub fn parse_incoming(frame: &[u8]) -> Result<Incoming, ParseError> {
    serde_json::from_slice(frame).map_err(|_| ParseError)
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerdictKind {
    Allow,
    Block,
}

/// Messages from the app to the extension.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Outgoing {
    HelloAck { protocol: u32 },
    Verdict { tab_id: i64, verdict: VerdictKind, reason_code: &'static str },
    /// `code` is a fixed identifier, never free text.
    Error { code: &'static str },
}

pub fn encode(message: &Outgoing) -> Vec<u8> {
    // Serialization of these plain enums cannot fail; an empty frame would be rejected by the peer.
    serde_json::to_vec(message).unwrap_or_default()
}

/// Chrome extension IDs are 32 characters from `a` to `p`.
pub fn is_valid_ext_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

pub fn is_valid_version(version: &str) -> bool {
    !version.is_empty() && version.chars().count() <= MAX_VERSION_CHARS && version.chars().all(|c| !c.is_control())
}

/// The host of an http(s) URL as the site-rule engine sees it (same `url::Url` parse, so the
/// gate and `judge` cannot disagree on what is a web page): lowercased, punycode, without
/// userinfo, port, or leading/trailing dots. `None` for other schemes, IPv6 literals, and
/// anything `Url` rejects. This is the only part of a URL that leaves memory (logs, `block_events`).
pub fn domain_of(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = match url.host()? {
        url::Host::Domain(name) => name.trim_matches('.').to_owned(),
        url::Host::Ipv4(ip) => ip.to_string(),
        url::Host::Ipv6(_) => return None,
    };
    let well_formed = !host.is_empty() && host.chars().count() <= MAX_DOMAIN_CHARS && !host.chars().any(char::is_control);
    well_formed.then_some(host)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn frame(body: &[u8]) -> Vec<u8> {
        let mut out = (body.len() as u32).to_ne_bytes().to_vec();
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn frame_round_trip_and_clean_end() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"{\"a\":1}").expect("write");
        write_frame(&mut buf, b"{}").expect("write");
        let mut reader = Cursor::new(buf);
        assert_eq!(read_frame(&mut reader).expect("first"), Some(b"{\"a\":1}".to_vec()));
        assert_eq!(read_frame(&mut reader).expect("second"), Some(b"{}".to_vec()));
        assert_eq!(read_frame(&mut reader).expect("end"), None);
    }

    #[test]
    fn truncated_header_and_body_are_errors() {
        assert!(matches!(read_frame(&mut Cursor::new(vec![1u8, 0])), Err(FrameError::Truncated)));
        let mut cut = frame(b"hello");
        cut.truncate(6);
        assert!(matches!(read_frame(&mut Cursor::new(cut)), Err(FrameError::Truncated)));
    }

    #[test]
    fn oversized_length_is_rejected_without_reading_the_body() {
        let header = ((MAX_FRAME_BYTES + 1) as u32).to_ne_bytes().to_vec();
        let mut reader = Cursor::new(header);
        assert!(matches!(read_frame(&mut reader), Err(FrameError::TooLarge)));
        assert_eq!(reader.position(), 4, "only the header was consumed");
        let huge = u32::MAX.to_ne_bytes().to_vec();
        assert!(matches!(read_frame(&mut Cursor::new(huge)), Err(FrameError::TooLarge)));
        let max = frame(&vec![b' '; MAX_FRAME_BYTES]);
        assert!(read_frame(&mut Cursor::new(max)).expect("exactly max is fine").is_some());
    }

    #[test]
    fn parses_each_message_type() {
        let hello = parse_incoming(br#"{"type":"hello","extId":"abcdefghijklmnopabcdefghijklmnop","version":"0.1.0"}"#);
        assert!(matches!(hello, Ok(Incoming::Hello { .. })));
        let tab = parse_incoming(br#"{"type":"tab_event","url":"https://a.example/x?y=1","title":"Secret","tabId":7,"kind":"updated"}"#)
            .expect("tab event");
        assert_eq!(
            tab,
            Incoming::TabEvent { url: "https://a.example/x?y=1".into(), tab_id: 7, kind: TabKind::Updated, title: Some("Secret".into()) }
        );
        let untitled = parse_incoming(br#"{"type":"tab_event","url":"https://a.example/","tabId":8,"kind":"activated"}"#).expect("no title");
        assert_eq!(untitled, Incoming::TabEvent { url: "https://a.example/".into(), tab_id: 8, kind: TabKind::Activated, title: None });
        let auth = parse_incoming(br#"{"type":"auth","token":"t","originExtId":"abc"}"#);
        assert!(matches!(auth, Ok(Incoming::Auth { .. })));
    }

    #[test]
    fn rejects_malformed_and_unknown_messages() {
        let cases: [&[u8]; 12] = [
            b"",
            b"not json",
            b"[]",
            b"null",
            b"{}",
            br#"{"type":"nope"}"#,
            br#"{"type":"tab_event"}"#,
            br#"{"type":"tab_event","url":"https://a.example/","tabId":"7","kind":"updated"}"#,
            br#"{"type":"tab_event","url":"https://a.example/","tabId":7,"kind":"hovered"}"#,
            br#"{"type":"hello","extId":1,"version":"1"}"#,
            &[0xff, 0xfe, 0x00],
            br#"{"type":"verdict","tabId":1}"#,
        ];
        for case in cases {
            assert!(parse_incoming(case).is_err(), "should reject {:?}", String::from_utf8_lossy(case));
        }
    }

    /// Cheap fuzz: every prefix and every single-byte corruption of a valid frame stream must
    /// produce a value or an error, never a panic or an oversized allocation.
    #[test]
    fn parser_survives_corrupted_input() {
        let valid = frame(br#"{"type":"tab_event","url":"https://a.example/","tabId":1,"kind":"activated"}"#);
        for cut in 0..=valid.len() {
            let _ = read_frame(&mut Cursor::new(valid[..cut].to_vec())).map(|f| f.map(|b| parse_incoming(&b)));
        }
        for i in 0..valid.len() {
            for flip in [0x00u8, 0xff, 0x7b, 0x22] {
                let mut bytes = valid.clone();
                bytes[i] = flip;
                let _ = read_frame(&mut Cursor::new(bytes)).map(|f| f.map(|b| parse_incoming(&b)));
            }
        }
    }

    #[test]
    fn debug_never_prints_the_url_or_token() {
        let tab = Incoming::TabEvent {
            url: "https://bank.example/account?token=abc".into(),
            tab_id: 1,
            kind: TabKind::Updated,
            title: Some("SECRET-TITLE".into()),
        };
        let auth = Incoming::Auth { token: "sekrettoken".into(), origin_ext_id: "x".into() };
        let printed = format!("{tab:?} {auth:?}");
        assert!(!printed.contains("bank.example") && !printed.contains("token=abc") && !printed.contains("sekret") && !printed.contains("SECRET-TITLE"), "{printed}");
    }

    #[test]
    fn outgoing_wire_shape() {
        let v = encode(&Outgoing::Verdict { tab_id: 3, verdict: VerdictKind::Block, reason_code: "blocked_site" });
        assert_eq!(String::from_utf8(v).expect("utf8"), r#"{"type":"verdict","tabId":3,"verdict":"block","reasonCode":"blocked_site"}"#);
        let e = encode(&Outgoing::Error { code: "app_not_running" });
        assert_eq!(String::from_utf8(e).expect("utf8"), r#"{"type":"error","code":"app_not_running"}"#);
    }

    #[test]
    fn domain_extraction_table() {
        let cases: [(&str, Option<&str>); 30] = [
            ("https://www.YouTube.com/watch?v=abc&list=PL1", Some("www.youtube.com")),
            ("http://example.com", Some("example.com")),
            ("https://example.com:8443/path", Some("example.com")),
            ("https://example.com./", Some("example.com")),
            ("https://user:pw@example.com/", Some("example.com")),
            ("https://google.com@evil.example/", Some("evil.example")),
            ("https://google.com:443@evil.example:80/x", Some("evil.example")),
            ("https://example.com?x=1", Some("example.com")),
            ("https://example.com#frag", Some("example.com")),
            ("https://example.com\\@evil.test/", Some("example.com")),
            ("HTTPS://Example.COM/A", Some("example.com")),
            ("https://한글.example/경로", Some("xn--bj0bj06e.example")),
            ("https://192.168.0.1:3000/", Some("192.168.0.1")),
            ("https://[::1]:80/", None),
            ("ftp://example.com/", None),
            ("chrome://settings", None),
            ("about:blank", None),
            ("https:///nohost", Some("nohost")),
            ("https:youtube.com", Some("youtube.com")),
            ("https:/youtube.com/", Some("youtube.com")),
            ("https://you\ttube.com/", Some("youtube.com")),
            ("https://youtube%2ecom/", Some("youtube.com")),
            ("https://.youtube.com/", Some("youtube.com")),
            ("  https://youtube.com/  ", Some("youtube.com")),
            ("https://youtube.com%00/", None),
            ("https://YOUTUBE.com:443/", Some("youtube.com")),
            ("https://exa mple.com/", None),
            ("https://example.com:80x/", None),
            ("https://a..b/", Some("a..b")),
            ("javascript:alert(1)", None),
        ];
        for (url, want) in cases {
            assert_eq!(domain_of(url).as_deref(), want, "url: {url}");
        }
    }

    #[test]
    fn identity_validators() {
        assert!(is_valid_ext_id("abcdefghijklmnopabcdefghijklmnop"));
        assert!(!is_valid_ext_id("short"));
        assert!(!is_valid_ext_id("abcdefghijklmnopabcdefghijklmnoq"));
        assert!(is_valid_version("0.1.0"));
        assert!(!is_valid_version(""));
        assert!(!is_valid_version(&"9".repeat(33)));
    }
}
