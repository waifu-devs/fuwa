//! Profile effects and avatar decorations an instance or a server offers
//! (docs/profile-items.md). The instance's are in node.db's `profile_items`,
//! a server's in its own file's, in the same shape, so the reads and writes
//! here serve both. `api/profile_items.rs` is `ProfileItemService`.
//!
//! An effect is a spec (docs/profile-effects.md, "The format"): data apps
//! play, never code or pictures. [`check_effect`] holds it to the format's
//! lists and ranges before it's stored, so every app gets one it can play;
//! apps check it again anyway (`sanitizeEffect` on the web).

use serde_json::{Map, Value, json};
use turso::{Connection, Row};

use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::timestamp;
use crate::pb;

/// The largest spec taken, as JSON.
pub const MAX_EFFECT_BYTES: usize = 16 * 1024;

const SHAPES: &[&str] = &["petal", "star", "sparkle", "heart", "snowflake", "bubble", "dot", "confetti", "streak"];
const MOTIONS: &[&str] = &["fall", "rise", "twinkle", "drift", "burst", "shoot", "pop"];
const REGIONS: &[&str] = &["top", "bottom", "edges", "corners", "anywhere", "top-left", "top-right", "center"];
const PHASES: &[&str] = &["intro", "idle"];
const PAINT_TOKENS: &[&str] = &["primary", "ring", "accent", "foreground", "card", "profile"];
const MAX_LAYERS: usize = 6;
const MAX_PER_LAYER: f64 = 24.0;
const MAX_COLORS: usize = 8;

/// A name: 1 to 40 characters.
pub fn checked_name(name: &str) -> Result<String> {
    let name = name.trim();
    let count = name.chars().count();
    if count == 0 || count > 40 {
        return Err(Error::invalid("a profile item's name is 1 to 40 characters"));
    }
    Ok(name.to_string())
}

/// A description: up to 120 characters.
pub fn checked_description(description: &str) -> Result<String> {
    let description = description.trim();
    if description.chars().count() > 120 {
        return Err(Error::invalid("a profile item's description is up to 120 characters"));
    }
    Ok(description.to_string())
}

fn layer_error(index: usize, what: &str) -> Error {
    Error::invalid(format!("the effect's layer {}: {what}", index + 1))
}

fn one_of(layer: &Map<String, Value>, field: &str, options: &[&str], index: usize) -> Result<Value> {
    match layer.get(field).and_then(Value::as_str) {
        Some(value) if options.contains(&value) => Ok(json!(value)),
        _ => Err(layer_error(index, &format!("{field} is one of {}", options.join(", ")))),
    }
}

fn number(value: &Value, min: f64, max: f64) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite() && (min..=max).contains(n))
}

fn optional_number(layer: &Map<String, Value>, field: &str, min: f64, max: f64, index: usize) -> Result<Option<Value>> {
    match layer.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => number(value, min, max)
            .map(|n| Some(json!(n)))
            .ok_or_else(|| layer_error(index, &format!("{field} is a number from {min} to {max}"))),
    }
}

fn range(layer: &Map<String, Value>, field: &str, min: f64, max: f64, index: usize) -> Result<Option<Value>> {
    let error = || layer_error(index, &format!("{field} is two numbers from {min} to {max}"));
    match layer.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(pair)) if pair.len() == 2 => {
            let a = number(&pair[0], min, max).ok_or_else(error)?;
            let b = number(&pair[1], min, max).ok_or_else(error)?;
            Ok(Some(json!([a.min(b), a.max(b)])))
        }
        Some(_) => Err(error()),
    }
}

fn paint(value: &Value) -> bool {
    let Some(value) = value.as_str() else { return false };
    PAINT_TOKENS.contains(&value)
        || (value.len() == 7 && value.starts_with('#') && value[1..].bytes().all(|b| b.is_ascii_hexdigit()))
}

/// An effect's spec, checked against the format and written out again with
/// only what the format has, under the item's id (in lowercase, so it's an
/// effect id apps take), name and description.
pub fn check_effect(json: &str, id: &str, name: &str, description: &str) -> Result<String> {
    if json.len() > MAX_EFFECT_BYTES {
        return Err(Error::invalid("an effect's spec is at most 16 KB"));
    }
    let spec: Value = serde_json::from_str(json).map_err(|_| Error::invalid("an effect's spec is JSON"))?;
    let layers = spec
        .get("layers")
        .and_then(Value::as_array)
        .filter(|layers| (1..=MAX_LAYERS).contains(&layers.len()))
        .ok_or_else(|| Error::invalid("an effect has 1 to 6 layers"))?;
    let mut checked = Vec::with_capacity(layers.len());
    for (index, layer) in layers.iter().enumerate() {
        let layer = layer.as_object().ok_or_else(|| layer_error(index, "a layer is an object"))?;
        let mut out = Map::new();
        out.insert("shape".into(), one_of(layer, "shape", SHAPES, index)?);
        out.insert("motion".into(), one_of(layer, "motion", MOTIONS, index)?);
        out.insert("phase".into(), one_of(layer, "phase", PHASES, index)?);
        let count = layer
            .get("count")
            .and_then(|count| number(count, 1.0, MAX_PER_LAYER))
            .ok_or_else(|| layer_error(index, "count is from 1 to 24"))?;
        out.insert("count".into(), json!(count.round() as i64));
        out.insert("from".into(), one_of(layer, "from", REGIONS, index)?);
        let size = range(layer, "size", 2.0, 96.0, index)?.ok_or_else(|| layer_error(index, "size is needed"))?;
        out.insert("size".into(), size);
        let duration =
            range(layer, "duration", 300.0, 20000.0, index)?.ok_or_else(|| layer_error(index, "duration is needed"))?;
        out.insert("duration".into(), duration);
        if let Some(delay) = range(layer, "delay", 0.0, 3000.0, index)? {
            out.insert("delay".into(), delay);
        }
        let colors = layer
            .get("colors")
            .and_then(Value::as_array)
            .filter(|colors| (1..=MAX_COLORS).contains(&colors.len()) && colors.iter().all(paint))
            .ok_or_else(|| {
                layer_error(index, "colors are 1 to 8 of primary, ring, accent, foreground, card, profile or #rrggbb")
            })?;
        out.insert("colors".into(), Value::Array(colors.clone()));
        for (field, min, max) in [("spin", -1440.0, 1440.0), ("sway", 0.0, 400.0), ("opacity", 0.05, 1.0)] {
            if let Some(value) = optional_number(layer, field, min, max, index)? {
                out.insert(field.into(), value);
            }
        }
        for field in ["flip", "glow"] {
            match layer.get(field) {
                None | Some(Value::Null) | Some(Value::Bool(false)) => {}
                Some(Value::Bool(true)) => {
                    out.insert(field.into(), json!(true));
                }
                Some(_) => return Err(layer_error(index, &format!("{field} is true or false"))),
            }
        }
        checked.push(Value::Object(out));
    }
    let spec = json!({
        "id": id.to_lowercase(),
        "name": name,
        "description": description,
        "layers": checked,
    });
    Ok(spec.to_string())
}

const COLUMNS: &str = "id, kind, name, description, effect, picture_url, animated, creator_id, size, created_at";

fn item_row(server_id: &str) -> impl Fn(&Row) -> turso::Result<pb::ProfileItem> + '_ {
    move |r| {
        Ok(pb::ProfileItem {
            id: r.get(0)?,
            server_id: server_id.to_string(),
            kind: r.get(1)?,
            name: r.get(2)?,
            description: r.get(3)?,
            effect: r.get(4)?,
            picture_url: r.get(5)?,
            animated: r.get(6)?,
            creator_id: r.get(7)?,
            size: r.get(8)?,
            created_at: Some(timestamp(r.get(9)?)),
        })
    }
}

/// Every item in a file, oldest first. `server_id` is empty for node.db.
pub async fn list(conn: &Connection, server_id: &str) -> Result<Vec<pb::ProfileItem>> {
    query_all(conn, &format!("SELECT {COLUMNS} FROM profile_items ORDER BY created_at, id"), (), item_row(server_id))
        .await
}

pub async fn get(conn: &Connection, server_id: &str, id: &str) -> Result<Option<pb::ProfileItem>> {
    query_one(conn, &format!("SELECT {COLUMNS} FROM profile_items WHERE id = ?1"), [id], item_row(server_id)).await
}

/// Whether the file has an item of this kind with this id.
pub async fn has(conn: &Connection, id: &str, kind: pb::ProfileItemKind) -> Result<bool> {
    Ok(query_one(conn, "SELECT 1 FROM profile_items WHERE id = ?1 AND kind = ?2", (id, kind as i64), |r| {
        r.get::<i64>(0)
    })
    .await?
    .is_some())
}

pub async fn insert(conn: &Connection, item: &pb::ProfileItem) -> Result<()> {
    conn.execute(
        &format!("INSERT INTO profile_items ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"),
        (
            item.id.as_str(),
            item.kind as i64,
            item.name.as_str(),
            item.description.as_str(),
            item.effect.as_str(),
            item.picture_url.as_str(),
            item.animated,
            item.creator_id.as_str(),
            item.size,
            item.created_at.as_ref().map(crate::id::millis).unwrap_or_default(),
        ),
    )
    .await?;
    Ok(())
}

pub async fn save(conn: &Connection, item: &pb::ProfileItem) -> Result<()> {
    conn.execute(
        "UPDATE profile_items SET name = ?2, description = ?3, effect = ?4 WHERE id = ?1",
        (item.id.as_str(), item.name.as_str(), item.description.as_str(), item.effect.as_str()),
    )
    .await?;
    Ok(())
}

pub async fn delete(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM profile_items WHERE id = ?1", [id]).await?;
    Ok(())
}

/// What a new item is, checked: its kind, name, description and effect spec
/// (for an effect), under the id it will have. A decoration's picture is the
/// caller's to check, since where it may come from depends on who offers it.
pub fn checked_new(new: &pb::NewProfileItem, id: &str) -> Result<pb::ProfileItem> {
    let kind = match pb::ProfileItemKind::try_from(new.kind) {
        Ok(kind @ (pb::ProfileItemKind::Effect | pb::ProfileItemKind::Decoration)) => kind,
        _ => return Err(Error::invalid("a profile item is an effect or a decoration")),
    };
    let name = checked_name(&new.name)?;
    let description = checked_description(&new.description)?;
    let effect = match kind {
        pb::ProfileItemKind::Effect => check_effect(&new.effect, id, &name, &description)?,
        _ if !new.effect.is_empty() => return Err(Error::invalid("only effects have a spec")),
        _ => String::new(),
    };
    if kind == pb::ProfileItemKind::Effect && !new.picture_url.is_empty() {
        return Err(Error::invalid("only decorations have a picture"));
    }
    Ok(pb::ProfileItem { id: id.to_string(), kind: kind as i32, name, description, effect, ..Default::default() })
}

/// `item` with `change` applied and checked. Whether anything changed.
pub fn apply(item: &mut pb::ProfileItem, change: &pb::ProfileItemChange) -> Result<bool> {
    let before = item.clone();
    if let Some(name) = &change.name {
        item.name = checked_name(name)?;
    }
    if let Some(description) = &change.description {
        item.description = checked_description(description)?;
    }
    let effect = item.kind == pb::ProfileItemKind::Effect as i32;
    match (&change.effect, effect) {
        (Some(_), false) => return Err(Error::invalid("only effects have a spec")),
        (Some(spec), true) => item.effect = check_effect(spec, &item.id, &item.name, &item.description)?,
        // The spec carries the name and description too, so they stay in step.
        (None, true) => item.effect = check_effect(&item.effect, &item.id, &item.name, &item.description)?,
        (None, false) => {}
    }
    Ok(*item != before)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAKURA: &str = r##"{"id":"whatever","name":"x","layers":[
        {"shape":"petal","motion":"burst","phase":"intro","count":18,"from":"top-left","size":[28,18],
         "duration":[1500,2300],"delay":[0,260],"colors":["#ffb7c5","primary"],"spin":540,"sway":280,
         "flip":true,"opacity":0.95,"extra":"dropped"}]}"##;

    #[test]
    fn a_good_spec_comes_back_tidied_under_the_item() {
        let spec = check_effect(SAKURA, "01ABCDEF", "Petals", "Pink").unwrap();
        let value: Value = serde_json::from_str(&spec).unwrap();
        assert_eq!(value["id"], "01abcdef");
        assert_eq!(value["name"], "Petals");
        assert_eq!(value["description"], "Pink");
        let layer = &value["layers"][0];
        assert_eq!(layer["size"], json!([18.0, 28.0]));
        assert_eq!(layer["count"], 18);
        assert_eq!(layer["flip"], true);
        assert!(layer.get("extra").is_none());
    }

    #[test]
    fn specs_outside_the_format_are_refused() {
        let bad = [
            "not json",
            r#"{"layers":[]}"#,
            r#"{"layers":[{"shape":"cube","motion":"fall","phase":"idle","count":3,"from":"top","size":[2,3],"duration":[300,400],"colors":["primary"]}]}"#,
            r#"{"layers":[{"shape":"dot","motion":"fall","phase":"idle","count":300,"from":"top","size":[2,3],"duration":[300,400],"colors":["primary"]}]}"#,
            r#"{"layers":[{"shape":"dot","motion":"fall","phase":"idle","count":3,"from":"top","size":[2,3],"duration":[300,400],"colors":["url(x)"]}]}"#,
            r#"{"layers":[{"shape":"dot","motion":"fall","phase":"idle","count":3,"from":"top","size":[2,3],"duration":[300,99999],"colors":["primary"]}]}"#,
            r#"{"layers":[{"shape":"dot","motion":"fall","phase":"idle","count":3,"from":"top","size":[2,3],"duration":[300,400],"colors":["primary"],"glow":"yes"}]}"#,
        ];
        for spec in bad {
            assert!(check_effect(spec, "01A", "n", "").is_err(), "{spec}");
        }
        let huge = format!(r#"{{"layers":[],"pad":"{}"}}"#, "x".repeat(MAX_EFFECT_BYTES));
        assert!(check_effect(&huge, "01A", "n", "").is_err());
    }

    #[test]
    fn new_items_are_one_kind_or_the_other() {
        let effect = pb::NewProfileItem {
            kind: pb::ProfileItemKind::Effect as i32,
            name: "Petals".into(),
            effect: SAKURA.into(),
            ..Default::default()
        };
        assert!(checked_new(&effect, "01A").is_ok());
        assert!(checked_new(&pb::NewProfileItem { picture_url: "x".into(), ..effect.clone() }, "01A").is_err());
        let decoration = pb::NewProfileItem {
            kind: pb::ProfileItemKind::Decoration as i32,
            name: "Ring".into(),
            ..Default::default()
        };
        assert!(checked_new(&decoration, "01A").is_ok());
        assert!(checked_new(&pb::NewProfileItem { effect: SAKURA.into(), ..decoration }, "01A").is_err());
        assert!(checked_new(&pb::NewProfileItem { name: " ".into(), ..effect }, "01A").is_err());
    }
}
