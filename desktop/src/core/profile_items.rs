//! Profile items (docs/profile-items.md): effects and avatar decorations an
//! instance or one of its servers offers. The store keeps each list; this is
//! what a person wears, and the calls that list, add, rename and delete them.
//! Like the web's `fuwa/profile-items.ts`.

use std::sync::Arc;

use tonic::Code;

use crate::core::Core;
use crate::core::api::Problem;
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

impl InstanceState {
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
            Some((sid, m)) => self.server_item(sid, &m.decoration_id),
            None => self.instance_item(&user.decoration_id),
        };
        item.filter(|i| i.kind == pb::ProfileItemKind::Decoration as i32 && !i.picture_url.is_empty())
            .map(|i| i.picture_url.as_str())
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

    /// Your profile in one server: an effect and a decoration shown there
    /// instead of your own (`None` keeps it, empty goes back to your own).
    pub async fn set_server_look(
        &self,
        key: &str,
        server_id: &str,
        effect: Option<String>,
        decoration_id: Option<String>,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let user_id = self.shared.read(|s| s.instance(key).and_then(|i| i.me.as_ref().map(|m| m.id.clone())));
        let req = pb::UpdateMemberRequest {
            server_id: server_id.into(),
            user_id: user_id.unwrap_or_default(),
            effect,
            decoration_id,
            ..Default::default()
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
