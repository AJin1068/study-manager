//! The web profile link. The web bundle (`web/`) is static and host-agnostic: the app only needs to
//! know where the user put it. A link is `<base>/#/u/<slug>`; the slug is a 16-hex capability for
//! reading what the owner marked public, so the link is shown only to its owner.

/// The slug alphabet the server issues (`^[a-f0-9]{16}$`).
pub fn valid_slug(slug: &str) -> bool {
    slug.len() == 16 && slug.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Accepts an https address, or http only for localhost testing. No credentials, query or fragment
/// (the app adds the fragment). Returns the trimmed base without a trailing slash.
pub fn normalize_base(raw: &str) -> Option<String> {
    let text = raw.trim();
    if text.is_empty() || text.len() > 200 || text.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (scheme, rest) = text.split_once("://")?;
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(host_end);
    if authority.is_empty() || authority.contains('@') || path.contains(['?', '#']) {
        return None;
    }
    let host = authority.split(':').next().unwrap_or_default();
    let local = host == "localhost" || host == "127.0.0.1";
    match scheme {
        "https" => {}
        "http" if local => {}
        _ => return None,
    }
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return None;
    }
    Some(format!("{scheme}://{authority}{}", path.trim_end_matches('/')))
}

/// The shareable link, or `None` until both the host address and the profile exist.
pub fn profile_link(base: Option<&str>, slug: Option<&str>) -> Option<String> {
    let base = normalize_base(base?)?;
    let slug = slug.filter(|s| valid_slug(s))?;
    Some(format!("{base}/#/u/{slug}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_needs_a_safe_base_and_a_valid_slug() {
        let slug = "0123456789abcdef";
        assert_eq!(profile_link(Some("https://me.example.test/study/"), Some(slug)).as_deref(), Some("https://me.example.test/study/#/u/0123456789abcdef"));
        assert_eq!(profile_link(Some("http://localhost:4173"), Some(slug)).as_deref(), Some("http://localhost:4173/#/u/0123456789abcdef"));
        assert_eq!(profile_link(None, Some(slug)), None);
        assert_eq!(profile_link(Some("https://x.test"), None), None);
        assert_eq!(profile_link(Some("https://x.test"), Some("not-a-slug")), None);
    }

    #[test]
    fn unsafe_bases_are_refused() {
        for bad in ["", "ftp://x.test", "http://x.test", "https://user:pw@x.test", "https://x.test/?q=1", "https://x.test/#a", "javascript:alert(1)", "https://x y.test", "https://"] {
            assert_eq!(normalize_base(bad), None, "{bad}");
        }
    }
}
