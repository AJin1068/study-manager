//! Names that become file or folder names. Subject names typed by the user are cleaned into a slug;
//! page names that come back from a model are validated strictly (a bad one is rejected, never
//! repaired into something else), because a name is the only thing that decides where a file goes.

use crate::error::{AppError, AppResult};

pub const MAX_NAME_CHARS: usize = 80;

/// Characters no portable file name may contain.
const FORBIDDEN: &str = "/\\:*?\"<>|";

/// Device names Windows refuses as file names, with or without an extension.
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

fn is_reserved(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_ascii_lowercase();
    RESERVED.contains(&stem.as_str())
}

fn squeeze_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Folder name for a subject typed by the user: forbidden and control characters dropped, runs of
/// whitespace become one `-`, leading dots and trailing dots/spaces removed. Hangul and emoji stay.
pub fn subject_slug(name: &str) -> AppResult<String> {
    let kept: String = name
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control() && !FORBIDDEN.contains(*c))
        .collect();
    let joined = squeeze_spaces(&kept).replace(' ', "-");
    let trimmed = joined.trim_start_matches('.').trim_end_matches(['.', '-']);
    let slug: String = trimmed.chars().take(MAX_NAME_CHARS).collect();
    let slug = slug.trim_end_matches(['.', '-']).to_string();
    if slug.is_empty() {
        return Err(AppError::Invalid(
            "the subject name has no usable characters".into(),
        ));
    }
    if is_reserved(&slug) {
        return Ok(format!("{slug}-과목"));
    }
    Ok(slug)
}

/// A page name from a model or a file stem: accepted as is (whitespace squeezed) or rejected.
pub fn page_name(raw: &str) -> Option<String> {
    let name = squeeze_spaces(raw);
    let ok = !name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && !name
            .chars()
            .any(|c| c.is_control() || FORBIDDEN.contains(c))
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.contains("..")
        && !is_reserved(&name);
    ok.then_some(name)
}

pub fn page_file_name(name: &str) -> String {
    format!("{name}.md")
}

/// Page name of a `*.md` file name, if it is a valid page.
pub fn page_name_of_file(file_name: &str) -> Option<String> {
    let stem = file_name.strip_suffix(".md")?;
    page_name(stem).filter(|name| name == stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_keep_hangul_and_emoji_and_drop_what_a_path_cannot_hold() {
        for (input, expected) in [
            ("수학", "수학"),
            ("  선형 대수  ", "선형-대수"),
            ("📚 영어 독해", "📚-영어-독해"),
            ("a/b\\c:d*e?f\"g<h>i|j", "abcdefghij"),
            ("..hidden", "hidden"),
            ("끝점.", "끝점"),
            ("tab\tnew\nline", "tab-new-line"),
        ] {
            assert_eq!(
                subject_slug(input).as_deref().ok(),
                Some(expected),
                "{input:?}"
            );
        }
        assert_eq!(subject_slug("CON").expect("reserved"), "CON-과목");
        assert_eq!(subject_slug("nul.txt").expect("reserved"), "nul.txt-과목");
        assert_eq!(
            subject_slug(&"가".repeat(200))
                .expect("long")
                .chars()
                .count(),
            MAX_NAME_CHARS
        );
    }

    #[test]
    fn a_name_with_nothing_usable_is_an_error() {
        for input in ["", "   ", "///", "..", "...", "\u{0}\u{7}"] {
            assert!(subject_slug(input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn page_names_are_validated_not_repaired() {
        assert_eq!(page_name("  미분   기초 ").as_deref(), Some("미분 기초"));
        for bad in [
            "",
            "   ",
            "../x",
            "a/b",
            "a\\b",
            "x:y",
            ".hidden",
            "trail.",
            "a..b",
            "nul",
            "con.md",
            "bell\u{7}",
            "q?",
            "a|b",
            &"x".repeat(81),
        ] {
            assert_eq!(page_name(bad), None, "{bad:?}");
        }
        assert_eq!(
            page_name(&"가".repeat(80)).map(|n| n.chars().count()),
            Some(80)
        );
    }

    #[test]
    fn file_names_round_trip_only_for_canonical_pages() {
        assert_eq!(page_file_name("미분"), "미분.md");
        assert_eq!(page_name_of_file("미분.md").as_deref(), Some("미분"));
        assert_eq!(page_name_of_file("미분.txt"), None);
        assert_eq!(
            page_name_of_file(" 미분 .md"),
            None,
            "padding would change the name"
        );
        assert_eq!(page_name_of_file("a  b.md"), None);
        assert_eq!(page_name_of_file(".md"), None);
    }
}
