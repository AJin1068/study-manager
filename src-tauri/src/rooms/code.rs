//! Invite codes and the invite payload (deep link / QR), and the client-side join cooldown.
//!
//! The server builds the code (10 characters of the 32-letter Crockford alphabet, about 50 bits
//! from the server CSPRNG) and keeps only its hash. This file only normalizes what a person typed
//! or scanned, so `o`/`O` and `i`/`l` typos reach the server as the digits they stand for.
//!
//! The payload is `studymanager://join?c=<code>&e=<server address>&k=<public anon key>`. The anon
//! key is a public client key (row level security protects the data), the same value the
//! integrations screen asks for; the payload carries no other secret. The code is a credential for
//! entering the room: it is shown in the UI, never logged.

use std::collections::VecDeque;

use url::Url;

pub const CODE_LEN: usize = 10;
const ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const SCHEME: &str = "studymanager";

/// Upper-cases, drops separators and spaces, maps `O`->`0` and `I`/`L`->`1`, and checks the
/// length and alphabet. `None` means "cannot be a code"; nothing is sent to the server for it.
pub fn normalize_code(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(CODE_LEN);
    for ch in raw.chars() {
        if ch.is_whitespace() || ch == '-' || ch == '_' {
            continue;
        }
        let ch = match ch.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        };
        if !ALPHABET.contains(ch) {
            return None;
        }
        out.push(ch);
    }
    (out.len() == CODE_LEN).then_some(out)
}

/// `ABCDE-FGHJK`, for reading aloud or typing.
pub fn display_code(code: &str) -> String {
    if code.len() == CODE_LEN && code.is_char_boundary(CODE_LEN / 2) {
        format!("{}-{}", &code[..CODE_LEN / 2], &code[CODE_LEN / 2..])
    } else {
        code.to_string()
    }
}

#[derive(Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvitePayload {
    pub code: String,
    pub endpoint: Option<String>,
    #[serde(skip)]
    pub anon_key: Option<String>,
}

impl std::fmt::Debug for InvitePayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InvitePayload { .. }")
    }
}

/// The deep link for a new invite. `None` when the address is not a valid server endpoint.
pub fn encode_invite(code: &str, endpoint: &str, anon_key: &str) -> Option<String> {
    let code = normalize_code(code)?;
    if !crate::remote::http::is_valid_endpoint(endpoint) || anon_key.is_empty() || anon_key.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let mut url = Url::parse(&format!("{SCHEME}://join")).ok()?;
    url.query_pairs_mut().append_pair("c", &code).append_pair("e", endpoint.trim_end_matches('/')).append_pair("k", anon_key);
    Some(url.to_string())
}

/// Reads either a deep link or a bare code. A link whose address or key is malformed is refused
/// whole (a half-valid payload must not fill in a server address).
pub fn decode_invite(text: &str) -> Option<InvitePayload> {
    let text = text.trim();
    if !text.contains("://") {
        return normalize_code(text).map(|code| InvitePayload { code, endpoint: None, anon_key: None });
    }
    let url = Url::parse(text).ok()?;
    if url.scheme() != SCHEME || url.host_str() != Some("join") {
        return None;
    }
    let mut code = None;
    let mut endpoint = None;
    let mut key = None;
    for (name, value) in url.query_pairs() {
        match name.as_ref() {
            "c" => code = normalize_code(&value),
            "e" => endpoint = Some(value.into_owned()),
            "k" => key = Some(value.into_owned()),
            _ => {}
        }
    }
    let code = code?;
    match (endpoint, key) {
        (None, None) => Some(InvitePayload { code, endpoint: None, anon_key: None }),
        (Some(endpoint), Some(key)) => {
            let key_ok = !key.is_empty() && key.len() <= 1024 && !key.chars().any(|c| c.is_whitespace() || c.is_control());
            (crate::remote::http::is_valid_endpoint(&endpoint) && key_ok)
                .then(|| InvitePayload { code, endpoint: Some(endpoint.trim_end_matches('/').to_string()), anon_key: Some(key) })
        }
        _ => None,
    }
}

/// Client-side brake on join attempts, in front of the server's own limit (5 a minute): at most
/// `MAX_PER_WINDOW` tries in a rolling minute and at least `MIN_GAP_MS` between two tries.
#[derive(Default)]
pub struct JoinGate {
    attempts: VecDeque<i64>,
}

pub const WINDOW_MS: i64 = 60_000;
pub const MAX_PER_WINDOW: usize = 5;
pub const MIN_GAP_MS: i64 = 2_000;

impl JoinGate {
    /// `Ok` records the attempt. `Err(wait_ms)` says how long until the next one is allowed.
    pub fn try_attempt(&mut self, now_ms: i64) -> Result<(), i64> {
        while self.attempts.front().is_some_and(|&at| now_ms - at >= WINDOW_MS) {
            self.attempts.pop_front();
        }
        if let Some(&last) = self.attempts.back() {
            let gap = now_ms - last;
            if gap < MIN_GAP_MS {
                return Err(MIN_GAP_MS - gap);
            }
        }
        if self.attempts.len() >= MAX_PER_WINDOW {
            let oldest = self.attempts.front().copied().unwrap_or(now_ms);
            return Err(WINDOW_MS - (now_ms - oldest));
        }
        self.attempts.push_back(now_ms);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_normalized_like_a_person_would_type_them() {
        assert_eq!(normalize_code("abcde-fghjk").as_deref(), Some("ABCDEFGHJK"));
        assert_eq!(normalize_code(" 0123 4567 89 ").as_deref(), Some("0123456789"));
        assert_eq!(normalize_code("OIL0OIL0OI").as_deref(), Some("0110011001"), "O->0, I/L->1");
        assert_eq!(display_code("ABCDEFGHJK"), "ABCDE-FGHJK");
    }

    #[test]
    fn a_string_that_cannot_be_a_code_is_refused_before_any_request() {
        for bad in ["", "ABCDEFGHJ", "ABCDEFGHJKM", "ABCDEFGHJU", "ABCDEFGH!K", "한글한글한글한글한글"] {
            assert_eq!(normalize_code(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_invite_payload_round_trips_and_carries_only_code_address_and_public_key() {
        let link = encode_invite("abcde-fghjk", "https://proj.supabase.co/", "anon-public-key").expect("link");
        assert!(link.starts_with("studymanager://join?"), "{link}");
        let parsed = decode_invite(&link).expect("parse");
        assert_eq!(parsed.code, "ABCDEFGHJK");
        assert_eq!(parsed.endpoint.as_deref(), Some("https://proj.supabase.co"));
        assert_eq!(parsed.anon_key.as_deref(), Some("anon-public-key"));
        let keys: Vec<String> = Url::parse(&link).expect("url").query_pairs().map(|(k, _)| k.into_owned()).collect();
        assert_eq!(keys, ["c", "e", "k"]);
    }

    #[test]
    fn a_bare_code_and_a_link_without_a_server_are_accepted() {
        let bare = decode_invite("abcde fghjk").expect("bare");
        assert_eq!((bare.code.as_str(), bare.endpoint, bare.anon_key), ("ABCDEFGHJK", None, None));
        let no_server = decode_invite("studymanager://join?c=ABCDEFGHJK").expect("link");
        assert_eq!(no_server.endpoint, None);
    }

    #[test]
    fn a_malformed_payload_is_refused_whole() {
        for bad in [
            "studymanager://join?c=ABCDEFGHJK&e=http%3A%2F%2Fevil.example&k=key",
            "studymanager://join?c=ABCDEFGHJK&e=https%3A%2F%2Fproj.supabase.co",
            "studymanager://join?c=ABCDEFGHJK&k=key",
            "studymanager://join?c=SHORT&e=https%3A%2F%2Fproj.supabase.co&k=key",
            "otherapp://join?c=ABCDEFGHJK",
            "studymanager://other?c=ABCDEFGHJK",
        ] {
            assert_eq!(decode_invite(bad), None, "{bad}");
        }
        assert_eq!(encode_invite("ABCDEFGHJK", "http://evil.example", "key"), None);
        assert_eq!(encode_invite("ABCDEFGHJK", "https://proj.supabase.co", "has space"), None);
    }

    #[test]
    fn debug_never_prints_the_code() {
        let payload = decode_invite("ABCDEFGHJK").expect("payload");
        assert!(!format!("{payload:?}").contains("ABCDE"));
    }

    #[test]
    fn the_join_gate_enforces_a_gap_and_a_rolling_window() {
        let mut gate = JoinGate::default();
        assert_eq!(gate.try_attempt(0), Ok(()));
        assert_eq!(gate.try_attempt(500), Err(1_500), "too soon after the last try");
        for i in 1..5 {
            assert_eq!(gate.try_attempt(i * 2_000), Ok(()), "attempt {i}");
        }
        assert_eq!(gate.try_attempt(10_000), Err(50_000), "sixth try inside a minute waits for the first to age out");
        assert_eq!(gate.try_attempt(60_000), Ok(()), "the oldest attempt left the window");
    }
}
