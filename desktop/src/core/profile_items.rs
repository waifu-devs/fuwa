//! Profile items (docs/profile-items.md): effects and avatar decorations an
//! instance or one of its servers offers. The store keeps each list; this is
//! what a person wears, and the calls that list, add, rename and delete them.
//! Like the web's `fuwa/profile-items.ts`.

use std::sync::Arc;

use tonic::Code;

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::profile_effects::{self as fx, Spec};
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
    /// An effect's spec, as JSON, sent as is: the instance checks it.
    Effect { spec: String },
}

/// An effect item's spec, ready to play: parsed and checked like any spec
/// that didn't ship with the app, under the item's id in lowercase and its
/// own name and description (the web's `itemEffect`).
pub fn item_effect(item: &pb::ProfileItem) -> Option<Spec> {
    if item.kind != pb::ProfileItemKind::Effect as i32 || item.effect.is_empty() {
        return None;
    }
    parse_effect(&item.effect, Some(&item.id.to_lowercase()), Some((&item.name, &item.description)))
}

/// A spec from JSON text; `id` and `text` replace its own when given.
pub fn parse_effect(json: &str, id: Option<&str>, text: Option<(&str, &str)>) -> Option<Spec> {
    let mut raw: serde_json::Value = serde_json::from_str(json).ok()?;
    let named = raw.as_object_mut()?;
    if let Some(id) = id {
        named.insert("id".into(), id.into());
    }
    if let Some((name, description)) = text {
        named.insert("name".into(), name.into());
        named.insert("description".into(), description.into());
    }
    fx::sanitize(&raw)
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
/// the spec under a stand-in id for the preview, or what's wrong.
pub fn check_effect_text(text: &str) -> Result<Spec, EffectProblem> {
    if text.trim().is_empty() {
        return Err(EffectProblem::Empty);
    }
    if serde_json::from_str::<serde_json::Value>(text).is_err() {
        return Err(EffectProblem::Json);
    }
    parse_effect(text, Some("preview"), Some(("preview", ""))).ok_or(EffectProblem::Spec)
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
            NewItem::Effect { spec } => {
                item.kind = pb::ProfileItemKind::Effect as i32;
                item.effect = spec;
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

    /// Renames an item, or changes its description.
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
        assert_eq!(check_effect_text(" ").unwrap_err(), EffectProblem::Empty);
        assert_eq!(check_effect_text("{").unwrap_err(), EffectProblem::Json);
        assert_eq!(check_effect_text("{}").unwrap_err(), EffectProblem::Spec);
        assert!(check_effect_text(spec).is_ok());
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
