//! Profile effects, avatar decorations and nameplates an instance or a server offers
//! (docs/profile-items.md). The instance's are in node.db's `profile_items`,
//! a server's in its own file's, in the same shape, so the reads and writes
//! here serve both. `api/profile_items.rs` is `ProfileItemService`.
//!
//! An effect is a spec (docs/profile-effects.md, "The format"): layers of
//! particles apps play, which may be the item's own pictures, and a shader
//! (WGSL) under them, which apps compile and time themselves.
//! [`check_effect`] holds it to the format's lists and ranges, and the shader
//! to what it may use, before it's stored, so every app gets one it can play;
//! apps check it again anyway (`sanitizeEffect` on the web).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use turso::{Connection, Row};

use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::timestamp;
use crate::pb;

/// The largest spec taken, as JSON: room for a whole shader and six layers.
pub const MAX_EFFECT_BYTES: usize = 40 * 1024;
/// The largest shader, in bytes of WGSL: the apps' own cap for custom shaders.
pub const MAX_SHADER_BYTES: usize = 16 * 1024;
/// The most pictures an effect has.
pub const MAX_PICTURES: usize = 8;

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
/// effect id apps take), name and description. `pictures` is how many the
/// item has, which its layers name by place.
pub fn check_effect(json: &str, id: &str, name: &str, description: &str, pictures: usize) -> Result<String> {
    if json.len() > MAX_EFFECT_BYTES {
        return Err(Error::invalid("an effect's spec is at most 40 KB"));
    }
    let spec: Value = serde_json::from_str(json).map_err(|_| Error::invalid("an effect's spec is JSON"))?;
    let shader = match spec.get("shader") {
        None | Some(Value::Null) => None,
        Some(shader) => Some(check_shader(shader)?),
    };
    // A shader alone is an effect; without one, it takes a layer.
    let fewest = if shader.is_some() { 0 } else { 1 };
    let layers = match spec.get("layers") {
        None | Some(Value::Null) if shader.is_some() => &Vec::new(),
        layers => layers
            .and_then(Value::as_array)
            .filter(|layers| (fewest..=MAX_LAYERS).contains(&layers.len()))
            .ok_or_else(|| Error::invalid("an effect has 1 to 6 layers, or a shader and up to 6"))?,
    };
    let mut checked = Vec::with_capacity(layers.len());
    for (index, layer) in layers.iter().enumerate() {
        let layer = layer.as_object().ok_or_else(|| layer_error(index, "a layer is an object"))?;
        let mut out = Map::new();
        // A layer draws one of the item's pictures, or a shape.
        let image = match layer.get("image") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let at = value.as_u64().filter(|&at| (at as usize) < pictures).ok_or_else(|| match pictures {
                    0 => layer_error(index, "image names one of the effect's pictures, and it has none"),
                    n => layer_error(index, &format!("image is a picture's place, from 0 to {}", n - 1)),
                })?;
                out.insert("image".into(), json!(at));
                Some(at)
            }
        };
        if image.is_none() {
            out.insert("shape".into(), one_of(layer, "shape", SHAPES, index)?);
        }
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
        // A picture has its own colors: a layer of them may leave colors out (its glow is then the theme's).
        let fewest = if image.is_some() { 0 } else { 1 };
        let colors = match layer.get("colors") {
            None | Some(Value::Null) if image.is_some() => Some(Vec::new()),
            colors => colors
                .and_then(Value::as_array)
                .filter(|colors| (fewest..=MAX_COLORS).contains(&colors.len()) && colors.iter().all(paint))
                .cloned(),
        }
        .ok_or_else(|| {
            layer_error(index, "colors are 1 to 8 of primary, ring, accent, foreground, card, profile or #rrggbb")
        })?;
        if !colors.is_empty() {
            out.insert("colors".into(), Value::Array(colors));
        }
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
    let mut spec = json!({
        "id": id.to_lowercase(),
        "name": name,
        "description": description,
        "layers": checked,
    });
    if let Some(shader) = shader {
        spec["shader"] = shader;
    }
    Ok(spec.to_string())
}

fn line_break(c: char) -> bool {
    matches!(c, '\n' | '\u{b}' | '\u{c}' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// WGSL without its comments, read as the compiler reads it: `//` runs to
/// the next line break, and block comments nest. The apps' `stripComments`.
fn strip_comments(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let mut rest = code;
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("//") {
            rest = &after[after.find(line_break).unwrap_or(after.len())..];
            continue;
        }
        if let Some(after) = rest.strip_prefix("/*") {
            let mut depth = 1;
            rest = after;
            while depth > 0 && !rest.is_empty() {
                if let Some(after) = rest.strip_prefix("/*") {
                    depth += 1;
                    rest = after;
                } else if let Some(after) = rest.strip_prefix("*/") {
                    depth -= 1;
                    rest = after;
                } else {
                    rest = &rest[rest.chars().next().map_or(1, char::len_utf8)..];
                }
            }
            out.push(' ');
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Where `word` stands as a whole word.
fn words<'a>(text: &'a str, word: &'a str) -> impl Iterator<Item = usize> + 'a {
    text.match_indices(word).map(|(at, _)| at).filter(move |&at| {
        let before = text[..at].chars().next_back().is_none_or(|c| !word_char(c));
        let after = text[at + word.len()..].chars().next().is_none_or(|c| !word_char(c));
        before && after
    })
}

/// What's wrong with a shader's WGSL, by the rules every app holds custom
/// shaders to (docs/themes.md): it defines `fn shade(uv: vec2f) -> vec4f`
/// over the apps' inputs and nothing else, so no attributes (no bindings,
/// textures or entry points of its own), no extensions and no links.
/// Whether it compiles, and runs fast enough, is each app's to find out.
pub fn shader_problem(code: &str) -> Option<&'static str> {
    if code.trim().is_empty() {
        return Some("the shader is empty");
    }
    if code.len() > MAX_SHADER_BYTES {
        return Some("a shader is at most 16 KB");
    }
    let link = code.match_indices("://").any(|(at, _)| {
        code[..at]
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
            .any(|c| c.is_ascii_alphabetic())
    });
    if link {
        return Some("a shader has no links");
    }
    let live = strip_comments(code);
    if live.contains('@') {
        return Some("a shader has no attributes (@): fuwa gives it its inputs");
    }
    if ["enable", "requires", "diagnostic"].iter().any(|w| words(&live, w).next().is_some()) {
        return Some("a shader turns on no extensions");
    }
    let shade = words(&live, "fn").any(|at| {
        let rest = &live[at + 2..];
        let name = rest.trim_start();
        name.len() < rest.len() && name.strip_prefix("shade").is_some_and(|r| r.trim_start().starts_with('('))
    });
    if !shade {
        return Some("a shader defines fn shade(uv: vec2f) -> vec4f");
    }
    None
}

/// An effect's shader: its WGSL (line ends made plain, no control
/// characters but tabs), and how strong and fast it plays.
fn check_shader(shader: &Value) -> Result<Value> {
    let shader = shader.as_object().ok_or_else(|| Error::invalid("the effect's shader is an object with its code"))?;
    let code = shader.get("code").and_then(Value::as_str).unwrap_or_default();
    let code: String = code.replace("\r\n", "\n").replace('\r', "\n");
    if code.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err(Error::invalid("the effect's shader has control characters"));
    }
    if let Some(problem) = shader_problem(&code) {
        return Err(Error::invalid(format!("the effect's shader: {problem}")));
    }
    let mut out = Map::new();
    out.insert("code".into(), json!(code));
    for (field, min, max) in [("intensity", 0.05, 1.0), ("speed", 0.0, 4.0)] {
        match shader.get(field) {
            None | Some(Value::Null) => {}
            Some(value) => {
                let n = number(value, min, max).ok_or_else(|| {
                    Error::invalid(format!("the effect's shader: {field} is a number from {min} to {max}"))
                })?;
                out.insert(field.into(), json!(n));
            }
        }
    }
    Ok(Value::Object(out))
}

/// An effect's picture as its row keeps it (`pictures`, JSON).
#[derive(Serialize, Deserialize)]
struct StoredPicture {
    url: String,
    #[serde(default)]
    animated: bool,
    #[serde(default)]
    size: i64,
}

fn pictures_from(json: &str) -> Vec<pb::ProfileItemPicture> {
    serde_json::from_str::<Vec<StoredPicture>>(json)
        .unwrap_or_default()
        .into_iter()
        .map(|p| pb::ProfileItemPicture { url: p.url, animated: p.animated, size: p.size })
        .collect()
}

fn pictures_json(pictures: &[pb::ProfileItemPicture]) -> String {
    let stored: Vec<StoredPicture> =
        pictures.iter().map(|p| StoredPicture { url: p.url.clone(), animated: p.animated, size: p.size }).collect();
    serde_json::to_string(&stored).unwrap_or_else(|_| "[]".into())
}

/// The files an item keeps, as a server counts them in its attachments: a
/// decoration's picture, or an effect's.
pub fn files(item: &pb::ProfileItem) -> i64 {
    if item.picture_url.is_empty() { item.pictures.len() as i64 } else { 1 }
}

/// Every picture an item uses.
pub fn picture_urls(item: &pb::ProfileItem) -> impl Iterator<Item = &str> {
    std::iter::once(item.picture_url.as_str())
        .filter(|u| !u.is_empty())
        .chain(item.pictures.iter().map(|p| p.url.as_str()))
}

const COLUMNS: &str =
    "id, kind, name, description, effect, picture_url, animated, creator_id, size, created_at, pictures";

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
            pictures: pictures_from(&r.get::<String>(10)?),
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
        &format!("INSERT INTO profile_items ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"),
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
            pictures_json(&item.pictures),
        ),
    )
    .await?;
    Ok(())
}

pub async fn save(conn: &Connection, item: &pb::ProfileItem) -> Result<()> {
    conn.execute(
        "UPDATE profile_items SET name = ?2, description = ?3, effect = ?4, size = ?5, pictures = ?6 WHERE id = ?1",
        (
            item.id.as_str(),
            item.name.as_str(),
            item.description.as_str(),
            item.effect.as_str(),
            item.size,
            pictures_json(&item.pictures),
        ),
    )
    .await?;
    Ok(())
}

pub async fn delete(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM profile_items WHERE id = ?1", [id]).await?;
    Ok(())
}

/// What a new item is, checked: its kind, name, description and effect spec
/// (for an effect), under the id it will have. Pictures (a decoration's, an
/// effect's) are the caller's to check, since where they may come from
/// depends on who offers it.
pub fn checked_new(new: &pb::NewProfileItem, id: &str) -> Result<pb::ProfileItem> {
    let kind = match pb::ProfileItemKind::try_from(new.kind) {
        Ok(kind @ (pb::ProfileItemKind::Effect | pb::ProfileItemKind::Decoration | pb::ProfileItemKind::Nameplate)) => {
            kind
        }
        _ => return Err(Error::invalid("a profile item is an effect, a decoration or a nameplate")),
    };
    let name = checked_name(&new.name)?;
    let description = checked_description(&new.description)?;
    let effect = match kind {
        pb::ProfileItemKind::Effect => {
            if new.picture_urls.len() > MAX_PICTURES {
                return Err(Error::invalid("an effect has up to 8 pictures"));
            }
            check_effect(&new.effect, id, &name, &description, new.picture_urls.len())?
        }
        _ if !new.effect.is_empty() => return Err(Error::invalid("only effects have a spec")),
        _ => String::new(),
    };
    if kind == pb::ProfileItemKind::Effect && !new.picture_url.is_empty() {
        return Err(Error::invalid("only decorations and nameplates have a picture"));
    }
    if kind != pb::ProfileItemKind::Effect && !new.picture_urls.is_empty() {
        return Err(Error::invalid("a decoration or a nameplate is one picture"));
    }
    Ok(pb::ProfileItem { id: id.to_string(), kind: kind as i32, name, description, effect, ..Default::default() })
}

/// `item` with `change` applied and checked, its pictures already the new
/// ones (the spec is checked against them). Whether anything changed.
pub fn apply(item: &mut pb::ProfileItem, change: &pb::ProfileItemChange) -> Result<bool> {
    let before = item.clone();
    if let Some(name) = &change.name {
        item.name = checked_name(name)?;
    }
    if let Some(description) = &change.description {
        item.description = checked_description(description)?;
    }
    let effect = item.kind == pb::ProfileItemKind::Effect as i32;
    if change.pictures.is_some() && !effect {
        return Err(Error::invalid("only effects have pictures; a decoration's or a nameplate's can't change"));
    }
    match (&change.effect, effect) {
        (Some(_), false) => return Err(Error::invalid("only effects have a spec")),
        (Some(spec), true) => {
            item.effect = check_effect(spec, &item.id, &item.name, &item.description, item.pictures.len())?
        }
        // The spec carries the name and description too, so they stay in step.
        (None, true) => {
            item.effect = check_effect(&item.effect, &item.id, &item.name, &item.description, item.pictures.len())?
        }
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
        let spec = check_effect(SAKURA, "01ABCDEF", "Petals", "Pink", 0).unwrap();
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
            assert!(check_effect(spec, "01A", "n", "", 0).is_err(), "{spec}");
        }
        let huge = format!(r#"{{"layers":[],"pad":"{}"}}"#, "x".repeat(MAX_EFFECT_BYTES));
        assert!(check_effect(&huge, "01A", "n", "", 0).is_err());
    }

    #[test]
    fn layers_may_draw_the_items_pictures() {
        let spec = r##"{"layers":[{"image":1,"motion":"fall","phase":"idle","count":3,"from":"top","size":[20,30],
            "duration":[3000,4000],"glow":true}]}"##;
        let value: Value = serde_json::from_str(&check_effect(spec, "01A", "n", "", 2).unwrap()).unwrap();
        let layer = &value["layers"][0];
        assert_eq!(layer["image"], 1);
        assert!(layer.get("shape").is_none());
        assert!(layer.get("colors").is_none());
        // A place past the pictures it has, or pictures it doesn't have at all.
        assert!(check_effect(spec, "01A", "n", "", 1).is_err());
        assert!(check_effect(spec, "01A", "n", "", 0).is_err());
        let negative = spec.replace(r#""image":1"#, r#""image":-1"#);
        assert!(check_effect(&negative, "01A", "n", "", 2).is_err());
    }

    const SHADE: &str = "fn shade(uv: vec2f) -> vec4f {\r\n  return vec4f(fuwa.primary.rgb, uv.y * 0.3);\n}";

    #[test]
    fn a_shader_alone_is_an_effect() {
        let spec = json!({"shader": {"code": SHADE, "intensity": 0.6, "speed": 1.5, "extra": 1}}).to_string();
        let value: Value = serde_json::from_str(&check_effect(&spec, "01A", "n", "", 0).unwrap()).unwrap();
        assert_eq!(value["layers"], json!([]));
        assert_eq!(value["shader"]["intensity"], 0.6);
        assert!(!value["shader"]["code"].as_str().unwrap().contains('\r'));
        assert!(value["shader"].get("extra").is_none());
        // Without a shader, an effect still takes a layer.
        assert!(check_effect(r#"{"layers":[]}"#, "01A", "n", "", 0).is_err());
    }

    #[test]
    fn shaders_keep_to_the_apps_inputs() {
        let bad = [
            "",
            "fn other(uv: vec2f) -> vec4f { return vec4f(0.0); }",
            "@group(0) @binding(1) var t: texture_2d<f32>;\nfn shade(uv: vec2f) -> vec4f { return vec4f(0.0); }",
            "enable f16;\nfn shade(uv: vec2f) -> vec4f { return vec4f(0.0); }",
            "// see https://example.com\nfn shade(uv: vec2f) -> vec4f { return vec4f(0.0); }",
        ];
        for code in bad {
            let spec = json!({"shader": {"code": code}}).to_string();
            assert!(check_effect(&spec, "01A", "n", "", 0).is_err(), "{code}");
        }
        // Attributes and directives in comments are only words.
        assert!(
            shader_problem(
                "/* @group /* nested */ enable */ // @binding\nfn shade (uv: vec2f) -> vec4f { return vec4f(0.0); }"
            )
            .is_none()
        );
        let slow = json!({"shader": {"code": SHADE, "speed": 9}}).to_string();
        assert!(check_effect(&slow, "01A", "n", "", 0).is_err());
        let huge = json!({"shader": {"code": format!("{SHADE}//{}", "x".repeat(MAX_SHADER_BYTES))}}).to_string();
        assert!(check_effect(&huge, "01A", "n", "", 0).is_err());
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
        assert!(checked_new(&pb::NewProfileItem { effect: SAKURA.into(), ..decoration.clone() }, "01A").is_err());
        assert!(
            checked_new(&pb::NewProfileItem { picture_urls: vec!["x".into()], ..decoration.clone() }, "01A").is_err()
        );
        let nameplate = pb::NewProfileItem { kind: pb::ProfileItemKind::Nameplate as i32, ..decoration };
        assert!(checked_new(&nameplate, "01A").is_ok());
        assert!(checked_new(&pb::NewProfileItem { picture_urls: vec!["x".into()], ..nameplate }, "01A").is_err());
        let many = pb::NewProfileItem { picture_urls: vec!["x".into(); MAX_PICTURES + 1], ..effect.clone() };
        assert!(checked_new(&many, "01A").is_err());
        assert!(checked_new(&pb::NewProfileItem { name: " ".into(), ..effect }, "01A").is_err());
    }
}
