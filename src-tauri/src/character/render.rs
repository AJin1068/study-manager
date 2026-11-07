//! Placeholder SVG rendering. The body of each species has fixed proportions in the catalog;
//! every clothing slot has an anchor box derived from them, so one item template fits all
//! species ("body-proportion alignment"). Pure: the same inputs give the same string.

use super::catalog::{Body, catalog};
use super::{Equipped, Look};
use crate::error::{AppError, AppResult};
use crate::points::rewards::Stage;
use crate::shop::items::{self, ItemKind, Slot};

const CX: f64 = 50.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

fn f(v: f64) -> String {
    format!("{v:.1}")
}

pub fn torso(body: &Body) -> Rect {
    Rect {
        x: CX - body.torso_w / 2.0,
        y: body.head_cy + body.head_r - 3.0,
        w: body.torso_w,
        h: body.torso_h,
    }
}

/// Where an item of `slot` sits for a body.
pub fn anchor(body: &Body, slot: Slot) -> Rect {
    let t = torso(body);
    let r = body.head_r;
    match slot {
        Slot::Hat => Rect {
            x: CX - r * 1.1,
            y: body.head_cy - r * 1.35,
            w: r * 2.2,
            h: r * 1.05,
        },
        Slot::Top => t,
        Slot::Accessory => Rect {
            x: t.x - 1.0,
            y: t.y - 2.0,
            w: t.w + 2.0,
            h: 7.0,
        },
        Slot::Hand => Rect {
            x: t.x + t.w + 1.0,
            y: t.y + t.h * 0.35,
            w: 11.0,
            h: 11.0,
        },
    }
}

fn fill(template: &str, vars: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (key, value) in vars {
        out = out.replace(&format!("{{{key}}}"), value);
    }
    out
}

fn anchor_vars(a: Rect) -> Vec<(&'static str, String)> {
    vec![
        ("x", f(a.x)),
        ("y", f(a.y)),
        ("w", f(a.w)),
        ("h", f(a.h)),
        ("xr", f(a.x + a.w)),
        ("yb", f(a.y + a.h)),
        ("cx", f(a.x + a.w / 2.0)),
        ("cy", f(a.y + a.h / 2.0)),
        ("hw", f(a.w * 0.6)),
        ("hh", f(a.h * 0.5)),
        ("xq", f(a.x + a.w * 0.3)),
        ("xq3", f(a.x + a.w * 0.7)),
        ("x4", f(a.x + a.w * 0.1)),
        ("xr4", f(a.x + a.w * 0.9)),
        ("hcx", f(a.x + a.w / 2.0)),
        ("hcy", f(a.y + a.h / 2.0)),
        ("hrx", f(a.w * 0.35)),
        ("hry", f(a.h * 0.5)),
    ]
}

fn head_vars(body: &Body, fur: &str) -> Vec<(&'static str, String)> {
    let (cy, r) = (body.head_cy, body.head_r);
    let t = torso(body);
    vec![
        ("fur", fur.to_string()),
        ("cx", f(CX)),
        ("cy", f(cy)),
        ("r", f(r)),
        ("d", f(r * 2.0)),
        ("top", f(cy - r)),
        ("bottom", f(cy + r)),
        ("left", f(CX - r)),
        ("right", f(CX + r)),
        ("lq", f(CX - r * 0.5)),
        ("rq", f(CX + r * 0.5)),
        ("tt", f(cy - r * 1.45)),
        ("tm", f(cy - r * 0.85)),
        ("r3", f(r * 0.3)),
        ("r4", f(r * 0.4)),
        ("r6", f(r * 0.6)),
        ("m1", f(cy + r * 0.45)),
        ("m2", f(cy + r * 0.8)),
        ("br", f(cy - r * 0.35)),
        ("tx", f(t.x)),
        ("ty", f(t.y)),
        ("tw", f(t.w)),
        ("th", f(t.h)),
        ("tr", f(t.x + t.w)),
        ("tb", f(t.y + t.h)),
    ]
}

fn scale(stage: Stage) -> f64 {
    match stage {
        Stage::Seed => 0.7,
        Stage::Sprout => 0.8,
        Stage::SmallTree => 0.9,
        Stage::ForestSpirit => 1.0,
    }
}

/// The SVG of a character at a growth stage wearing `equipped`. Equipped ids that the catalog no
/// longer has are skipped.
pub fn render(look: &Look, stage: Stage, equipped: &Equipped) -> AppResult<String> {
    let cat = catalog()?;
    let bad = |what: &str| AppError::Internal(format!("character look names an unknown {what}"));
    let species = cat.species(&look.species).ok_or_else(|| bad("species"))?;
    let palette = cat.palette(&look.palette).ok_or_else(|| bad("palette"))?;
    let expression = cat
        .expression(&look.expression)
        .ok_or_else(|| bad("expression"))?;
    let prop = cat.prop(&look.prop).ok_or_else(|| bad("prop"))?;
    let body = &species.body;
    let vars = head_vars(body, &palette.hex);
    let t = torso(body);
    let hand = anchor(body, Slot::Hand);

    let mut g = String::new();
    if stage == Stage::ForestSpirit {
        g.push_str("<circle cx='50' cy='55' r='46' fill='#cfe8c4' opacity='0.45'/>");
    }
    g.push_str(&fill(&species.svg, &vars));
    g.push_str(&format!(
        "<rect x='{}' y='{}' width='{}' height='{}' rx='8' fill='{}'/>",
        f(t.x),
        f(t.y),
        f(t.w),
        f(t.h),
        palette.hex
    ));
    g.push_str(&format!(
        "<circle cx='50' cy='{}' r='{}' fill='{}'/>",
        f(body.head_cy),
        f(body.head_r),
        palette.hex
    ));
    g.push_str(&fill(&expression.svg, &vars));
    let leaves = match stage {
        Stage::Seed => 0,
        Stage::Sprout => 1,
        Stage::SmallTree | Stage::ForestSpirit => 2,
    };
    let top = body.head_cy - body.head_r;
    for i in 0..leaves {
        let dx = if i == 0 { -3.0 } else { 4.0 };
        g.push_str(&format!(
            "<ellipse cx='{}' cy='{}' rx='3' ry='5' fill='#5fa35a'/>",
            f(CX + dx),
            f(top - 5.0 - f64::from(i))
        ));
    }
    // The prop is the base of the hand slot; a hand item is drawn over it.
    g.push_str(&fill(&prop.svg, &anchor_vars(hand)));
    for slot in [Slot::Top, Slot::Accessory, Slot::Hand, Slot::Hat] {
        let Some(id) = equipped.get(slot) else {
            continue;
        };
        let Some(item) = items::item(id)? else {
            continue;
        };
        if item.kind != ItemKind::Clothing || item.slot != Some(slot) {
            continue;
        }
        g.push_str(&fill(&item.svg, &anchor_vars(anchor(body, slot))));
    }
    let s = scale(stage);
    Ok(format!(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><g transform='translate(50 96) scale({s}) translate(-50 -96)'>{g}</g></svg>"
    ))
}
