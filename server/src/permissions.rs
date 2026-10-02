//! Roles and permissions: what each member of a community server can do,
//! server-wide and in each of its channels.
//!
//! A server's [`Rules`] (its owner, roles, channels and their overwrites) are
//! read from its file; everything else here is worked out from them, the same
//! way the web app does in `web/src/lib/permissions.ts`.

use std::collections::{HashMap, HashSet};

use turso::Connection;

use crate::db::query_all;
use crate::error::{Error, Result};
use crate::id::timestamp;
use crate::pb::{self, Permission as P};

/// A set of permissions: bit `1 << n` for each `fuwa.v1.Permission` n.
pub type Bits = u64;

pub const fn bit(p: P) -> Bits {
    1 << p as u64
}

const KNOWN: [P; 17] = [
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
];

/// Every permission there is.
pub const ALL: Bits = {
    let mut all = 0;
    let mut i = 0;
    while i < KNOWN.len() {
        all |= bit(KNOWN[i]);
        i += 1;
    }
    all
};

/// The permissions a channel's overwrites can change.
pub const CHANNEL: Bits = bit(P::ManageChannels)
    | bit(P::ManageRoles)
    | bit(P::ViewChannels)
    | bit(P::SendMessages)
    | bit(P::EmbedLinks)
    | bit(P::AttachFiles)
    | bit(P::MentionEveryone)
    | bit(P::ManageMessages)
    | bit(P::CreateInvite);

/// What @everyone can do in a new server.
pub const EVERYONE: Bits = bit(P::ViewChannels)
    | bit(P::SendMessages)
    | bit(P::EmbedLinks)
    | bit(P::AttachFiles)
    | bit(P::ChangeNickname)
    | bit(P::CreateInvite);

/// The Admin role a new server starts with, and that the admins of servers
/// from before roles were given: what admins could do then.
pub const ADMIN: Bits = bit(P::ManageServer)
    | bit(P::ManageChannels)
    | bit(P::ManageMessages)
    | bit(P::MentionEveryone)
    | bit(P::ViewAuditLog)
    | bit(P::KickMembers)
    | bit(P::BanMembers)
    | bit(P::TimeOutMembers)
    | bit(P::ManageNicknames);

/// Reads permissions off the wire, refusing ones this version doesn't know.
pub fn from_list(list: &[i32]) -> Result<Bits> {
    list.iter().try_fold(0, |bits, &value| match P::try_from(value) {
        Ok(p) if p != P::Unspecified => Ok(bits | bit(p)),
        _ => Err(Error::invalid(format!("{value} is not a permission"))),
    })
}

/// Permissions as the wire lists them, in the protocol's order.
pub fn to_list(bits: Bits) -> Vec<i32> {
    KNOWN.iter().filter(|&&p| bits & bit(p) != 0).map(|&p| p as i32).collect()
}

/// A permission's name, as people see it.
pub fn label(p: P) -> &'static str {
    match p {
        P::Unspecified => "no",
        P::Administrator => "Administrator",
        P::ManageServer => "Manage server",
        P::ManageRoles => "Manage roles",
        P::ViewAuditLog => "View audit log",
        P::ChangeNickname => "Change nickname",
        P::ManageNicknames => "Manage nicknames",
        P::KickMembers => "Kick members",
        P::BanMembers => "Ban members",
        P::TimeOutMembers => "Time out members",
        P::ManageChannels => "Manage channels",
        P::ViewChannels => "View channels",
        P::SendMessages => "Send messages",
        P::EmbedLinks => "Embed links",
        P::AttachFiles => "Attach files",
        P::MentionEveryone => "Mention everyone",
        P::ManageMessages => "Manage messages",
        P::CreateInvite => "Create invite",
    }
}

/// Refuses for want of `p`.
pub fn missing(p: P) -> Error {
    Error::denied(format!("you need the {} permission for that", label(p)))
}

pub struct Role {
    pub position: i64,
    pub permissions: Bits,
}

pub struct Overwrite {
    pub target_id: String,
    pub member: bool,
    pub allow: Bits,
    pub deny: Bits,
}

pub struct Channel {
    pub parent_id: Option<String>,
    pub category: bool,
    pub overwrites: Vec<Overwrite>,
}

/// Everything that decides who can do what in one server.
pub struct Rules {
    /// The server's id, which is also @everyone's.
    pub everyone_id: String,
    pub owner_id: String,
    pub roles: HashMap<String, Role>,
    pub channels: HashMap<String, Channel>,
}

/// What one member can do.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Access {
    pub owner: bool,
    /// Server-wide: @everyone's permissions and every role's they have. Every
    /// permission for the owner and for administrators.
    pub server: Bits,
    /// Their highest role's position; above every role for the owner.
    pub rank: i64,
    /// Per channel, after its overwrites. Channels they can't see aren't here.
    channels: HashMap<String, Bits>,
}

impl Rules {
    /// What someone with these roles can do.
    pub fn access(&self, user_id: &str, role_ids: &[String]) -> Access {
        let owner = user_id == self.owner_id;
        let held: Vec<&Role> = role_ids.iter().filter_map(|id| self.roles.get(id)).collect();
        let everyone = self.roles.get(&self.everyone_id).map_or(0, |r| r.permissions);
        let base = held.iter().fold(everyone, |bits, role| bits | role.permissions);
        let rank = if owner { i64::MAX } else { held.iter().map(|r| r.position).max().unwrap_or(0) };
        let unbound = owner || base & bit(P::Administrator) != 0;
        let server = if unbound { ALL } else { base };

        let mut channels = HashMap::with_capacity(self.channels.len());
        for (id, channel) in &self.channels {
            let bits = if unbound { ALL } else { self.channel_bits(base, user_id, role_ids, channel) };
            if bits & bit(P::ViewChannels) != 0 {
                channels.insert(id.clone(), bits);
            }
        }
        // A category shows while any channel in it does, even if it's hidden itself.
        let holders: Vec<String> = channels
            .keys()
            .filter_map(|id| self.channels.get(id)?.parent_id.clone())
            .filter(|parent| !channels.contains_key(parent) && self.channels.get(parent).is_some_and(|c| c.category))
            .collect();
        for parent in holders {
            channels.insert(parent, bit(P::ViewChannels));
        }
        Access { owner, server, rank, channels }
    }

    /// Permissions in a channel: its category's overwrites, then its own. In
    /// each, @everyone's, then every role's they have together, then their own.
    fn channel_bits(&self, base: Bits, user_id: &str, role_ids: &[String], channel: &Channel) -> Bits {
        let mut bits = base;
        let parent = channel.parent_id.as_ref().and_then(|id| self.channels.get(id)).filter(|p| p.category);
        for layer in parent.into_iter().chain([channel]) {
            let find = |id: &str| layer.overwrites.iter().find(|o| o.target_id == id);
            if let Some(o) = find(&self.everyone_id).filter(|o| !o.member) {
                bits = (bits & !o.deny) | o.allow;
            }
            let (mut allow, mut deny) = (0, 0);
            for o in layer.overwrites.iter().filter(|o| !o.member && role_ids.contains(&o.target_id)) {
                allow |= o.allow;
                deny |= o.deny;
            }
            bits = (bits & !deny) | allow;
            if let Some(o) = find(user_id).filter(|o| o.member) {
                bits = (bits & !o.deny) | o.allow;
            }
        }
        if bits & bit(P::ViewChannels) == 0 { 0 } else { bits }
    }
}

impl Access {
    pub fn has(&self, p: P) -> bool {
        self.server & bit(p) != 0
    }

    /// Refuses unless they have `p` server-wide.
    pub fn require(&self, p: P) -> Result<()> {
        if self.has(p) { Ok(()) } else { Err(missing(p)) }
    }

    pub fn can_see(&self, channel_id: &str) -> bool {
        self.channels.contains_key(channel_id)
    }

    /// Their permissions in a channel, none if they can't see it.
    pub fn in_channel(&self, channel_id: &str) -> Bits {
        self.channels.get(channel_id).copied().unwrap_or(0)
    }

    pub fn has_in(&self, channel_id: &str, p: P) -> bool {
        self.in_channel(channel_id) & bit(p) != 0
    }

    /// Refuses unless they have `p` in the channel; a channel they can't see
    /// reads as not there at all.
    pub fn require_in(&self, channel_id: &str, p: P) -> Result<()> {
        if !self.can_see(channel_id) {
            return Err(Error::NotFound("channel"));
        }
        if self.has_in(channel_id, p) { Ok(()) } else { Err(missing(p)) }
    }

    /// The channels they can see.
    pub fn visible(&self) -> HashSet<String> {
        self.channels.keys().cloned().collect()
    }

    /// Whether they rank above someone else: the owner above everyone, others
    /// by their highest roles.
    pub fn outranks(&self, other: &Access) -> bool {
        self.owner || (!other.owner && self.rank > other.rank)
    }

    /// Whether they rank above a role at `position`.
    pub fn above(&self, position: i64) -> bool {
        self.owner || self.rank > position
    }

    /// Whether they may change these permissions, given what they have: only
    /// ones they have themselves, unless they're the owner or an administrator.
    pub fn may_change(&self, changed: Bits, have: Bits) -> bool {
        self.owner || self.has(P::Administrator) || changed & !have == 0
    }
}

/// A server's rules, as its file has them now.
pub async fn load(conn: &Connection, everyone_id: &str) -> Result<Rules> {
    let owner_id = crate::db::query_one(conn, "SELECT owner_id FROM server", (), |r| r.get::<String>(0))
        .await?
        .ok_or_else(|| Error::internal("server row missing"))?;
    let roles = query_all(conn, "SELECT id, position, permissions FROM roles", (), |r| {
        Ok((r.get::<String>(0)?, Role { position: r.get(1)?, permissions: r.get::<i64>(2)? as Bits }))
    })
    .await?
    .into_iter()
    .collect();
    let mut channels: HashMap<String, Channel> = query_all(conn, "SELECT id, parent_id, type FROM channels", (), |r| {
        Ok((
            r.get::<String>(0)?,
            Channel {
                parent_id: r.get::<Option<String>>(1)?,
                category: r.get::<i64>(2)? == pb::ChannelType::Category as i64,
                overwrites: vec![],
            },
        ))
    })
    .await?
    .into_iter()
    .collect();
    for (channel_id, overwrite) in overwrite_rows(conn, None).await? {
        if let Some(channel) = channels.get_mut(&channel_id) {
            channel.overwrites.push(overwrite);
        }
    }
    Ok(Rules { everyone_id: everyone_id.to_string(), owner_id, roles, channels })
}

/// Overwrites as stored, for one channel or all of them.
async fn overwrite_rows(conn: &Connection, channel_id: Option<&str>) -> Result<Vec<(String, Overwrite)>> {
    query_all(
        conn,
        "SELECT channel_id, target_id, target, allow, deny FROM channel_overwrites
         WHERE ?1 IS NULL OR channel_id = ?1 ORDER BY channel_id, target, target_id",
        [channel_id],
        |r| {
            Ok((
                r.get::<String>(0)?,
                Overwrite {
                    target_id: r.get(1)?,
                    member: r.get::<i64>(2)? == pb::OverwriteTarget::Member as i64,
                    allow: r.get::<i64>(3)? as Bits,
                    deny: r.get::<i64>(4)? as Bits,
                },
            ))
        },
    )
    .await
}

fn overwrite_pb(o: &Overwrite) -> pb::PermissionOverwrite {
    pb::PermissionOverwrite {
        target_id: o.target_id.clone(),
        target: if o.member { pb::OverwriteTarget::Member } else { pb::OverwriteTarget::Role } as i32,
        allow: to_list(o.allow),
        deny: to_list(o.deny),
    }
}

/// Fills in the overwrites of channels read without them.
pub async fn attach_overwrites(conn: &Connection, channels: &mut [pb::Channel]) -> Result<()> {
    let only = (channels.len() == 1).then(|| channels[0].id.clone());
    let mut by_channel: HashMap<String, Vec<pb::PermissionOverwrite>> = HashMap::new();
    for (channel_id, overwrite) in overwrite_rows(conn, only.as_deref()).await? {
        by_channel.entry(channel_id).or_default().push(overwrite_pb(&overwrite));
    }
    for channel in channels {
        channel.permission_overwrites = by_channel.remove(&channel.id).unwrap_or_default();
    }
    Ok(())
}

/// The roles someone has, besides @everyone.
pub async fn member_role_ids(conn: &Connection, user_id: &str) -> Result<Vec<String>> {
    query_all(conn, "SELECT role_id FROM member_roles WHERE user_id = ?1 ORDER BY role_id", [user_id], |r| r.get(0))
        .await
}

/// Fills in the roles of members read without them.
pub async fn attach_roles(conn: &Connection, members: &mut [pb::Member]) -> Result<()> {
    if let [member] = members {
        let id = member.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
        member.role_ids = member_role_ids(conn, &id).await?;
        return Ok(());
    }
    let mut by_user: HashMap<String, Vec<String>> = HashMap::new();
    for (user_id, role_id) in query_all(conn, "SELECT user_id, role_id FROM member_roles ORDER BY role_id", (), |r| {
        Ok((r.get::<String>(0)?, r.get::<String>(1)?))
    })
    .await?
    {
        by_user.entry(user_id).or_default().push(role_id);
    }
    for member in members {
        let id = member.user.as_ref().map(|u| u.id.as_str()).unwrap_or_default();
        member.role_ids = by_user.remove(id).unwrap_or_default();
    }
    Ok(())
}

pub const ROLE_COLUMNS: &str = "id, name, color, position, permissions, hoist, mentionable, created_at, updated_at";

pub fn role_row(server_id: &str) -> impl Fn(&turso::Row) -> turso::Result<pb::Role> + '_ {
    move |r| {
        Ok(pb::Role {
            id: r.get(0)?,
            server_id: server_id.to_string(),
            name: r.get(1)?,
            color: r.get::<Option<i64>>(2)?.map(|c| c as i32),
            position: r.get::<i64>(3)? as i32,
            permissions: to_list(r.get::<i64>(4)? as Bits),
            hoist: r.get(5)?,
            mentionable: r.get(6)?,
            created_at: Some(timestamp(r.get(7)?)),
            updated_at: Some(timestamp(r.get(8)?)),
        })
    }
}

/// Every role, highest first, @everyone last.
pub async fn roles(conn: &Connection, server_id: &str) -> Result<Vec<pb::Role>> {
    query_all(conn, &format!("SELECT {ROLE_COLUMNS} FROM roles ORDER BY position DESC, id"), (), role_row(server_id))
        .await
}

pub async fn role(conn: &Connection, server_id: &str, role_id: &str) -> Result<Option<pb::Role>> {
    crate::db::query_one(
        conn,
        &format!("SELECT {ROLE_COLUMNS} FROM roles WHERE id = ?1"),
        [role_id],
        role_row(server_id),
    )
    .await
}

/// Gives a server the roles it starts with, if it has none yet: @everyone,
/// and an Admin role held by whoever was an admin before roles. Inside a
/// write; returns the roles it made.
pub async fn seed(conn: &Connection, server_id: &str, now: i64) -> Result<Vec<pb::Role>> {
    let any = crate::db::query_one(conn, "SELECT 1 FROM roles LIMIT 1", (), |r| r.get::<i64>(0)).await?;
    if any.is_some() {
        return Ok(vec![]);
    }
    let admin = crate::id::new_id();
    conn.execute(
        "INSERT INTO roles (id, name, color, position, permissions, hoist, mentionable, created_at, updated_at)
         VALUES (?1, '@everyone', NULL, 0, ?2, 0, 0, ?4, ?4), (?3, 'Admin', ?5, 1, ?6, 1, 0, ?4, ?4)",
        (server_id, EVERYONE as i64, admin.as_str(), now, ADMIN_COLOR, ADMIN as i64),
    )
    .await?;
    // Ranks from before roles: 2 was admin.
    conn.execute(
        "INSERT INTO member_roles (user_id, role_id) SELECT user_id, ?1 FROM members WHERE role = 2",
        [admin.as_str()],
    )
    .await?;
    roles(conn, server_id).await
}

/// The Admin role's color: fuwa's pink.
const ADMIN_COLOR: i64 = 0xF472B6;

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        let role = |position, permissions| Role { position, permissions };
        let overwrite =
            |target_id: &str, member, allow, deny| Overwrite { target_id: target_id.into(), member, allow, deny };
        let channel = |parent: Option<&str>, category, overwrites| Channel {
            parent_id: parent.map(Into::into),
            category,
            overwrites,
        };
        Rules {
            everyone_id: "s".into(),
            owner_id: "owner".into(),
            roles: HashMap::from([
                ("s".into(), role(0, EVERYONE)),
                ("mod".into(), role(2, bit(P::KickMembers) | bit(P::ManageMessages))),
                ("vip".into(), role(1, 0)),
                ("boss".into(), role(3, bit(P::Administrator))),
            ]),
            channels: HashMap::from([
                ("general".into(), channel(None, false, vec![])),
                // A category only VIPs see, with one channel mods can see too.
                (
                    "lounge".into(),
                    channel(
                        None,
                        true,
                        vec![
                            overwrite("s", false, 0, bit(P::ViewChannels)),
                            overwrite("vip", false, bit(P::ViewChannels), 0),
                        ],
                    ),
                ),
                ("vip-chat".into(), channel(Some("lounge"), false, vec![])),
                (
                    "mod-chat".into(),
                    channel(Some("lounge"), false, vec![overwrite("mod", false, bit(P::ViewChannels), 0)]),
                ),
                // Read-only for everyone but one member.
                (
                    "news".into(),
                    channel(
                        None,
                        false,
                        vec![
                            overwrite("s", false, 0, bit(P::SendMessages)),
                            overwrite("writer", true, bit(P::SendMessages), 0),
                        ],
                    ),
                ),
            ]),
        }
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn everyone_sees_what_isnt_hidden() {
        let access = rules().access("someone", &[]);
        assert_eq!(access.rank, 0);
        assert!(access.has(P::SendMessages) && !access.has(P::KickMembers));
        assert_eq!(access.visible(), HashSet::from(["general".into(), "news".into()]));
        assert!(access.has_in("general", P::SendMessages));
        assert!(!access.has_in("news", P::SendMessages));
        assert!(matches!(access.require_in("vip-chat", P::ViewChannels), Err(Error::NotFound(_))));
    }

    #[test]
    fn overwrites_apply_category_first_then_roles_then_members() {
        let rules = rules();
        let vip = rules.access("v", &ids(&["vip"]));
        assert!(vip.can_see("lounge") && vip.can_see("vip-chat") && vip.can_see("mod-chat"));

        // Mods only see their channel, and the category it sits in to hold it.
        let moderator = rules.access("m", &ids(&["mod"]));
        assert!(moderator.can_see("mod-chat") && !moderator.can_see("vip-chat"));
        assert_eq!(moderator.in_channel("lounge"), bit(P::ViewChannels));
        assert!(moderator.has_in("mod-chat", P::ManageMessages));

        let writer = rules.access("writer", &[]);
        assert!(writer.has_in("news", P::SendMessages));
    }

    #[test]
    fn owners_and_administrators_do_everything() {
        let rules = rules();
        for access in [rules.access("owner", &[]), rules.access("b", &ids(&["boss"]))] {
            assert_eq!(access.server, ALL);
            assert_eq!(access.visible().len(), 5);
            assert!(access.has_in("news", P::SendMessages));
        }
    }

    #[test]
    fn rank_follows_the_highest_role() {
        let rules = rules();
        let owner = rules.access("owner", &[]);
        let boss = rules.access("b", &ids(&["boss"]));
        let moderator = rules.access("m", &ids(&["vip", "mod"]));
        let member = rules.access("x", &[]);
        assert_eq!(moderator.rank, 2);
        assert!(owner.outranks(&boss) && !boss.outranks(&owner));
        assert!(boss.outranks(&moderator) && moderator.outranks(&member));
        assert!(!moderator.outranks(&rules.access("y", &ids(&["mod"]))));
        assert!(moderator.above(1) && !moderator.above(2));
        // Only what they have, unless they can do everything.
        assert!(moderator.may_change(bit(P::KickMembers), moderator.server));
        assert!(!moderator.may_change(bit(P::BanMembers), moderator.server));
        assert!(boss.may_change(bit(P::BanMembers), 0));
    }

    #[test]
    fn permissions_cross_the_wire() {
        let bits = bit(P::ViewChannels) | bit(P::ManageRoles);
        assert_eq!(from_list(&to_list(bits)).unwrap(), bits);
        assert!(from_list(&[0]).is_err());
        assert!(from_list(&[999]).is_err());
        assert_eq!(to_list(ALL).len(), KNOWN.len());
        assert_eq!(EVERYONE & !ALL, 0);
        assert_eq!(CHANNEL & !ALL, 0);
    }
}
