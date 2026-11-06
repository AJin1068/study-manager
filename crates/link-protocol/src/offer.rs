//! The pairing offer: the text inside the QR code the desktop app shows and the phone scans.
//!
//! It carries a short-lived one-time code, the server address and the PUBLIC anon key (row level
//! security protects the data, the key is not a secret). It carries no long-lived secret and no
//! session token: the phone signs in anonymously by itself after scanning.

use serde::{Deserialize, Serialize};

pub const OFFER_VERSION: u32 = 1;
pub const CODE_LEN: usize = 10;
const ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const KEY_MAX: usize = 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairingOffer {
    pub v: u32,
    pub code: String,
    pub endpoint: String,
    pub anon_key: String,
}

impl std::fmt::Debug for PairingOffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingOffer { .. }")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfferError {
    NotJson,
    Version,
    BadCode,
    BadEndpoint,
    BadKey,
}

impl std::fmt::Display for OfferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OfferError::NotJson => "not_an_offer",
            OfferError::Version => "unsupported_version",
            OfferError::BadCode => "bad_code",
            OfferError::BadEndpoint => "bad_endpoint",
            OfferError::BadKey => "bad_key",
        })
    }
}

impl std::error::Error for OfferError {}

/// Upper-cases, drops separators, maps `O`->`0` and `I`/`L`->`1`, checks length and alphabet.
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

/// https anywhere, plain http only for a loopback address (local development).
fn endpoint_ok(endpoint: &str) -> bool {
    let Some((scheme, rest)) = endpoint.split_once("://") else { return false };
    let authority = rest.trim_end_matches('/');
    if authority.is_empty() || authority.contains(['/', '?', '#', '@', ' ']) {
        return false;
    }
    let host = authority.split(':').next().unwrap_or_default();
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return false;
    }
    match scheme {
        "https" => true,
        "http" => host == "localhost" || host == "127.0.0.1",
        _ => false,
    }
}

impl PairingOffer {
    pub fn new(code: &str, endpoint: &str, anon_key: &str) -> Result<Self, OfferError> {
        let offer = Self {
            v: OFFER_VERSION,
            code: normalize_code(code).ok_or(OfferError::BadCode)?,
            endpoint: endpoint.trim_end_matches('/').to_string(),
            anon_key: anon_key.to_string(),
        };
        offer.validate()?;
        Ok(offer)
    }

    fn validate(&self) -> Result<(), OfferError> {
        if self.v != OFFER_VERSION {
            return Err(OfferError::Version);
        }
        if normalize_code(&self.code).as_deref() != Some(self.code.as_str()) {
            return Err(OfferError::BadCode);
        }
        if !endpoint_ok(&self.endpoint) {
            return Err(OfferError::BadEndpoint);
        }
        if self.anon_key.is_empty() || self.anon_key.len() > KEY_MAX || self.anon_key.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(OfferError::BadKey);
        }
        Ok(())
    }

    /// The text to put into the QR code.
    pub fn to_qr_text(&self) -> Result<String, OfferError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| OfferError::NotJson)
    }

    /// Reads scanned text; a half-valid offer is refused whole.
    pub fn parse(text: &str) -> Result<Self, OfferError> {
        let offer: Self = serde_json::from_str(text.trim()).map_err(|_| OfferError::NotJson)?;
        offer.validate()?;
        Ok(offer)
    }
}
