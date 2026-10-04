//! What the directory knows about every community server: its profile, who's
//! in it, its invites' codes, and (in a split instance) which shard holds it. Kept in memory and
//! built from the servers' own files: read at start by a single process, or
//! reported by each shard when it registers.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::RwLock;

use crate::cpb;
use crate::pb;

#[derive(Default)]
pub struct Index {
    inner: RwLock<Inner>,
}

#[derive(Default)]
struct Inner {
    servers: HashMap<String, pb::Server>,
    /// Account id to the ids of the servers it's a member of.
    memberships: HashMap<String, BTreeSet<String>>,
    /// Server id to its members' account ids.
    members: HashMap<String, HashSet<String>>,
    /// Server id to the shard holding it. Also has servers whose shard hasn't
    /// registered since the directory started, which aren't in `servers` yet.
    placements: HashMap<String, String>,
    /// Invite code to the server it's for.
    invites: HashMap<String, String>,
}

impl Inner {
    fn insert(&mut self, server: pb::Server, member_ids: Vec<String>, invite_codes: Vec<String>) {
        let id = server.id.clone();
        self.forget_members(&id);
        for member in &member_ids {
            self.memberships.entry(member.clone()).or_default().insert(id.clone());
        }
        self.members.insert(id.clone(), member_ids.into_iter().collect());
        self.invites.retain(|_, server_id| *server_id != id);
        for code in invite_codes {
            self.invites.insert(code, id.clone());
        }
        self.servers.insert(id, server);
    }

    fn forget_members(&mut self, server_id: &str) {
        for member in self.members.remove(server_id).unwrap_or_default() {
            if let Some(servers) = self.memberships.get_mut(&member) {
                servers.remove(server_id);
                if servers.is_empty() {
                    self.memberships.remove(&member);
                }
            }
        }
    }

    fn remove(&mut self, server_id: &str) {
        self.forget_members(server_id);
        self.servers.remove(server_id);
        self.placements.remove(server_id);
        self.invites.retain(|_, id| id != server_id);
    }
}

impl Index {
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Inner> {
        self.inner.read().unwrap_or_else(|p| p.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(|p| p.into_inner())
    }

    /// Adds a server (or replaces what was known about it), on `shard` if the
    /// instance is split.
    pub fn insert(&self, server: pb::Server, member_ids: Vec<String>, invite_codes: Vec<String>, shard: Option<&str>) {
        let mut inner = self.write();
        if let Some(shard) = shard {
            inner.placements.insert(server.id.clone(), shard.to_string());
        }
        inner.insert(server, member_ids, invite_codes);
    }

    /// Takes what a shard says it holds as the truth about that shard: its
    /// servers are these, and servers placed on it before that aren't among
    /// them are gone. Returns the ids of those gone.
    pub fn register(&self, shard: &str, entries: Vec<cpb::ServerEntry>) -> Vec<String> {
        let mut inner = self.write();
        let held: HashSet<&str> =
            entries.iter().filter_map(|entry| entry.server.as_ref().map(|s| s.id.as_str())).collect();
        let gone: Vec<String> = inner
            .placements
            .iter()
            .filter(|(id, on)| on.as_str() == shard && !held.contains(id.as_str()))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &gone {
            inner.remove(id);
        }
        for entry in entries {
            let Some(server) = entry.server else { continue };
            if let Some(other) = inner.placements.get(&server.id).filter(|on| on.as_str() != shard) {
                tracing::warn!(server = %server.id, from = %other, to = %shard, "a server moved shards");
            }
            inner.placements.insert(server.id.clone(), shard.to_string());
            inner.insert(server, entry.member_ids, entry.invite_codes);
        }
        gone
    }

    /// Where servers are, as the directory last stored it, before their shards
    /// register again.
    pub fn load_placements(&self, placements: Vec<(String, String)>) {
        let mut inner = self.write();
        for (server, shard) in placements {
            inner.placements.entry(server).or_insert(shard);
        }
    }

    pub fn placement(&self, server_id: &str) -> Option<String> {
        self.read().placements.get(server_id).cloned()
    }

    /// Servers held by each shard.
    pub fn shard_sizes(&self) -> HashMap<String, usize> {
        let mut sizes = HashMap::new();
        for shard in self.read().placements.values() {
            *sizes.entry(shard.clone()).or_default() += 1;
        }
        sizes
    }

    /// The ids of the servers each shard holds, among `ids` (every server when empty).
    pub fn by_shard(&self, ids: &[String]) -> HashMap<String, Vec<String>> {
        let inner = self.read();
        let mut grouped: HashMap<String, Vec<String>> = HashMap::new();
        let mut add = |id: &String, shard: &String| grouped.entry(shard.clone()).or_default().push(id.clone());
        if ids.is_empty() {
            for (id, shard) in &inner.placements {
                add(id, shard);
            }
        } else {
            for id in ids {
                if let Some(shard) = inner.placements.get(id) {
                    add(id, shard);
                }
            }
        }
        grouped
    }

    pub fn remove(&self, server_id: &str) {
        self.write().remove(server_id);
    }

    pub fn summary(&self, id: &str) -> Option<pb::Server> {
        self.read().servers.get(id).cloned()
    }

    /// Records a server's new profile after a committed change.
    pub fn update(&self, server: pb::Server) {
        let mut inner = self.write();
        if let Some(known) = inner.servers.get_mut(&server.id) {
            *known = server;
        }
    }

    /// Someone joined. Hearing of it twice counts it once.
    pub fn join(&self, account_id: &str, server_id: &str) {
        let mut inner = self.write();
        if !inner.servers.contains_key(server_id) {
            return;
        }
        let added = inner.members.entry(server_id.to_string()).or_default().insert(account_id.to_string());
        inner.memberships.entry(account_id.to_string()).or_default().insert(server_id.to_string());
        if added && let Some(server) = inner.servers.get_mut(server_id) {
            server.member_count += 1;
        }
    }

    /// Someone left. Hearing of it twice counts it once.
    pub fn leave(&self, account_id: &str, server_id: &str) {
        let mut inner = self.write();
        let removed = inner.members.get_mut(server_id).is_some_and(|members| members.remove(account_id));
        if let Some(servers) = inner.memberships.get_mut(account_id) {
            servers.remove(server_id);
            if servers.is_empty() {
                inner.memberships.remove(account_id);
            }
        }
        if removed && let Some(server) = inner.servers.get_mut(server_id) {
            server.member_count = (server.member_count - 1).max(0);
        }
    }

    /// The server an invite code is for.
    pub fn invite(&self, code: &str) -> Option<String> {
        self.read().invites.get(code).cloned()
    }

    /// An invite was made (`exists`), or deleted, used up or expired.
    pub fn index_invite(&self, server_id: &str, code: &str, exists: bool) {
        let mut inner = self.write();
        if !exists {
            inner.invites.remove(code);
        } else if inner.servers.contains_key(server_id) {
            inner.invites.insert(code.to_string(), server_id.to_string());
        }
    }

    /// Ids of the servers an account is a member of.
    pub fn joined_ids(&self, account_id: &str) -> Vec<String> {
        self.read().memberships.get(account_id).map(|ids| ids.iter().cloned().collect()).unwrap_or_default()
    }

    /// The servers an account is a member of, oldest first.
    pub fn joined(&self, account_id: &str) -> Vec<pb::Server> {
        let inner = self.read();
        let Some(ids) = inner.memberships.get(account_id) else { return vec![] };
        ids.iter().filter_map(|id| inner.servers.get(id).cloned()).collect()
    }

    pub fn is_member(&self, account_id: &str, server_id: &str) -> bool {
        self.read().memberships.get(account_id).is_some_and(|ids| ids.contains(server_id))
    }

    /// Whether two accounts share a server.
    pub fn share_a_server(&self, a: &str, b: &str) -> bool {
        let inner = self.read();
        match (inner.memberships.get(a), inner.memberships.get(b)) {
            (Some(a), Some(b)) => a.intersection(b).next().is_some(),
            _ => false,
        }
    }

    /// Everyone who shares a server with `account_id` and passes `keep`, with
    /// the servers they share. Leaves `account_id` out.
    pub fn neighbours(&self, account_id: &str, keep: impl Fn(&str) -> bool) -> HashMap<String, Vec<String>> {
        let inner = self.read();
        let mut found: HashMap<String, Vec<String>> = HashMap::new();
        let Some(servers) = inner.memberships.get(account_id) else { return found };
        for server_id in servers {
            for member in inner.members.get(server_id).into_iter().flatten() {
                if member != account_id && keep(member) {
                    found.entry(member.clone()).or_default().push(server_id.clone());
                }
            }
        }
        found
    }

    /// The servers two accounts share.
    pub fn shared_servers(&self, a: &str, b: &str) -> Vec<String> {
        let inner = self.read();
        match (inner.memberships.get(a), inner.memberships.get(b)) {
            (Some(a), Some(b)) => a.intersection(b).cloned().collect(),
            _ => vec![],
        }
    }

    /// The members of a server who pass `keep`.
    pub fn members_where(&self, server_id: &str, keep: impl Fn(&str) -> bool) -> Vec<String> {
        self.read().members.get(server_id).into_iter().flatten().filter(|id| keep(id)).cloned().collect()
    }

    /// Discoverable servers, busiest first.
    pub fn discoverable(&self) -> Vec<pb::Server> {
        let inner = self.read();
        let mut servers: Vec<pb::Server> = inner.servers.values().filter(|s| s.discoverable).cloned().collect();
        servers.sort_by(|a, b| b.member_count.cmp(&a.member_count).then_with(|| a.id.cmp(&b.id)));
        servers
    }

    pub fn owned_count(&self, account_id: &str) -> i64 {
        self.read().servers.values().filter(|s| s.owner_id == account_id).count() as i64
    }

    /// Every server known, in id order.
    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.read().servers.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Servers, and how many are discoverable.
    pub fn count(&self) -> (i64, i64) {
        let inner = self.read();
        (inner.servers.len() as i64, inner.servers.values().filter(|s| s.discoverable).count() as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(id: &str, members: i64) -> pb::Server {
        pb::Server { id: id.into(), owner_id: "owner".into(), member_count: members, ..Default::default() }
    }

    fn entry(id: &str, members: &[&str]) -> cpb::ServerEntry {
        cpb::ServerEntry {
            server: Some(server(id, members.len() as i64)),
            member_ids: members.iter().map(|m| m.to_string()).collect(),
            invite_codes: vec![format!("{id}-invite")],
        }
    }

    #[test]
    fn memberships_count_once() {
        let index = Index::default();
        index.insert(server("s1", 1), vec!["owner".into()], vec![], None);
        index.join("mika", "s1");
        index.join("mika", "s1");
        assert_eq!(index.summary("s1").unwrap().member_count, 2);
        assert!(index.is_member("mika", "s1"));
        assert!(index.share_a_server("mika", "owner"));
        index.leave("mika", "s1");
        index.leave("mika", "s1");
        assert_eq!(index.summary("s1").unwrap().member_count, 1);
        assert!(index.joined_ids("mika").is_empty());
        index.join("mika", "nowhere");
        assert!(index.joined_ids("mika").is_empty());
    }

    #[test]
    fn a_registration_is_the_truth_about_its_shard() {
        let index = Index::default();
        index.load_placements(vec![("s1".into(), "a".into()), ("s2".into(), "a".into()), ("s3".into(), "b".into())]);
        assert_eq!(index.placement("s2").as_deref(), Some("a"));
        assert!(index.summary("s1").is_none());

        // Shard a now holds s1 (with a new member) and s4; s2 is gone.
        let gone = index.register("a", vec![entry("s1", &["owner", "mika"]), entry("s4", &["owner"])]);
        assert_eq!(gone, ["s2"]);
        assert_eq!(index.placement("s2"), None);
        assert_eq!(index.placement("s4").as_deref(), Some("a"));
        assert_eq!(index.joined_ids("mika"), ["s1"]);
        assert_eq!(index.shard_sizes()["a"], 2);

        // s1 moved to shard b: registering there takes it, and shard a's next
        // registration (without it) doesn't take it back out.
        index.register("b", vec![entry("s1", &["owner"]), entry("s3", &[])]);
        assert_eq!(index.placement("s1").as_deref(), Some("b"));
        assert!(index.joined_ids("mika").is_empty());
        assert!(index.register("a", vec![entry("s4", &["owner"])]).is_empty());
        assert_eq!(index.placement("s1").as_deref(), Some("b"));
        assert_eq!(index.invite("s1-invite").as_deref(), Some("s1"));
        assert_eq!(index.invite("s2-invite"), None);
        let grouped = index.by_shard(&[]);
        assert_eq!(grouped["a"], ["s4"]);
        assert_eq!(index.by_shard(&["s3".into(), "gone".into()]).len(), 1);
    }

    #[test]
    fn invites_go_with_their_server() {
        let index = Index::default();
        index.insert(server("s1", 1), vec!["owner".into()], vec!["old".into()], None);
        index.index_invite("s1", "new", true);
        index.index_invite("nowhere", "stray", true);
        assert_eq!(index.invite("new").as_deref(), Some("s1"));
        assert_eq!(index.invite("stray"), None);
        index.index_invite("s1", "old", false);
        assert_eq!(index.invite("old"), None);
        index.remove("s1");
        assert_eq!(index.invite("new"), None);
    }
}
