use std::collections::HashSet;
use std::sync::Arc;

use rusqlite::Connection;

use super::catalog::{catalog, is_banned};
use super::compose::*;
use super::service::CharacterService;
use super::*;
use crate::db::open_in_memory;
use crate::llm::mock::MockStep;
use crate::llm::service::tests::{mock_harness, Harness};
use crate::llm::LlmError;
use crate::messages::Tone;
use crate::points::rewards::Stage;
use crate::shop::items::{self, ItemKind, Slot};
use crate::testutil::set_lock_for_test;

const NOW: i64 = 1_800_000_000_000;
const YEAR: i32 = 2027;

fn input(mbti: Option<&str>, year: Option<i32>, gender: Option<Gender>) -> SetupInput {
    SetupInput { mbti: mbti.map(str::to_string), birth_year: year, gender }
}

fn traits(mbti: Option<&str>, year: Option<i32>, gender: Option<Gender>) -> Traits {
    input(mbti, year, gender).normalize(YEAR).expect("valid input")
}

fn candidates(t: &Traits, seed: u64) -> Vec<Candidate> {
    compose(catalog().expect("catalog"), t, seed)
}

fn request(c: &Candidate, name: &str, setup: SetupInput) -> ChooseRequest {
    ChooseRequest { look: c.look.clone(), starter_top: c.starter_top.clone(), name: name.into(), blurb: "함께 공부해요.".into(), input: setup }
}

fn service(h: &Harness) -> CharacterService {
    CharacterService::new(Arc::clone(&h.service), h.clock.clone())
}

#[test]
fn the_same_input_and_seed_give_the_same_candidates_and_a_reroll_differs() {
    let t = traits(Some("INFP"), Some(1999), Some(Gender::Female));
    assert_eq!(candidates(&t, 7), candidates(&t, 7));
    let rolls: Vec<Vec<Candidate>> = (0..6).map(|seed| candidates(&t, seed)).collect();
    let distinct = rolls.iter().filter(|r| **r != rolls[0]).count();
    assert!(distinct > 0, "re-rolling must give different candidates");
}

#[test]
fn three_or_four_distinct_valid_candidates_for_every_seed_and_every_input_skipped() {
    let cat = catalog().expect("catalog");
    for t in [traits(None, None, None), traits(Some("ESTJ"), Some(2001), Some(Gender::Male)), Traits::default()] {
        for seed in 0..60 {
            let list = candidates(&t, seed);
            assert!((3..=4).contains(&list.len()), "seed {seed}: {}", list.len());
            let species: HashSet<_> = list.iter().map(|c| c.look.species.clone()).collect();
            assert_eq!(species.len(), list.len(), "species are distinct");
            for c in &list {
                let s = cat.species(&c.look.species).expect("species");
                assert!(!s.shop_only);
                assert!(cat.palette(&c.look.palette).is_some() && cat.expression(&c.look.expression).is_some() && cat.prop(&c.look.prop).is_some());
            }
        }
    }
}

#[test]
fn every_input_is_skippable_and_bad_input_is_rejected() {
    assert_eq!(SetupInput::default().normalize(YEAR).expect("all skipped"), Traits::default());
    assert_eq!(input(Some("  "), None, None).normalize(YEAR).expect("blank is skipped").mbti, None);
    assert_eq!(input(Some("infp"), None, None).normalize(YEAR).expect("lower case").mbti.as_deref(), Some("INFP"));
    for bad in ["INF", "INFPX", "ABCD", "IXFP", "한국어네글"] {
        assert!(input(Some(bad), None, None).normalize(YEAR).is_err(), "{bad}");
    }
    for year in [1899, YEAR + 1, -5] {
        assert!(input(None, Some(year), None).normalize(YEAR).is_err(), "{year}");
    }
    assert!(input(None, Some(1900), None).normalize(YEAR).is_ok() && input(None, Some(YEAR), None).normalize(YEAR).is_ok());
}

#[test]
fn mbti_axes_pick_the_pools_and_the_zodiac_picks_the_first_species() {
    let cat = catalog().expect("catalog");
    for seed in 0..40 {
        for c in candidates(&traits(Some("ENTJ"), None, None), seed) {
            assert_eq!(cat.expression(&c.look.expression).expect("e").energy, super::catalog::Energy::High);
            assert_eq!(cat.palette(&c.look.palette).expect("p").temperature, super::catalog::Temperature::Cool);
            assert_eq!(cat.prop(&c.look.prop).expect("p").mind, super::catalog::Mind::Thinking);
        }
        for c in candidates(&traits(Some("ISFP"), None, None), seed) {
            assert_eq!(cat.expression(&c.look.expression).expect("e").energy, super::catalog::Energy::Low);
            assert_eq!(cat.palette(&c.look.palette).expect("p").temperature, super::catalog::Temperature::Warm);
            assert_eq!(cat.prop(&c.look.prop).expect("p").mind, super::catalog::Mind::Feeling);
        }
        // Rat 1996, rabbit 1999, dragon 2024.
        for (year, species) in [(1996, "squirrel"), (1999, "rabbit"), (2024, "owl")] {
            assert_eq!(candidates(&traits(None, Some(year), None), seed)[0].look.species, species, "{year}");
        }
    }
    for species in ZODIAC_SPECIES {
        assert!(cat.species(species).is_some_and(|s| !s.shop_only), "{species}");
    }
}

#[test]
fn gender_only_changes_the_free_starter_top() {
    let base = traits(Some("INFP"), Some(2000), None);
    let looks = |g| candidates(&traits(Some("INFP"), Some(2000), g), 3).into_iter().map(|c| c.look).collect::<Vec<_>>();
    assert_eq!(looks(Some(Gender::Female)), looks(None));
    assert_eq!(looks(Some(Gender::Male)), looks(Some(Gender::Other)));
    let tops = |g| candidates(&traits(Some("INFP"), Some(2000), g), 3)[0].starter_top.clone();
    assert_eq!((tops(Some(Gender::Female)).as_str(), tops(Some(Gender::Male)).as_str(), tops(None).as_str()), ("top_cardigan", "top_hoodie", "top_vest"));
    assert_eq!(candidates(&base, 3)[0].starter_top, "top_vest");
}

#[test]
fn the_mbti_tone_table_is_complete_and_never_harsh() {
    for ie in ['I', 'E'] {
        for sn in ['S', 'N'] {
            for tf in ['T', 'F'] {
                for jp in ['J', 'P'] {
                    let code: String = [ie, sn, tf, jp].iter().collect();
                    let expected = match (tf, jp) {
                        ('F', 'P') => Tone::Gentle,
                        ('F', 'J') => Tone::Warm,
                        ('T', 'P') => Tone::Normal,
                        _ => Tone::Firm,
                    };
                    assert_eq!(tone_for_mbti(&code), Some(expected), "{code}");
                }
            }
        }
    }
    assert_eq!(tone_for_mbti("infj"), Some(Tone::Warm));
    for bad in ["", "ABCD", "INF", "INFPP"] {
        assert_eq!(tone_for_mbti(bad), None, "{bad}");
    }
}

#[test]
fn the_catalogs_are_original_well_formed_and_render_without_leftovers() {
    let cat = catalog().expect("catalog");
    let shop = items::catalog().expect("items");
    let mut ids = HashSet::new();
    let id_ok = |id: &str| !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    for id in cat.species.iter().map(|s| &s.id).chain(cat.palettes.iter().map(|p| &p.id)).chain(cat.expressions.iter().map(|e| &e.id)).chain(cat.props.iter().map(|p| &p.id)) {
        assert!(id_ok(id), "{id}");
    }
    for item in &shop.items {
        assert!(id_ok(&item.id) && ids.insert(item.id.clone()), "{}", item.id);
        assert!(!is_banned(&item.label), "{}", item.label);
    }
    let labels: Vec<&str> = cat
        .species
        .iter()
        .map(|s| s.label.as_str())
        .chain(cat.palettes.iter().map(|p| p.label.as_str()))
        .chain(cat.expressions.iter().map(|e| e.label.as_str()))
        .chain(cat.props.iter().map(|p| p.label.as_str()))
        .chain(BUILTIN_NAMES)
        .collect();
    assert!(labels.iter().all(|l| !is_banned(l)), "an original name was needed");
    assert!(is_banned("토토로") && is_banned("지브리 스타일") && !is_banned("도토리"));
    for i in 0..BUILTIN_NAMES.len() {
        assert!(clean_name(&builtin_name(i as u64, i)).is_some());
    }
    for c in candidates(&traits(Some("INFP"), None, None), 1) {
        assert!(!is_banned(&builtin_blurb(cat, &c.look)) && clean_intro(&builtin_blurb(cat, &c.look)).is_some());
    }

    // Every species wears every clothing item with every placeholder filled.
    for species in &cat.species {
        let look = Look { species: species.id.clone(), palette: "honey".into(), expression: "smile".into(), prop: "book".into() };
        let mut worn = Equipped::default();
        for item in shop.items.iter().filter(|i| i.kind == ItemKind::Clothing) {
            match item.slot {
                Some(Slot::Hat) => worn.hat = Some(item.id.clone()),
                Some(Slot::Top) => worn.top = Some(item.id.clone()),
                Some(Slot::Accessory) => worn.accessory = Some(item.id.clone()),
                Some(Slot::Hand) => worn.hand = Some(item.id.clone()),
                None => panic!("{} has no slot", item.id),
            }
            for stage in [Stage::Seed, Stage::Sprout, Stage::SmallTree, Stage::ForestSpirit] {
                let svg = render::render(&look, stage, &worn).expect("render");
                assert!(!svg.contains('{') && !svg.contains('}'), "{} on {}: {svg}", item.id, species.id);
                assert!(svg.starts_with("<svg") && svg.ends_with("</svg>") && !svg.contains("<script"));
            }
        }
    }
}

#[test]
fn clothing_anchors_follow_each_body_so_one_item_fits_every_species() {
    let cat = catalog().expect("catalog");
    for species in &cat.species {
        let b = &species.body;
        let torso = render::torso(b);
        assert_eq!(render::anchor(b, Slot::Top), torso, "tops cover exactly the torso of {}", species.id);
        for slot in Slot::ALL {
            let a = render::anchor(b, slot);
            assert!(a.x >= 0.0 && a.y >= 0.0 && a.x + a.w <= 100.0 && a.y + a.h <= 100.0, "{slot:?} of {} leaves the picture: {a:?}", species.id);
        }
        let hat = render::anchor(b, Slot::Hat);
        assert!(hat.y < b.head_cy - b.head_r && hat.y + hat.h > b.head_cy - b.head_r, "hat straddles the top of the head of {}", species.id);
        let scarf = render::anchor(b, Slot::Accessory);
        assert!(scarf.y < torso.y + 3.0 && scarf.y + scarf.h > torso.y, "scarf sits at the neck of {}", species.id);
        assert!(render::anchor(b, Slot::Hand).x >= torso.x + torso.w, "hand items sit beside the torso of {}", species.id);
    }
}

#[test]
fn choosing_saves_locally_wears_the_starter_and_sets_the_tone_once() {
    let mut conn = open_in_memory().expect("db");
    let c = &candidates(&traits(Some("INFP"), Some(1999), Some(Gender::Female)), 1)[0];
    let out = choose(&mut conn, &request(c, "도토리", input(Some("infp"), Some(1999), Some(Gender::Female))), NOW, YEAR).expect("choose");
    assert_eq!(out.character.name, "도토리");
    assert_eq!(out.character.equipped.top.as_deref(), Some("top_cardigan"));
    assert_eq!((out.character.level, out.character.stage), (1, Stage::Seed));
    assert_eq!(out.tone_applied.as_deref(), Some("gentle"));
    assert_eq!(crate::messages::config::MessageConfig::load(&conn).expect("cfg").tone, Tone::Gentle);
    let stored: (Option<String>, Option<i64>, Option<String>) = conn
        .query_row("SELECT mbti, birth_year, gender FROM character_profile", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("row");
    assert_eq!(stored, (Some("INFP".into()), Some(1999), Some("female".into())));

    // A second pick with another MBTI does not touch the tone any more.
    let d = &candidates(&traits(Some("ESTJ"), None, None), 2)[0];
    let again = choose(&mut conn, &request(d, "보리", input(Some("ESTJ"), None, None)), NOW + 1, YEAR).expect("choose again");
    assert_eq!(again.tone_applied, None);
    assert_eq!(crate::messages::config::MessageConfig::load(&conn).expect("cfg").tone, Tone::Gentle);
    assert_eq!(again.character.name, "보리");
}

#[test]
fn a_tone_the_user_already_has_wins_and_a_locked_setting_is_skipped() {
    let mut conn = open_in_memory().expect("db");
    crate::messages::config::update_setting(&mut conn, crate::messages::config::MessageSettingKey::Tone, crate::settings::SettingValue::Text("normal".into()), 1).expect("user tone");
    let c = &candidates(&traits(Some("ENTJ"), None, None), 1)[0];
    let out = choose(&mut conn, &request(c, "하루", input(Some("ENTJ"), None, None)), NOW, YEAR).expect("choose");
    assert_eq!(out.tone_applied, None);
    assert_eq!(crate::messages::config::MessageConfig::load(&conn).expect("cfg").tone, Tone::Normal);

    let locked = open_in_memory().expect("db");
    set_lock_for_test(&locked, true);
    let mut locked = locked;
    let out = choose(&mut locked, &request(c, "하루", input(Some("ENTJ"), None, None)), NOW, YEAR).expect("choose while locked");
    assert_eq!(out.tone_applied, None, "a locked tone is not forced");
    assert_eq!(out.character.name, "하루");
}

#[test]
fn choosing_validates_every_id_and_the_name() {
    let mut conn = open_in_memory().expect("db");
    let c = candidates(&traits(None, None, None), 1)[0].clone();
    let go = |conn: &mut Connection, mutate: &dyn Fn(&mut ChooseRequest)| {
        let mut r = request(&c, "도토리", SetupInput::default());
        mutate(&mut r);
        choose(conn, &r, NOW, YEAR)
    };
    assert!(go(&mut conn, &|r| r.look.species = "dragon".into()).is_err());
    assert!(go(&mut conn, &|r| r.look.species = "raccoon".into()).is_err(), "a shop-only species needs its item");
    assert!(go(&mut conn, &|r| r.look.palette = "neon".into()).is_err());
    assert!(go(&mut conn, &|r| r.look.expression = "x".into()).is_err());
    assert!(go(&mut conn, &|r| r.look.prop = "x".into()).is_err());
    assert!(go(&mut conn, &|r| r.starter_top = "hat_beanie".into()).is_err(), "only a free starter top");
    assert!(go(&mut conn, &|r| r.starter_top = "top_stripe".into()).is_err());
    assert!(go(&mut conn, &|r| r.name = "  ".into()).is_err());
    assert!(go(&mut conn, &|r| r.name = "아주아주긴이름입니다".into()).is_err());
    assert!(go(&mut conn, &|r| r.name = "토토로".into()).is_err(), "protected names are refused");
    assert!(go(&mut conn, &|r| r.blurb = "가".repeat(41)).is_err());
    assert!(go(&mut conn, &|r| r.input.mbti = Some("ZZZZ".into())).is_err());
    assert!(view(&conn).expect("view").is_none(), "nothing was saved by a refused pick");
    assert!(go(&mut conn, &|_| {}).is_ok());
}

#[test]
fn rolling_again_never_changes_what_was_chosen() {
    let h = mock_harness();
    let svc = service(&h);
    let first = svc.candidates(&input(Some("INFP"), Some(1999), None), 0).expect("roll");
    let pick = &first.candidates[0];
    let before = {
        let mut conn = h.db.lock().expect("db");
        choose(
            &mut conn,
            &ChooseRequest { look: pick.look.clone(), starter_top: pick.starter_top.clone(), name: pick.name.clone(), blurb: pick.blurb.clone(), input: SetupInput::default() },
            NOW,
            YEAR,
        )
        .expect("choose")
        .character
    };
    for seed in 1..8 {
        svc.candidates(&input(Some("ENTP"), None, Some(Gender::Male)), seed).expect("roll");
    }
    let conn = h.db.lock().expect("db");
    assert_eq!(view(&conn).expect("view"), Some(before));
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM point_ledger", [], |r| r.get(0)).expect("count");
    assert_eq!(rows, 0);
}

#[test]
fn names_come_from_the_model_screened_per_candidate_with_a_builtin_fallback() {
    let h = mock_harness();
    let svc = service(&h);
    let set = svc.candidates(&input(None, None, None), 5).expect("roll");
    assert_eq!(set.fallback_reason, None);
    for (i, c) in set.candidates.iter().enumerate() {
        assert_eq!((c.name_source, c.name.as_str()), (NameSource::Llm, builtin_name(0, i).as_str()));
    }
    assert!((3..=4).contains(&set.candidates.len()));

    let n = set.candidates.len();
    let items = |first: &str| {
        let mut v = vec![serde_json::json!({"name": first, "intro": "좋은 친구예요."})];
        v.extend((1..n).map(|i| serde_json::json!({"name": format!("친구{i}"), "intro": "함께해요."})));
        serde_json::json!({ "items": v }).to_string()
    };
    h.remote.mock_llm().push(MockStep::Text(items("토토로")));
    let set = svc.candidates(&input(None, None, None), 5).expect("roll");
    assert_eq!(set.fallback_reason.as_deref(), Some("bad_answer"));
    assert_eq!(set.candidates[0].name_source, NameSource::Builtin);
    assert_eq!(set.candidates[0].name, builtin_name(5, 0));
    assert!(set.candidates[1..].iter().all(|c| c.name_source == NameSource::Llm));

    for (step, code) in [
        (MockStep::Text("그냥 문장".into()), "bad_answer"),
        (MockStep::Fail(LlmError::Disabled), "llm_disabled"),
        (MockStep::Fail(LlmError::NotConfigured), "llm_not_configured"),
        (MockStep::Fail(LlmError::Overloaded), "llm_overloaded"),
    ] {
        h.remote.mock_llm().push(step);
        let set = svc.candidates(&input(None, None, None), 5).expect("roll never fails on the model");
        assert_eq!(set.fallback_reason.as_deref(), Some(code));
        assert!(set.candidates.iter().all(|c| c.name_source == NameSource::Builtin && clean_name(&c.name).is_some()));
    }
}

#[test]
fn the_model_prompt_carries_part_labels_and_never_the_setup_answers() {
    let h = mock_harness();
    let svc = service(&h);
    let set = svc.candidates(&input(Some("ESTJ"), Some(1987), Some(Gender::Female)), 3).expect("roll");
    let calls = h.remote.mock_llm().calls();
    assert_eq!(calls.len(), 1);
    let text = format!("{} {}", calls[0].system, calls[0].user);
    for secret in ["ESTJ", "estj", "1987", "female", "Female", "여성", "남성", "띠"] {
        assert!(!text.contains(secret), "{secret} leaked into the prompt: {text}");
    }
    assert!(text.contains(&set.candidates[0].labels.species) && text.contains(&set.candidates[0].labels.prop));
    // Nor does any candidate or view field carry them.
    let wire = serde_json::to_string(&set).expect("json");
    assert!(!wire.contains("ESTJ") && !wire.contains("1987"));
    assert!(wire.contains("\"svg\"") && !wire.contains("mbti") && !wire.contains("birth"));
}

#[test]
fn the_view_never_exposes_the_setup_answers() {
    let mut conn = open_in_memory().expect("db");
    let c = &candidates(&traits(Some("INTJ"), Some(1993), Some(Gender::Male)), 1)[0];
    let out = choose(&mut conn, &request(c, "솔이", input(Some("INTJ"), Some(1993), Some(Gender::Male))), NOW, YEAR).expect("choose");
    let wire = serde_json::to_string(&out).expect("json");
    for secret in ["INTJ", "1993", "male", "mbti", "birth", "gender"] {
        assert!(!wire.contains(secret), "{secret}: {wire}");
    }
}

#[test]
fn the_growth_stage_changes_exactly_at_the_level_boundaries() {
    let mut conn = open_in_memory().expect("db");
    let c = &candidates(&traits(None, None, None), 1)[0];
    choose(&mut conn, &request(c, "나루", SetupInput::default()), NOW, YEAR).expect("choose");
    // (earned points, level, stage): level 3 starts at 50, 6 at 200, 9 at 500.
    let table = [(0, 1, Stage::Seed), (49, 2, Stage::Seed), (50, 3, Stage::Sprout), (199, 5, Stage::Sprout), (200, 6, Stage::SmallTree), (499, 8, Stage::SmallTree), (500, 9, Stage::ForestSpirit)];
    let mut paid = 0;
    for (earned, level, stage) in table {
        if earned > paid {
            crate::store::insert_ledger_entry(&conn, 1, earned - paid, "test", &format!("t{earned}")).expect("ledger");
            paid = earned;
        }
        let v = view(&conn).expect("view").expect("character");
        assert_eq!((v.level, v.stage), (level, stage), "earned {earned}");
    }
}

#[test]
fn the_menu_bar_line_names_the_stage_the_name_and_the_level() {
    let mut conn = open_in_memory().expect("db");
    assert!(view(&conn).expect("view").is_none());
    let c = &candidates(&traits(None, None, None), 1)[0];
    let v = choose(&mut conn, &request(c, "도토리", SetupInput::default()), NOW, YEAR).expect("choose").character;
    assert_eq!(tray_line(&v), "🌰 도토리 · Lv.1 씨앗");
}

#[test]
fn the_migration_checks_match_the_rust_enums() {
    let conn = open_in_memory().expect("db");
    for gender in [Gender::Male, Gender::Female, Gender::Other] {
        conn.execute(
            "INSERT INTO character_profile (id, species, palette, expression, prop, name, blurb, gender, chosen_at_ms) VALUES (1, 'fox', 'honey', 'smile', 'book', 'n', '', ?1, 0)",
            [gender.as_str()],
        )
        .expect("gender accepted");
        conn.execute("DELETE FROM character_profile", []).expect("clear");
    }
    assert!(conn.execute("INSERT INTO character_profile (id, species, palette, expression, prop, name, blurb, gender, chosen_at_ms) VALUES (1, 'fox', 'honey', 'smile', 'book', 'n', '', 'x', 0)", []).is_err());
    assert!(conn.execute("INSERT INTO character_profile (id, species, palette, expression, prop, name, blurb, mbti, chosen_at_ms) VALUES (1, 'fox', 'honey', 'smile', 'book', 'n', '', 'infp', 0)", []).is_err(), "mbti is stored upper case");
    assert!(conn.execute("INSERT INTO character_profile (id, species, palette, expression, prop, name, blurb, chosen_at_ms) VALUES (2, 'fox', 'honey', 'smile', 'book', 'n', '', 0)", []).is_err(), "one character only");
    for slot in Slot::ALL {
        conn.execute("INSERT INTO wardrobe (slot, item_id) VALUES (?1, 'x')", [slot.as_str()]).expect("slot accepted");
    }
    assert!(conn.execute("INSERT INTO wardrobe (slot, item_id) VALUES ('furniture', 'x')", []).is_err());
}
