//! Profile items (docs/profile-items.md): effects and avatar decorations an
//! instance or one of its servers offers. The store keeps each list; this is
//! what a person wears, and the calls that list, add, rename and delete them.
//! Like the web's `fuwa/profile-items.ts`.

use std::sync::Arc;

use tonic::Code;

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::profile_effects::{self as fx, Picture, Spec};
use crate::core::store::{self, InstanceState};
use crate::pb;
use crate::rpc;

fn missing() -> Problem {
    Problem::new(Code::NotFound, "That instance isn't here.")
}

/// Whose list an item is on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    Instance,
    Server(String),
}

/// What a new item is, before it's sent.
#[derive(Debug, Clone)]
pub enum NewItem {
    /// A picture from this computer, uploaded first.
    Decoration { content_type: String, bytes: Vec<u8> },
    /// An effect's spec, as JSON, sent as is: the instance checks it. With
    /// its pictures' links, in order, uploaded already (`send_pictures`).
    Effect { spec: String, pictures: Vec<String> },
}

/// One of an effect's pictures while it's edited: on the instance already,
/// or a file on this computer to upload.
#[derive(Debug, Clone, PartialEq)]
pub enum DraftPicture {
    Uploaded { url: String, animated: bool },
    File(std::path::PathBuf),
}

impl DraftPicture {
    /// As the preview plays it: a file shows from this computer.
    pub fn picture(&self) -> Picture {
        match self {
            DraftPicture::Uploaded { url, animated } => Picture { url: url.clone(), animated: *animated, local: false },
            DraftPicture::File(path) => {
                let name = path.to_string_lossy().into_owned();
                let animated = name.to_ascii_lowercase().ends_with(".gif");
                Picture { url: name, animated, local: true }
            }
        }
    }
}

/// An item's pictures as specs take them.
pub fn item_pictures(item: &pb::ProfileItem) -> Vec<Picture> {
    item.pictures.iter().map(|p| Picture { url: p.url.clone(), animated: p.animated, local: false }).collect()
}

/// An effect item's spec, ready to play: parsed and checked like any spec
/// that didn't ship with the app, under the item's id in lowercase and its
/// own name and description (the web's `itemEffect`).
pub fn item_effect(item: &pb::ProfileItem) -> Option<Spec> {
    if item.kind != pb::ProfileItemKind::Effect as i32 || item.effect.is_empty() {
        return None;
    }
    parse_effect(
        &item.effect,
        Some(&item.id.to_lowercase()),
        Some((&item.name, &item.description)),
        &item_pictures(item),
    )
}

/// A spec from JSON text, with the pictures its image layers draw; `id` and
/// `text` replace its own when given.
pub fn parse_effect(json: &str, id: Option<&str>, text: Option<(&str, &str)>, pictures: &[Picture]) -> Option<Spec> {
    let mut raw: serde_json::Value = serde_json::from_str(json).ok()?;
    let named = raw.as_object_mut()?;
    if let Some(id) = id {
        named.insert("id".into(), id.into());
    }
    if let Some((name, description)) = text {
        named.insert("name".into(), name.into());
        named.insert("description".into(), description.into());
    }
    fx::sanitize(&raw, pictures)
}

/// Why an effect someone pasted or picked can't be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectProblem {
    Empty,
    Json,
    Spec,
}

impl EffectProblem {
    pub fn key(self) -> &'static str {
        match self {
            EffectProblem::Empty => "serversettings.profileItems.effectEmpty",
            EffectProblem::Json => "serversettings.profileItems.effectNotJson",
            EffectProblem::Spec => "serversettings.profileItems.effectNothing",
        }
    }
}

/// Checks an effect someone is about to add (the instance checks again):
/// the spec under a stand-in id for the preview, or what's wrong. `pictures`
/// are the ones it will have (files on this computer until they're uploaded).
pub fn check_effect_text(text: &str, pictures: &[Picture]) -> Result<Spec, EffectProblem> {
    if text.trim().is_empty() {
        return Err(EffectProblem::Empty);
    }
    if serde_json::from_str::<serde_json::Value>(text).is_err() {
        return Err(EffectProblem::Json);
    }
    parse_effect(text, Some("preview"), Some(("preview", "")), pictures).ok_or(EffectProblem::Spec)
}

/// A spec's text split for editing: the JSON without its shader's code, and
/// the code on its own, so the WGSL is edited as WGSL rather than inside a
/// JSON string. Text that isn't JSON (or has no shader) comes back as it is.
/// The web's `splitShader`.
pub fn split_shader(text: &str) -> (String, String) {
    let Ok(serde_json::Value::Object(mut spec)) = serde_json::from_str::<serde_json::Value>(text) else {
        return (text.to_owned(), String::new());
    };
    let Some(serde_json::Value::Object(shader)) = spec.get_mut("shader") else {
        return (text.to_owned(), String::new());
    };
    let Some(serde_json::Value::String(code)) = shader.remove("code") else {
        return (text.to_owned(), String::new());
    };
    if shader.is_empty() {
        spec.remove("shader");
    }
    let json = serde_json::to_string_pretty(&spec).unwrap_or_else(|_| text.to_owned());
    (json, code)
}

/// The spec's text again from its two parts: the shader's code put back in
/// (its other settings, if any, kept), or the shader left out when there's
/// no code. JSON that doesn't parse comes back as it is, for the checks to
/// say so. A shader alone needs no JSON at all. The web's `joinShader`.
pub fn join_shader(json: &str, wgsl: &str) -> String {
    use serde_json::{Value, json};
    let code = wgsl.trim_end();
    if json.trim().is_empty() {
        return if code.trim().is_empty() {
            String::new()
        } else {
            json!({ "layers": [], "shader": { "code": code } }).to_string()
        };
    }
    let Ok(Value::Object(mut spec)) = serde_json::from_str::<Value>(json) else { return json.to_owned() };
    if code.trim().is_empty() {
        spec.remove("shader");
    } else {
        let mut settings = match spec.remove("shader") {
            Some(Value::Object(settings)) => settings,
            _ => serde_json::Map::new(),
        };
        settings.insert("code".into(), code.into());
        spec.insert("shader".into(), Value::Object(settings));
    }
    if !spec.get("layers").is_some_and(Value::is_array) {
        spec.insert("layers".into(), Value::Array(Vec::new()));
    }
    Value::Object(spec).to_string()
}

/// An item by id, of a kind, whatever the id's case (an effect's spec keeps it in lowercase).
fn find<'a>(items: &'a [pb::ProfileItem], id: &str, kind: pb::ProfileItemKind) -> Option<&'a pb::ProfileItem> {
    if id.is_empty() {
        return None;
    }
    items.iter().find(|i| i.kind == kind as i32 && (i.id == id || i.id.eq_ignore_ascii_case(id)))
}

/// An effect by id: a built-in, else one of the items given.
pub fn resolve_effect(id: &str, items: &[pb::ProfileItem]) -> Option<Spec> {
    if id.is_empty() {
        return None;
    }
    fx::spec(id).or_else(|| find(items, id, pb::ProfileItemKind::Effect).and_then(item_effect))
}

/// A decoration by id among the items given.
pub fn resolve_decoration<'a>(id: &str, items: &'a [pb::ProfileItem]) -> Option<&'a pb::ProfileItem> {
    find(items, id, pb::ProfileItemKind::Decoration)
}

/// The effects a list offers, as pickers take them: each one that can play, oldest first.
pub fn offered_effects(items: &[pb::ProfileItem]) -> Vec<(String, Spec)> {
    items.iter().filter_map(|i| item_effect(i).map(|s| (i.id.clone(), s))).collect()
}

/// A name for an item from a file's name: no extension, separators as
/// spaces, at most 40 characters (the web's `nameFromFile`).
pub fn name_from_file(file: &str) -> String {
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    let name = stem.replace(['_', '-'], " ").split_whitespace().collect::<Vec<_>>().join(" ");
    name.chars().take(40).collect::<String>().trim().to_owned()
}

// ───────────────────────── Helping people make them ─────────────────────────

/// The docs that explain each kind: the effect format, and what a decoration should look like.
pub const EFFECT_DOCS: &str = "https://github.com/waifu-devs/fuwa/blob/master/docs/profile-effects.md#the-format";
pub const DECORATION_DOCS: &str = "https://github.com/waifu-devs/fuwa/blob/master/docs/profile-items.md#decorations";

/// What someone pastes into an AI chat to have it write a profile effect,
/// ending where they describe what they want. The web's `EFFECT_PROMPT`, word
/// for word.
pub const EFFECT_PROMPT: &str = r##"You're writing a profile effect for fuwa, a chat app: an animated layer of small particles that plays over someone's profile card (304 by 420 pixels) when it opens. People still need to read the name, status and bio under it, so particles start at the card's edges and corners and cross it quickly.

An effect is a JSON spec, data rather than code. It plays a short intro (under three seconds) when the card opens, then settles into a sparse idle loop for as long as the card stays open.

{
  "layers": [ ... 1 to 6 layers ... ]
}

Each layer is one kind of particle. Its fields:
- shape: one of petal, star, sparkle, heart, snowflake, bubble, dot, confetti, streak.
- motion: fall or rise (across the card, swaying), twinkle (a glint in place, then a rest), drift (wanders and glows), burst (flies out and falls away), shoot (a streak, then a rest), pop (swells and pops).
- phase: intro plays once when the card opens; idle loops after it. Have at least one of each.
- count: 1 to 24 particles, for the 304 by 420 card. At most 60 play at once across all layers, so keep idle layers sparse (5 to 12).
- from: where they start: top, bottom, edges, corners, anywhere, top-left, top-right, center.
- size: [min, max] in pixels, 2 to 96. Most particles look right between 10 and 32.
- duration: [min, max] for one pass, 300 to 20000 ms. Intro layers 900 to 2500; idle layers 2000 to 12000.
- delay: [min, max] in ms, 0 to 3000: when each intro particle starts after the card opens. Leave it out for idle layers.
- colors: 1 to 8 of #rrggbb or the theme's tokens primary, ring, accent, foreground, card, or profile (the card's own color). Mixing a token in makes the effect fit every theme.
- spin: degrees turned over a pass, -1440 to 1440 (optional).
- sway: 0 to 400 pixels: sideways sway for fall and rise (10 to 30), wander radius for drift, flight distance for burst (200 to 300) (optional).
- flip: true to tumble in 3D, like paper (optional).
- glow: true for a soft glow in the particle's color (optional).
- opacity: peak opacity, 0.05 to 1 (optional, 1 when left out).

Two more things an effect can have, if what I want calls for them:

- Pictures: an effect can carry up to 8 pictures of its own, uploaded with it in an order. A layer with "image": 0 draws the first picture instead of a shape ("image": 1 the second, and so on). It leaves out shape, and may leave out colors: a picture keeps its own, and its glow is then the theme's primary. Only use pictures I say I have, and tell me which picture goes in which place.
- A shader: "shader": { "code": "...", "intensity": 0.6, "speed": 1 } draws a WGSL shader across the whole card, under the particles. The code defines fn shade(uv: vec2f) -> vec4f, called for each pixel with uv from 0,0 at the top left to 1,1 at the bottom right, and returns a color and how much it covers the card (straight alpha, 0 to 1). Keep alpha low in the middle (0.15 or less) so the text stays readable. It can read the uniform fuwa: fuwa.primary (the theme's color), fuwa.accent (the card's own color), fuwa.background (the card), fuwa.foreground (its text), all vec4f; fuwa.res (its size in pixels) and fuwa.pointer (0 to 1), both vec2f; fuwa.time (seconds) and fuwa.intensity, both f32; and the helpers hash21(p: vec2f) -> f32, noise(p: vec2f) -> f32 and fbm(p: vec2f) -> f32. It can't declare anything with an attribute (no @group, @binding or entry points), turn on extensions or contain links, and it's at most 16 KB. intensity (0.05 to 1) scales how much it covers, speed (0 to 4) how fast its time runs. A shader alone, with "layers": [], is an effect too. In the JSON the code is one string, so write its line breaks as \n.

fuwa refuses a spec with a shape, motion, place or color not on these lists, a number out of its range, a shader that breaks those rules, or more than 40 KB, and drops fields it doesn't know. The effect gets its id, name and description from the item it's added as, so leave those out.

An example, fuwa's own Sakura (a gust of petals from the top left, then a gentle fall):

{
  "layers": [
    { "shape": "petal", "motion": "burst", "phase": "intro", "count": 18, "from": "top-left", "size": [18, 28], "duration": [1500, 2300], "delay": [0, 260], "colors": ["#ffb7c5", "#ff8fab", "#ffc8d6", "primary"], "spin": 540, "sway": 280, "flip": true, "opacity": 0.95 },
    { "shape": "petal", "motion": "fall", "phase": "idle", "count": 9, "from": "top", "size": [15, 24], "duration": [6500, 9500], "colors": ["#ffb7c5", "#ff8fab", "#ffc8d6", "primary"], "spin": 320, "sway": 22, "flip": true, "opacity": 0.9 }
  ]
}

Answer with the whole spec in one json code block, then say in a sentence or two which numbers to change to tune it. If I come back with an error from fuwa, fix it and answer with the whole spec again.

What I want:
"##;

/// What someone pastes into an AI that makes pictures to have it draw an
/// avatar decoration. The web's `DECORATION_PROMPT`, word for word.
pub const DECORATION_PROMPT: &str = r##"Draw an avatar decoration for fuwa, a chat app: a frame that's drawn over someone's round profile picture, like a wreath or a crown around it.

- A square PNG, 600 by 600 pixels, with a truly transparent background (real alpha, not a checkerboard pattern painted in, and no background color).
- The profile picture is a circle 500 pixels across in the middle, 50 pixels in from each edge. Keep that circle see-through so the face shows: put the art around its rim and in the corners, overlapping the circle's edge only a little.
- Don't draw a face, a person or a profile picture inside it: only the frame.
- It's shown as small as 40 pixels across next to messages, so use bold shapes and clear outlines, and no text or fine detail.

What I want:
"##;

/// The template's size and the avatar's circle in it: the avatar is 1/1.2 of the picture.
pub const TEMPLATE_SIZE: u32 = 600;
pub const TEMPLATE_AVATAR: u32 = 500;

/// A built-in effect as a spec to start from: only its layers (the item gets
/// its own id, name and line), laid out like the web's `starterText`, with
/// each list of numbers or colors on one line.
pub fn starter_text(id: &str) -> Option<String> {
    let spec = fx::spec(id)?;
    let num = |n: f32| format!("{n}");
    let pair = |(a, b): (f32, f32)| format!("[{}, {}]", num(a), num(b));
    let layers: Vec<String> = spec
        .layers
        .iter()
        .map(|l| {
            let colors: Vec<String> = l.colors.iter().map(|c| format!("\"{}\"", c.name())).collect();
            let mut fields = vec![
                format!("\"shape\": \"{}\"", l.shape.map_or("", fx::Shape::name)),
                format!("\"motion\": \"{}\"", l.motion.name()),
                format!("\"phase\": \"{}\"", if l.intro { "intro" } else { "idle" }),
                format!("\"count\": {}", l.count),
                format!("\"from\": \"{}\"", l.from.name()),
                format!("\"size\": {}", pair(l.size)),
                format!("\"duration\": {}", pair(l.duration)),
            ];
            if l.intro {
                fields.push(format!("\"delay\": {}", pair(l.delay)));
            }
            fields.push(format!("\"colors\": [{}]", colors.join(", ")));
            if l.spin != 0.0 {
                fields.push(format!("\"spin\": {}", num(l.spin)));
            }
            if l.sway != 0.0 {
                fields.push(format!("\"sway\": {}", num(l.sway)));
            }
            if l.flip {
                fields.push("\"flip\": true".into());
            }
            if l.glow {
                fields.push("\"glow\": true".into());
            }
            if l.opacity != 1.0 {
                fields.push(format!("\"opacity\": {}", num(l.opacity)));
            }
            format!("    {{\n      {}\n    }}", fields.join(",\n      "))
        })
        .collect();
    Some(format!("{{\n  \"layers\": [\n{}\n  ]\n}}", layers.join(",\n")))
}

/// A picture to draw a decoration on, as RGBA pixels `TEMPLATE_SIZE` square:
/// see-through, with the avatar's circle shaded grey and dashed round its rim.
pub fn template_pixels() -> Vec<u8> {
    let size = TEMPLATE_SIZE as usize;
    let (mid, r) = (TEMPLATE_SIZE as f32 / 2.0, TEMPLATE_AVATAR as f32 / 2.0);
    let mut px = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 + 0.5 - mid, y as f32 + 0.5 - mid);
            let d = (dx * dx + dy * dy).sqrt();
            let fill = 0.35 * (r - d + 0.5).clamp(0.0, 1.0);
            // A 3 pixel line, 12 on and 8 off along the rim.
            let along = (dy.atan2(dx) + std::f32::consts::PI) * r;
            let dashed = along % 20.0 < 12.0;
            let line = if dashed { 0.9 * (2.0 - (d - r).abs()).clamp(0.0, 1.0) } else { 0.0 };
            let a = line + fill * (1.0 - line);
            let i = (y * size + x) * 4;
            px[i..i + 4].copy_from_slice(&[128, 128, 128, (a * 255.0).round() as u8]);
        }
    }
    px
}

impl InstanceState {
    /// Whether the instance lets profile effects play.
    pub fn effects_on(&self) -> bool {
        self.node.as_ref().is_some_and(|n| n.profile_effects)
    }

    /// A server's offered items (none until its snapshot is in).
    pub fn server_list(&self, server_id: &str) -> &[pb::ProfileItem] {
        self.server_items.get(server_id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The effect someone shows: their server profile's pick where they're
    /// seen in a server (a built-in or one of that server's), else their own
    /// (a built-in or one of the instance's). The web's `wornEffect`.
    pub fn worn_effect(&self, own: &str, server_id: Option<&str>, member: Option<&pb::Member>) -> Option<Spec> {
        match server_id.zip(member).filter(|(_, m)| !m.effect.is_empty()) {
            Some((sid, m)) => resolve_effect(&m.effect, self.server_list(sid)),
            None => resolve_effect(own, &self.profile_items),
        }
    }

    /// Whether the instance draws decorations at all.
    pub fn decorations_on(&self) -> bool {
        self.node.as_ref().is_some_and(|n| n.profile_decorations)
    }

    /// One of the instance's items, by id.
    pub fn instance_item(&self, id: &str) -> Option<&pb::ProfileItem> {
        if id.is_empty() {
            return None;
        }
        self.profile_items.iter().find(|item| item.id == id)
    }

    /// One of a server's items, by id.
    pub fn server_item(&self, server_id: &str, id: &str) -> Option<&pb::ProfileItem> {
        if id.is_empty() {
            return None;
        }
        self.server_items.get(server_id)?.iter().find(|item| item.id == id)
    }

    /// The picture of the decoration someone wears: in a server their pick
    /// there (from the server's list) wins over their own (the instance's).
    /// None when the instance has decorations off or the item is unknown.
    pub fn decoration_url(
        &self,
        server_id: Option<&str>,
        member: Option<&pb::Member>,
        user: Option<&pb::User>,
    ) -> Option<&str> {
        if !self.decorations_on() {
            return None;
        }
        let user = user.or_else(|| member.and_then(|m| m.user.as_ref()))?;
        let picked = server_id.zip(member).filter(|(_, m)| !m.decoration_id.is_empty());
        let item = match picked {
            Some((sid, m)) => resolve_decoration(&m.decoration_id, self.server_list(sid)),
            None => resolve_decoration(&user.decoration_id, &self.profile_items),
        };
        item.filter(|i| !i.picture_url.is_empty()).map(|i| i.picture_url.as_str())
    }

    /// [`InstanceState::decoration_url`] for someone by id, finding their member row.
    pub fn decoration_of(&self, server_id: Option<&str>, user_id: &str) -> Option<&str> {
        if !self.decorations_on() {
            return None;
        }
        let member = server_id.and_then(|sid| self.member(sid, user_id));
        self.decoration_url(server_id, member, self.users.get(user_id))
    }

    /// Someone's member row in a server.
    pub fn member(&self, server_id: &str, user_id: &str) -> Option<&pb::Member> {
        self.members.get(server_id)?.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == user_id))
    }

    /// The items of one kind on a list, oldest first.
    pub fn items_of(&self, scope: &Scope, kind: pb::ProfileItemKind) -> Vec<pb::ProfileItem> {
        let list = match scope {
            Scope::Instance => Some(&self.profile_items),
            Scope::Server(id) => self.server_items.get(id),
        };
        list.into_iter().flatten().filter(|i| i.kind == kind as i32).cloned().collect()
    }
}

/// The instance's items changed: when the node's `profile_items_at` moved.
pub fn items_moved(before: Option<&pb::Node>, after: Option<&pb::Node>) -> bool {
    let at = |n: Option<&pb::Node>| n.and_then(|n| n.profile_items_at.as_ref()).map(|t| (t.seconds, t.nanos));
    at(before) != at(after)
}

/// Puts an item on its list in the store, replacing one with its id.
fn put(i: &mut InstanceState, scope: &Scope, item: pb::ProfileItem) {
    let list = match scope {
        Scope::Instance => &mut i.profile_items,
        Scope::Server(id) => i.server_items.entry(id.clone()).or_default(),
    };
    match list.iter_mut().find(|x| x.id == item.id) {
        Some(x) => *x = item,
        None => list.push(item),
    }
}

fn take(i: &mut InstanceState, scope: &Scope, id: &str) {
    match scope {
        Scope::Instance => i.profile_items.retain(|x| x.id != id),
        Scope::Server(sid) => {
            if let Some(list) = i.server_items.get_mut(sid) {
                list.retain(|x| x.id != id);
            }
        }
    }
}

impl Core {
    /// Lists the instance's items again. An instance from before them has none.
    pub async fn refresh_instance_items(&self, key: &str) {
        let Some(api) = self.api(key) else { return };
        match rpc!(api.profile_items(), list_instance_profile_items(pb::ListInstanceProfileItemsRequest {})).await {
            Ok(res) => {
                self.shared.instance(key, |i| i.profile_items = res.items);
            }
            Err(e) if e.code == Code::Unimplemented => {
                self.shared.instance(key, |i| i.profile_items.clear());
            }
            Err(_) => {}
        }
    }

    /// Adds an item to the instance's list or a server's.
    pub async fn add_profile_item(
        self: &Arc<Self>,
        key: &str,
        scope: &Scope,
        name: &str,
        description: &str,
        new: NewItem,
    ) -> Result<pb::ProfileItem, Problem> {
        let mut item = pb::NewProfileItem { name: name.into(), description: description.into(), ..Default::default() };
        match new {
            NewItem::Decoration { content_type, bytes } => {
                let server_id = match scope {
                    Scope::Instance => "",
                    Scope::Server(id) => id.as_str(),
                };
                item.kind = pb::ProfileItemKind::Decoration as i32;
                item.picture_url =
                    self.upload_picture_for(key, server_id, pb::MediaPurpose::Decoration, &content_type, bytes).await?;
            }
            NewItem::Effect { spec, pictures } => {
                item.kind = pb::ProfileItemKind::Effect as i32;
                item.effect = spec;
                item.picture_urls = pictures;
            }
        }
        let api = self.api(key).ok_or_else(missing)?;
        let made = match scope {
            Scope::Instance => {
                let req = pb::CreateInstanceProfileItemRequest { item: Some(item) };
                rpc!(api.profile_items(), create_instance_profile_item(req)).await?.item
            }
            Scope::Server(id) => {
                let req = pb::CreateServerProfileItemRequest { server_id: id.clone(), item: Some(item) };
                rpc!(api.profile_items(), create_server_profile_item(req)).await?.item
            }
        }
        .unwrap_or_default();
        self.shared.instance(key, |i| put(i, scope, made.clone()));
        Ok(made)
    }

    /// Uploads an effect's new pictures (for the server, or the instance), in
    /// order, and hands back every picture's link (the web's `sendPictures`).
    pub async fn send_pictures(
        self: &Arc<Self>,
        key: &str,
        scope: &Scope,
        pictures: Vec<DraftPicture>,
    ) -> Result<Vec<String>, Problem> {
        let server_id = match scope {
            Scope::Instance => "",
            Scope::Server(id) => id.as_str(),
        };
        let mut urls = Vec::with_capacity(pictures.len());
        for picture in pictures {
            match picture {
                DraftPicture::Uploaded { url, .. } => urls.push(url),
                DraftPicture::File(path) => {
                    let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let content_type = crate::core::account::picture_type(&file).unwrap_or("image/png");
                    let bytes = crate::core::account::read_picture(&path).await?;
                    urls.push(
                        self.upload_picture_for(key, server_id, pb::MediaPurpose::Decoration, content_type, bytes)
                            .await?,
                    );
                }
            }
        }
        Ok(urls)
    }

    /// Renames an item, changes its description, or replaces an effect's spec and pictures.
    pub async fn change_profile_item(
        &self,
        key: &str,
        scope: &Scope,
        id: &str,
        change: pb::ProfileItemChange,
    ) -> Result<pb::ProfileItem, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let changed = match scope {
            Scope::Instance => {
                let req = pb::UpdateInstanceProfileItemRequest { item_id: id.into(), change: Some(change) };
                rpc!(api.profile_items(), update_instance_profile_item(req)).await?.item
            }
            Scope::Server(sid) => {
                let req = pb::UpdateServerProfileItemRequest {
                    server_id: sid.clone(),
                    item_id: id.into(),
                    change: Some(change),
                };
                rpc!(api.profile_items(), update_server_profile_item(req)).await?.item
            }
        }
        .unwrap_or_default();
        self.shared.instance(key, |i| put(i, scope, changed.clone()));
        Ok(changed)
    }

    /// Deletes an item, which takes it off everyone wearing it.
    pub async fn delete_profile_item(&self, key: &str, scope: &Scope, id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        match scope {
            Scope::Instance => {
                let req = pb::DeleteInstanceProfileItemRequest { item_id: id.into() };
                rpc!(api.profile_items(), delete_instance_profile_item(req)).await?;
            }
            Scope::Server(sid) => {
                let req = pb::DeleteServerProfileItemRequest { server_id: sid.clone(), item_id: id.into() };
                rpc!(api.profile_items(), delete_server_profile_item(req)).await?;
            }
        }
        self.shared.instance(key, |i| {
            take(i, scope, id);
            // The instance takes it off everyone; ours shows that at once.
            if scope == &Scope::Instance
                && let Some(me) = i.me.clone().filter(|me| me.decoration_id == id)
            {
                store::update_user(i, &pb::User { decoration_id: String::new(), ..me });
            }
        });
        Ok(())
    }

    /// Wears one of the instance's decorations everywhere, or none with an empty id.
    pub async fn set_decoration(&self, key: &str, decoration_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let req = pb::UpdateProfileRequest { decoration_id: Some(decoration_id.into()), ..Default::default() };
        let res = rpc!(api.auth(), update_profile(req)).await?;
        if let Some(user) = &res.user {
            self.shared.instance(key, |i| store::update_user(i, user));
        }
        Ok(())
    }

    /// Your profile in one server: a nickname, and an effect and a decoration
    /// shown there instead of your own (`None` keeps it, empty goes back to your own).
    pub async fn set_server_look(
        &self,
        key: &str,
        server_id: &str,
        nickname: Option<String>,
        effect: Option<String>,
        decoration_id: Option<String>,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let user_id = self.shared.read(|s| s.instance(key).and_then(|i| i.me.as_ref().map(|m| m.id.clone())));
        let req = pb::UpdateMemberRequest {
            server_id: server_id.into(),
            user_id: user_id.unwrap_or_default(),
            nickname,
            effect,
            decoration_id,
        };
        let res = rpc!(api.servers(), update_member(req)).await?;
        if let Some(member) = res.member {
            self.shared.instance(key, |i| {
                if let Some(list) = i.members.get_mut(server_id)
                    && let Some(m) =
                        list.iter_mut().find(|m| m.user.as_ref().map(|u| &u.id) == member.user.as_ref().map(|u| &u.id))
                {
                    *m = member;
                }
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web(file: &str) -> String {
        let path = format!("{}/../web/src/lib/{file}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(path).expect("the web app's profile items")
    }

    #[test]
    fn the_prompts_are_the_webs_word_for_word() {
        let web = web("profile-items.ts");
        // As the template literal reads at run time: `\\n` in its source is `\n` in the text.
        let read = web.replace("\\\\", "\\");
        assert!(read.contains(&format!("export const EFFECT_PROMPT = `{EFFECT_PROMPT}`;")));
        assert!(web.contains(&format!("export const DECORATION_PROMPT = `{DECORATION_PROMPT}`;")));
        assert!(web.contains(&format!("export const EFFECT_DOCS = \"{EFFECT_DOCS}\";")));
        assert!(web.contains(&format!("export const DECORATION_DOCS = \"{DECORATION_DOCS}\";")));
        assert!(web.contains(&format!("export const TEMPLATE_SIZE = {TEMPLATE_SIZE};")));
        assert!(web.contains(&format!("export const TEMPLATE_AVATAR = {TEMPLATE_AVATAR};")));
    }

    #[test]
    fn every_starter_is_its_built_ins_layers() {
        for id in fx::IDS {
            let text = starter_text(id).unwrap();
            let spec = check_effect_text(&text, &[]).unwrap();
            assert_eq!(spec.layers, fx::spec(id).unwrap().layers, "{id}");
            assert!(text.contains("\"size\": ["), "{text}");
        }
        assert!(starter_text("nope").is_none());
        // Laid out as the web's `specText` lays out the same spec.
        let sakura = starter_text("sakura").unwrap();
        assert!(sakura.starts_with("{\n  \"layers\": [\n    {\n      \"shape\": \"petal\",\n"), "{sakura}");
        assert!(
            sakura.contains("\"colors\": [\"#ffb7c5\", \"#ff8fab\", \"#ffc8d6\", \"primary\"],\n      \"spin\": 540"),
            "{sakura}"
        );
        assert!(sakura.contains("\"opacity\": 0.95\n    },"), "{sakura}");
    }

    #[test]
    fn the_shader_is_edited_apart_from_the_json() {
        let text =
            r#"{"layers":[],"shader":{"code":"fn shade(uv: vec2f) -> vec4f {\n  return vec4f(0.0);\n}","speed":2}}"#;
        let (json, wgsl) = split_shader(text);
        assert_eq!(wgsl, "fn shade(uv: vec2f) -> vec4f {\n  return vec4f(0.0);\n}");
        assert!(!json.contains("code") && json.contains("\"speed\": 2"), "{json}");
        let joined: serde_json::Value = serde_json::from_str(&join_shader(&json, &format!("{wgsl}\n\n"))).unwrap();
        assert_eq!(joined, serde_json::from_str::<serde_json::Value>(text).unwrap());
        // No code: no shader; no JSON: a shader alone.
        let bare = join_shader(r#"{"layers":[],"shader":{"speed":2}}"#, " ");
        assert_eq!(bare, r#"{"layers":[]}"#);
        assert_eq!(join_shader("", "x"), r#"{"layers":[],"shader":{"code":"x"}}"#);
        assert_eq!(join_shader("", "  "), "");
        assert_eq!(join_shader("{", "x"), "{");
        assert_eq!(split_shader("{"), ("{".to_owned(), String::new()));
        assert_eq!(split_shader(r#"{"layers":[]}"#).1, "");
        // Without settings beside the code, the shader goes from the JSON altogether.
        assert!(!split_shader(r#"{"layers":[],"shader":{"code":"x"}}"#).0.contains("shader"));
    }

    #[test]
    fn an_items_pictures_reach_its_spec() {
        let spec = r#"{"layers":[{"image":0,"motion":"pop","phase":"intro","count":3,"from":"edges","size":[4,8],"duration":[400,900]}]}"#;
        let mut item = pb::ProfileItem {
            id: "01PIC".into(),
            kind: pb::ProfileItemKind::Effect as i32,
            name: "Pics".into(),
            effect: spec.into(),
            ..Default::default()
        };
        assert!(item_effect(&item).is_none());
        item.pictures = vec![pb::ProfileItemPicture { url: "https://k/media/p".into(), animated: true, size: 10 }];
        let found = item_effect(&item).unwrap();
        assert_eq!(found.pictures, vec![Picture { url: "https://k/media/p".into(), animated: true, local: false }]);
        assert_eq!(check_effect_text(spec, &[]).unwrap_err(), EffectProblem::Spec);
        let file = DraftPicture::File("/tmp/a.GIF".into()).picture();
        assert!(file.local && file.animated);
        assert!(check_effect_text(spec, &[file]).is_ok());
    }

    #[test]
    fn the_template_marks_the_avatar_and_leaves_the_rest_clear() {
        let px = template_pixels();
        let at = |x: usize, y: usize| px[(y * TEMPLATE_SIZE as usize + x) * 4 + 3];
        assert_eq!(px.len(), (TEMPLATE_SIZE * TEMPLATE_SIZE * 4) as usize);
        assert_eq!(at(300, 300), 89);
        assert_eq!(at(10, 10), 0);
        assert_eq!(at(599, 300), 0);
    }

    fn decoration(id: &str, server: &str) -> pb::ProfileItem {
        pb::ProfileItem {
            id: id.into(),
            server_id: server.into(),
            kind: pb::ProfileItemKind::Decoration as i32,
            picture_url: format!("https://k/media/{id}"),
            ..Default::default()
        }
    }

    #[test]
    fn offered_effects_resolve_by_id_whatever_the_case() {
        let spec = r#"{"id":"x","layers":[{"shape":"dot","motion":"pop","phase":"intro","count":3,"from":"edges","size":[4,8],"duration":[400,900],"colors":["primary"]}]}"#;
        let item = pb::ProfileItem {
            id: "01ABC".into(),
            kind: pb::ProfileItemKind::Effect as i32,
            name: "Dots".into(),
            effect: spec.into(),
            ..Default::default()
        };
        let items = vec![item];
        let found = resolve_effect("01abc", &items).unwrap();
        assert_eq!((found.id.as_str(), found.name.as_str()), ("01abc", "Dots"));
        assert_eq!(resolve_effect("sakura", &items).unwrap().id, "sakura");
        assert!(resolve_effect("gone", &items).is_none());
        assert_eq!(offered_effects(&items).len(), 1);
        assert_eq!(check_effect_text(" ", &[]).unwrap_err(), EffectProblem::Empty);
        assert_eq!(check_effect_text("{", &[]).unwrap_err(), EffectProblem::Json);
        assert_eq!(check_effect_text("{}", &[]).unwrap_err(), EffectProblem::Spec);
        assert!(check_effect_text(spec, &[]).is_ok());
        assert_eq!(name_from_file("Cherry_Blossom.json"), "Cherry Blossom");
        assert_eq!(name_from_file("ring-gold.png"), "ring gold");
    }

    #[test]
    fn a_server_pick_wins_and_the_switch_hides_all() {
        let mut i = InstanceState::new("k", "https://k");
        i.node = Some(pb::Node { profile_decorations: true, ..Default::default() });
        i.profile_items = vec![decoration("mine", "")];
        i.server_items.insert("s".into(), vec![decoration("theirs", "s")]);
        let user = pb::User { id: "u".into(), decoration_id: "mine".into(), ..Default::default() };
        let mut member = pb::Member { user: Some(user.clone()), ..Default::default() };
        i.users.insert("u".into(), user.clone());
        assert_eq!(i.decoration_url(None, None, Some(&user)), Some("https://k/media/mine"));
        assert_eq!(i.decoration_url(Some("s"), Some(&member), None), Some("https://k/media/mine"));
        member.decoration_id = "theirs".into();
        assert_eq!(i.decoration_url(Some("s"), Some(&member), None), Some("https://k/media/theirs"));
        // Gone from the list: nothing.
        member.decoration_id = "gone".into();
        assert_eq!(i.decoration_url(Some("s"), Some(&member), None), None);
        i.members.insert("s".into(), vec![pb::Member { decoration_id: "theirs".into(), ..member }]);
        assert_eq!(i.decoration_of(Some("s"), "u"), Some("https://k/media/theirs"));
        i.node = Some(pb::Node { profile_decorations: false, ..Default::default() });
        assert_eq!(i.decoration_of(Some("s"), "u"), None);
    }
}
