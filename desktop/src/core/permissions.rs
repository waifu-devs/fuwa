//! What someone may do in a server, worked out the way the server does it.
//! A port of `web/src/lib/permissions.ts`: the window hides what you can't
//! do, but the server decides.

use std::collections::HashMap;

use crate::core::store::InstanceState;
use crate::pb::{self, Permission as P};

pub type Bits = u32;

pub const fn bit(p: P) -> Bits {
    1 << (p as u32)
}

const KNOWN: [P; 19] = [
    P::Administrator,
    P::ManageServer,
    P::ManageRoles,
    P::ViewAuditLog,
    P::ChangeNickname,
    P::ManageNicknames,
    P::KickMembers,
    P::BanMembers,
    P::TimeOutMembers,
    P::ManageChannels,
    P::ViewChannels,
    P::SendMessages,
    P::EmbedLinks,
    P::AttachFiles,
    P::MentionEveryone,
    P::ManageMessages,
    P::CreateInvite,
    P::ManageEmoji,
    P::ManageWebhooks,
];

pub const ALL: Bits = {
    let mut bits = 0;
    let mut n = 0;
    while n < KNOWN.len() {
        bits |= bit(KNOWN[n]);
        n += 1;
    }
    bits
};

/// What a member who hasn't agreed to the rules yet can't do.
pub const TALK: Bits = bit(P::SendMessages)
    | bit(P::EmbedLinks)
    | bit(P::AttachFiles)
    | bit(P::MentionEveryone)
    | bit(P::CreateInvite)
    | bit(P::ChangeNickname);

fn from_list(list: &[i32]) -> Bits {
    list.iter().filter_map(|p| P::try_from(*p).ok()).filter(|p| KNOWN.contains(p)).fold(0, |bits, p| bits | bit(p))
}

#[derive(Debug, Clone, Default)]
pub struct Access {
    pub owner: bool,
    /// Server-wide. Every permission for the owner and for administrators.
    pub server: Bits,
    /// Their highest role's position; above every role for the owner.
    pub rank: i64,
    /// Per channel they can see, after its overwrites.
    pub channels: HashMap<String, Bits>,
    /// Hasn't agreed to the server's rules yet, so can read but not talk.
    pub pending: bool,
}

impl Access {
    pub fn has(&self, p: P) -> bool {
        self.server & bit(p) != 0
    }

    pub fn has_in(&self, channel_id: &str, p: P) -> bool {
        self.channels.get(channel_id).is_some_and(|bits| bits & bit(p) != 0)
    }
}

fn is_role(o: &pb::PermissionOverwrite) -> bool {
    o.target != pb::OverwriteTarget::Member as i32
}

/// A channel's category's overwrites, then its own. In each, @everyone's,
/// then their roles' together, then their own.
fn channel_bits(
    base: Bits,
    everyone_id: &str,
    user_id: &str,
    role_ids: &[String],
    channel: &pb::Channel,
    by_id: &HashMap<&str, &pb::Channel>,
) -> Bits {
    let mut bits = base;
    let parent = by_id.get(channel.parent_id.as_str()).filter(|p| p.r#type == pb::ChannelType::Category as i32);
    let layers: Vec<&pb::Channel> = match parent {
        Some(parent) => vec![parent, channel],
        None => vec![channel],
    };
    for layer in layers {
        let overwrites = &layer.permission_overwrites;
        if let Some(o) = overwrites.iter().find(|o| o.target_id == everyone_id && is_role(o)) {
            bits = (bits & !from_list(&o.deny)) | from_list(&o.allow);
        }
        let (mut allow, mut deny) = (0, 0);
        for o in overwrites.iter().filter(|o| is_role(o) && role_ids.contains(&o.target_id)) {
            allow |= from_list(&o.allow);
            deny |= from_list(&o.deny);
        }
        bits = (bits & !deny) | allow;
        if let Some(o) = overwrites.iter().find(|o| o.target_id == user_id && !is_role(o)) {
            bits = (bits & !from_list(&o.deny)) | from_list(&o.allow);
        }
    }
    if bits & bit(P::ViewChannels) != 0 { bits } else { 0 }
}

/// What someone with these roles can do. `everyone_id` is the server's id.
pub fn access_of(
    everyone_id: &str,
    owner_id: &str,
    roles: &[pb::Role],
    channels: &[pb::Channel],
    user_id: &str,
    role_ids: &[String],
    pending: bool,
) -> Access {
    let owner = user_id == owner_id;
    let held: Vec<&pb::Role> = roles.iter().filter(|r| role_ids.contains(&r.id)).collect();
    let everyone = roles.iter().find(|r| r.id == everyone_id).map(|r| from_list(&r.permissions)).unwrap_or(0);
    let base = held.iter().fold(everyone, |bits, r| bits | from_list(&r.permissions));
    let rank = if owner { i64::MAX } else { held.iter().map(|r| i64::from(r.position)).max().unwrap_or(0).max(0) };
    let unbound = owner || base & bit(P::Administrator) != 0;
    let by_id: HashMap<&str, &pb::Channel> = channels.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut visible: HashMap<String, Bits> = HashMap::new();
    for c in channels {
        let bits = if unbound { ALL } else { channel_bits(base, everyone_id, user_id, role_ids, c, &by_id) };
        if bits & bit(P::ViewChannels) != 0 {
            visible.insert(c.id.clone(), bits);
        }
    }
    // A category shows while any channel in it does.
    for c in channels {
        if let Some(parent) = by_id.get(c.parent_id.as_str())
            && parent.r#type == pb::ChannelType::Category as i32
            && visible.contains_key(&c.id)
            && !visible.contains_key(&parent.id)
        {
            visible.insert(parent.id.clone(), bit(P::ViewChannels));
        }
    }
    let held_back = pending && !owner;
    if held_back {
        for bits in visible.values_mut() {
            *bits &= !TALK;
        }
    }
    Access {
        owner,
        server: (if unbound { ALL } else { base }) & if held_back { !TALK } else { ALL },
        rank,
        channels: visible,
        pending: held_back,
    }
}

impl InstanceState {
    /// Your own member in a server.
    pub fn my_member(&self, server_id: &str) -> Option<&pb::Member> {
        let me = self.me.as_ref()?;
        self.members.get(server_id)?.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == me.id))
    }

    /// What you may do in a server.
    pub fn access(&self, server_id: &str) -> Access {
        let (Some(server), Some(me)) = (self.server(server_id), self.me.as_ref()) else { return Access::default() };
        let member = self.my_member(server_id);
        access_of(
            server_id,
            &server.owner_id,
            self.roles.get(server_id).map(Vec::as_slice).unwrap_or_default(),
            self.channels.get(server_id).map(Vec::as_slice).unwrap_or_default(),
            &me.id,
            member.map(|m| m.role_ids.as_slice()).unwrap_or_default(),
            member.is_some_and(|m| m.pending),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(id: &str, position: i32, permissions: &[P]) -> pb::Role {
        pb::Role {
            id: id.into(),
            position,
            permissions: permissions.iter().map(|p| *p as i32).collect(),
            ..Default::default()
        }
    }

    fn channel(id: &str, parent: &str, overwrites: Vec<pb::PermissionOverwrite>) -> pb::Channel {
        pb::Channel {
            id: id.into(),
            parent_id: parent.into(),
            r#type: if id.starts_with("cat") { pb::ChannelType::Category } else { pb::ChannelType::Text } as i32,
            permission_overwrites: overwrites,
            ..Default::default()
        }
    }

    fn deny(target: &str, kind: pb::OverwriteTarget, permissions: &[P]) -> pb::PermissionOverwrite {
        pb::PermissionOverwrite {
            target_id: target.into(),
            target: kind as i32,
            deny: permissions.iter().map(|p| *p as i32).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn overwrites_layer_like_the_server() {
        let roles = vec![role("mods", 2, &[P::ManageMessages]), role("S", 0, &[P::ViewChannels, P::SendMessages])];
        let channels = vec![
            channel("general", "", vec![]),
            channel("cat-staff", "", vec![deny("S", pb::OverwriteTarget::Role, &[P::ViewChannels])]),
            channel("staff", "cat-staff", vec![]),
            channel("quiet", "", vec![deny("me", pb::OverwriteTarget::Member, &[P::SendMessages])]),
        ];
        let a = access_of("S", "owner", &roles, &channels, "me", &["mods".into()], false);
        assert!(a.has_in("general", P::SendMessages));
        assert!(a.has_in("general", P::ManageMessages));
        assert!(!a.channels.contains_key("staff"), "the category hides it");
        assert!(a.has_in("quiet", P::ViewChannels) && !a.has_in("quiet", P::SendMessages));
        assert_eq!(a.rank, 2);

        let pending = access_of("S", "owner", &roles, &channels, "me", &[], true);
        assert!(pending.pending && !pending.has_in("general", P::SendMessages));

        let owner = access_of("S", "me", &roles, &channels, "me", &[], true);
        assert!(owner.owner && owner.has_in("staff", P::SendMessages) && !owner.pending);
    }
}
