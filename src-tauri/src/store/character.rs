//! `character_profile` and `wardrobe`. The profile row holds the user's setup answers; no reader
//! other than the character module is meant to expose them.

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::AppResult;

pub struct ProfileRow {
    pub species: String,
    pub palette: String,
    pub expression: String,
    pub prop: String,
    pub name: String,
    pub blurb: String,
    pub mbti: Option<String>,
    pub birth_year: Option<i32>,
    pub gender: Option<String>,
    pub chosen_at_ms: i64,
}

/// What the view needs; the setup answers are deliberately not read back.
pub struct ProfileLook {
    pub species: String,
    pub palette: String,
    pub expression: String,
    pub prop: String,
    pub name: String,
    pub blurb: String,
    pub chosen_at_ms: i64,
}

pub fn get_profile(conn: &Connection) -> AppResult<Option<ProfileLook>> {
    Ok(conn
        .query_row(
            "SELECT species, palette, expression, prop, name, blurb, chosen_at_ms FROM character_profile WHERE id = 1",
            [],
            |r| {
                Ok(ProfileLook {
                    species: r.get(0)?,
                    palette: r.get(1)?,
                    expression: r.get(2)?,
                    prop: r.get(3)?,
                    name: r.get(4)?,
                    blurb: r.get(5)?,
                    chosen_at_ms: r.get(6)?,
                })
            },
        )
        .optional()?)
}

pub fn save_profile(conn: &Connection, p: &ProfileRow) -> AppResult<()> {
    conn.execute(
        "INSERT INTO character_profile (id, species, palette, expression, prop, name, blurb, mbti, birth_year, gender, chosen_at_ms)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (id) DO UPDATE SET species = excluded.species, palette = excluded.palette,
           expression = excluded.expression, prop = excluded.prop, name = excluded.name, blurb = excluded.blurb,
           mbti = excluded.mbti, birth_year = excluded.birth_year, gender = excluded.gender,
           chosen_at_ms = excluded.chosen_at_ms",
        params![p.species, p.palette, p.expression, p.prop, p.name, p.blurb, p.mbti, p.birth_year, p.gender, p.chosen_at_ms],
    )?;
    Ok(())
}

/// Switches the species of the saved character. `false` when there is no character yet.
pub fn set_species(conn: &Connection, species: &str) -> AppResult<bool> {
    Ok(conn.execute(
        "UPDATE character_profile SET species = ?1 WHERE id = 1",
        params![species],
    )? == 1)
}

pub fn wardrobe(conn: &Connection) -> AppResult<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT slot, item_id FROM wardrobe ORDER BY slot")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn set_wardrobe(conn: &Connection, slot: &str, item_id: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO wardrobe (slot, item_id) VALUES (?1, ?2) ON CONFLICT (slot) DO UPDATE SET item_id = excluded.item_id",
        params![slot, item_id],
    )?;
    Ok(())
}

pub fn clear_wardrobe(conn: &Connection, slot: &str) -> AppResult<()> {
    conn.execute("DELETE FROM wardrobe WHERE slot = ?1", params![slot])?;
    Ok(())
}
