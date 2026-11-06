//! Rule-based candidate composition. Nothing here is random in the OS sense: a seeded generator
//! and the normalized input decide everything, so the same input and seed always give the same
//! candidates and a re-roll is just the next seed.
//!
//! The rules (all optional inputs; a skipped input leaves that choice open):
//! - MBTI I/E picks the expression pool (E: lively, I: quiet); T/F picks the prop pool (T: study
//!   tools, F: soft things) and the fur temperature (T: cool, F: warm).
//! - The birth year's zodiac animal picks the species that comes first.
//! - Gender only picks the free starter top.
//!
//! Inputs are never sent anywhere: the name prompt carries only part labels.

use super::catalog::{Catalog, Energy, Mind, Temperature};
use super::{Candidate, Gender, Look, Traits};
use crate::messages::Tone;

/// Species of the zodiac animal, indexed by `(year - 4) mod 12` (rat first).
pub const ZODIAC_SPECIES: [&str; 12] =
    ["squirrel", "bear", "cat", "rabbit", "owl", "frog", "deer", "deer", "squirrel", "owl", "fox", "bear"];

pub fn zodiac_index(year: i32) -> usize {
    usize::try_from((year - 4).rem_euclid(12)).unwrap_or(0)
}

/// Free starter tops by gender; the open choice gets the vest.
pub fn starter_top(gender: Option<Gender>) -> &'static str {
    match gender {
        Some(Gender::Female) => "top_cardigan",
        Some(Gender::Male) => "top_hoodie",
        Some(Gender::Other) | None => "top_vest",
    }
}

/// The message tone an MBTI starts with: F types lean gentle, T types firm; J is stricter than P.
/// Never harsh. `None` for anything that is not an MBTI.
pub fn tone_for_mbti(mbti: &str) -> Option<Tone> {
    let letters: Vec<char> = mbti.trim().to_uppercase().chars().collect();
    let [ie, sn, tf, jp] = letters.as_slice() else { return None };
    if !matches!(ie, 'I' | 'E') || !matches!(sn, 'S' | 'N') || !matches!(tf, 'T' | 'F') || !matches!(jp, 'J' | 'P') {
        return None;
    }
    Some(match (tf, jp) {
        ('F', 'P') => Tone::Gentle,
        ('F', _) => Tone::Warm,
        (_, 'P') => Tone::Normal,
        _ => Tone::Firm,
    })
}

struct Rng(u64);

impl Rng {
    /// SplitMix64.
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        usize::try_from(self.next() % u64::try_from(n).unwrap_or(1)).unwrap_or(0)
    }
}

fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn pick<'a, T>(rng: &mut Rng, pool: &[&'a T]) -> Option<&'a T> {
    pool.get(rng.below(pool.len())).copied()
}

/// 3 or 4 distinct candidates (distinct species).
pub fn compose(cat: &Catalog, traits: &Traits, seed: u64) -> Vec<Candidate> {
    let mut rng = Rng(seed ^ fnv1a(&traits.key()));
    let count = 3 + rng.below(2);

    let mut species: Vec<&str> = cat.species.iter().filter(|s| !s.shop_only).map(|s| s.id.as_str()).collect();
    for i in (1..species.len()).rev() {
        species.swap(i, rng.below(i + 1));
    }
    if let Some(favored) = traits.zodiac.map(|z| ZODIAC_SPECIES[z % ZODIAC_SPECIES.len()]) {
        if let Some(at) = species.iter().position(|s| *s == favored) {
            let id = species.remove(at);
            species.insert(0, id);
        }
    }

    let temperature = traits.thinking.map(|t| if t { Temperature::Cool } else { Temperature::Warm });
    let energy = traits.extravert.map(|e| if e { Energy::High } else { Energy::Low });
    let mind = traits.thinking.map(|t| if t { Mind::Thinking } else { Mind::Feeling });
    let palettes: Vec<_> = cat.palettes.iter().filter(|p| temperature.is_none_or(|t| p.temperature == t)).collect();
    let expressions: Vec<_> = cat.expressions.iter().filter(|e| energy.is_none_or(|n| e.energy == n)).collect();
    let props: Vec<_> = cat.props.iter().filter(|p| mind.is_none_or(|m| p.mind == m)).collect();

    let mut out: Vec<Candidate> = Vec::new();
    for id in species.into_iter().take(count) {
        // Prefer a fur color the earlier candidates do not use.
        let fresh: Vec<_> = palettes.iter().copied().filter(|p| !out.iter().any(|c| c.look.palette == p.id)).collect();
        let palette = pick(&mut rng, if fresh.is_empty() { &palettes } else { &fresh });
        let expression = pick(&mut rng, &expressions);
        let prop = pick(&mut rng, &props);
        if let (Some(palette), Some(expression), Some(prop)) = (palette, expression, prop) {
            out.push(Candidate {
                look: Look {
                    species: id.to_string(),
                    palette: palette.id.clone(),
                    expression: expression.id.clone(),
                    prop: prop.id.clone(),
                },
                starter_top: starter_top(traits.gender).to_string(),
            });
        }
    }
    out
}

/// Built-in names; used when the model is off or answers badly. Original, short, Korean.
pub const BUILTIN_NAMES: [&str; 16] =
    ["도토리", "보리", "솔이", "하루", "모카", "별이", "콩이", "구름", "바람", "달이", "새봄", "나루", "단비", "소라", "루미", "꼬미"];

pub fn builtin_name(seed: u64, index: usize) -> String {
    let base = usize::try_from(seed % 16).unwrap_or(0);
    BUILTIN_NAMES[(base + index * 5) % BUILTIN_NAMES.len()].to_string()
}

/// One line for a look, from the expression and the prop in hand. No particles depend on the noun.
pub fn builtin_blurb(cat: &Catalog, look: &Look) -> String {
    let prop = cat.prop(&look.prop).map_or("작은 소품", |p| p.label.as_str());
    let tail = match look.expression.as_str() {
        "smile" => "얼굴에는 웃음",
        "bright" => "눈에는 반짝임",
        "cheer" => "입에는 응원",
        "calm" => "마음에는 차분함",
        "sleepy" => "곁에는 포근한 졸음",
        _ => "눈에는 진지함",
    };
    format!("손에는 {prop}, {tail}. 같이 천천히 공부해요.")
}

/// The mock model's answer for `count` candidates: the built-in names.
pub fn mock_names_json(count: usize) -> String {
    let items: Vec<_> = (0..count)
        .map(|i| serde_json::json!({"name": builtin_name(0, i), "intro": "함께 공부하는 숲속 친구예요."}))
        .collect();
    serde_json::json!({ "items": items }).to_string()
}

pub const NAME_MAX_CHARS: usize = 8;
pub const INTRO_MAX_CHARS: usize = 40;

/// A usable name: trimmed, 1-8 characters, no control characters, not a protected name.
pub fn clean_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    let ok = !name.is_empty() && name.chars().count() <= NAME_MAX_CHARS && !name.chars().any(char::is_control) && !super::catalog::is_banned(name);
    ok.then(|| name.to_string())
}

pub fn clean_intro(raw: &str) -> Option<String> {
    let intro = raw.trim();
    let ok = !intro.is_empty() && intro.chars().count() <= INTRO_MAX_CHARS && !intro.chars().any(char::is_control) && !super::catalog::is_banned(intro);
    ok.then(|| intro.to_string())
}

#[derive(serde::Deserialize)]
struct RawNames {
    items: Vec<RawName>,
}

#[derive(serde::Deserialize)]
struct RawName {
    name: String,
    intro: String,
}

/// The model's `(name, intro)` per candidate, `None` where the entry is missing or unusable.
pub fn parse_names(answer: &str, count: usize) -> Vec<Option<(String, String)>> {
    let parsed: Option<RawNames> = serde_json::from_str(answer.trim()).ok();
    let mut out = vec![None; count];
    if let Some(parsed) = parsed {
        for (slot, entry) in out.iter_mut().zip(parsed.items) {
            *slot = clean_name(&entry.name).zip(clean_intro(&entry.intro));
        }
    }
    out
}
