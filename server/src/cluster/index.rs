//! What the directory knows about every community server: its profile, who's
//! in it, its invites' codes, and (in a split instance) which shard holds it. Kept in memory and
//! built from the servers' own files: read at start by a single process, or
//! reported by each shard when it registers.
//!
//! Memberships are what grows with big servers, so they're kept small: each
//! account and each server gets a number while it's in the index, an account
//! that's a ULID is kept as its 128 bits rather than its text, and a
//! membership is two of those numbers. That's about 100 bytes an account and
//! 10 a membership, instead of about 600 a membership.

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use ulid::Ulid;

use crate::cpb;
use crate::pb;

#[derive(Default)]
pub struct Index {
    inner: RwLock<Inner>,
}

#[derive(Default)]
struct Inner {
    servers: HashMap<String, pb::Server>,
    accounts: Accounts,
    /// Server id to its number and its members' numbers.
    places: HashMap<String, Place>,
    /// Server number to its id (`None`: free).
    place_ids: Vec<Option<String>>,
    free_places: Vec<u32>,
    /// Server id to the shard holding it. Also has servers whose shard hasn't
    /// registered since the directory started, which aren't in `servers` yet.
    placements: HashMap<String, String>,
    /// Invite code to the server it's for.
    invites: HashMap<String, String>,
}

struct Place {
    number: u32,
    members: HashSet<u32>,
}

/// An account as the index keeps it: a ULID as its bits (as every account
/// is), in two halves so it packs without padding, or any other id as its
/// text.
enum Name {
    Ulid([u64; 2]),
    Text(Box<str>),
}

/// The numbers of the servers an account is in, in order: most accounts are
/// in one, which takes no allocation of its own.
enum Places {
    One(u32),
    Many(Box<[u32]>),
}

impl Places {
    fn as_slice(&self) -> &[u32] {
        match self {
            Places::One(place) => std::slice::from_ref(place),
            Places::Many(places) => places,
        }
    }

    fn from_vec(mut places: Vec<u32>) -> Self {
        match places.len() {
            1 => Places::One(places[0]),
            _ => {
                places.shrink_to_fit();
                Places::Many(places.into_boxed_slice())
            }
        }
    }
}

/// The accounts in at least one server, by number.
#[derive(Default)]
struct Accounts {
    ulids: HashMap<[u64; 2], u32>,
    texts: HashMap<Box<str>, u32>,
    /// By number: who it is and the numbers of the servers it's in, in
    /// order (`None`: free).
    slots: Vec<Option<(Name, Places)>>,
    free: Vec<u32>,
}

/// The id as its ULID bits, if it is one exactly as `id::new_id` writes it.
fn ulid_bits(id: &str) -> Option<[u64; 2]> {
    let ulid = Ulid::from_string(id).ok()?;
    let mut buf = [0; ulid::ULID_LEN];
    (ulid.array_to_str(&mut buf) == id).then_some([(ulid.0 >> 64) as u64, ulid.0 as u64])
}

impl Accounts {
    fn number(&self, id: &str) -> Option<u32> {
        match ulid_bits(id) {
            Some(bits) => self.ulids.get(&bits).copied(),
            None => self.texts.get(id).copied(),
        }
    }

    fn number_or_new(&mut self, id: &str) -> u32 {
        if let Some(number) = self.number(id) {
            return number;
        }
        let name = match ulid_bits(id) {
            Some(bits) => Name::Ulid(bits),
            None => Name::Text(id.into()),
        };
        let number = match self.free.pop() {
            Some(number) => number,
            None => {
                self.slots.push(None);
                u32::try_from(self.slots.len() - 1).expect("fewer than 4 billion accounts")
            }
        };
        match &name {
            Name::Ulid(bits) => self.ulids.insert(*bits, number),
            Name::Text(text) => self.texts.insert(text.clone(), number),
        };
        self.slots[number as usize] = Some((name, Places::Many(Box::default())));
        number
    }

    /// The servers an account is in, by number.
    fn servers(&self, number: u32) -> &[u32] {
        self.slots.get(number as usize).and_then(Option::as_ref).map_or(&[], |(_, servers)| servers.as_slice())
    }

    /// Calls `f` with the account's id, without keeping a copy of it.
    fn with_id<R>(&self, number: u32, f: impl FnOnce(&str) -> R) -> R {
        match self.slots.get(number as usize).and_then(Option::as_ref).map(|(name, _)| name) {
            Some(Name::Ulid([high, low])) => {
                f(Ulid((u128::from(*high) << 64) | u128::from(*low)).array_to_str(&mut [0; ulid::ULID_LEN]))
            }
            Some(Name::Text(text)) => f(text),
            None => f(""),
        }
    }

    fn id(&self, number: u32) -> String {
        self.with_id(number, str::to_string)
    }

    fn add(&mut self, number: u32, place: u32) {
        let Some(Some((_, servers))) = self.slots.get_mut(number as usize) else { return };
        if let Err(at) = servers.as_slice().binary_search(&place) {
            let mut grown = servers.as_slice().to_vec();
            grown.insert(at, place);
            *servers = Places::from_vec(grown);
        }
    }

    /// Takes a server off an account's, and the account out when it was its last.
    fn remove(&mut self, number: u32, place: u32) {
        let Some(Some((name, servers))) = self.slots.get_mut(number as usize) else { return };
        if let Ok(at) = servers.as_slice().binary_search(&place) {
            let mut shrunk = servers.as_slice().to_vec();
            shrunk.remove(at);
            *servers = Places::from_vec(shrunk);
        }
        if servers.as_slice().is_empty() {
            match name {
                Name::Ulid(bits) => self.ulids.remove(bits),
                Name::Text(text) => self.texts.remove(&**text),
            };
            self.slots[number as usize] = None;
            self.free.push(number);
        }
    }
}

impl Inner {
    fn insert(&mut self, server: pb::Server, member_ids: Vec<String>, invite_codes: Vec<String>) {
        let id = server.id.clone();
        self.forget_members(&id);
        let number = match self.free_places.pop() {
            Some(number) => number,
            None => {
                self.place_ids.push(None);
                u32::try_from(self.place_ids.len() - 1).expect("fewer than 4 billion servers")
            }
        };
        self.place_ids[number as usize] = Some(id.clone());
        let mut members = HashSet::with_capacity(member_ids.len());
        for member in &member_ids {
            let account = self.accounts.number_or_new(member);
            self.accounts.add(account, number);
            members.insert(account);
        }
        self.places.insert(id.clone(), Place { number, members });
        self.invites.retain(|_, server_id| *server_id != id);
        for code in invite_codes {
            self.invites.insert(code, id.clone());
        }
        self.servers.insert(id, server);
    }

    fn forget_members(&mut self, server_id: &str) {
        let Some(place) = self.places.remove(server_id) else { return };
        for member in place.members {
            self.accounts.remove(member, place.number);
        }
        self.place_ids[place.number as usize] = None;
        self.free_places.push(place.number);
    }

    fn remove(&mut self, server_id: &str) {
        self.forget_members(server_id);
        self.servers.remove(server_id);
        self.placements.remove(server_id);
        self.invites.retain(|_, id| id != server_id);
    }

    fn place_id(&self, number: u32) -> &str {
        self.place_ids.get(number as usize).and_then(Option::as_deref).unwrap_or_default()
    }

    /// The ids of the servers an account is in, in id order (oldest first).
    fn server_ids(&self, account_id: &str) -> Vec<String> {
        let Some(account) = self.accounts.number(account_id) else { return vec![] };
        let mut ids: Vec<String> =
            self.accounts.servers(account).iter().map(|&n| self.place_id(n).to_string()).collect();
        ids.sort();
        ids
    }

    /// The server numbers two accounts share.
    fn shared(&self, a: &str, b: &str) -> Vec<u32> {
        let (Some(a), Some(b)) = (self.accounts.number(a), self.accounts.number(b)) else { return vec![] };
        let b = self.accounts.servers(b);
        self.accounts.servers(a).iter().filter(|n| b.binary_search(n).is_ok()).copied().collect()
    }

    /// Whether an account (by number) is in a server (by number).
    fn in_place(&self, account: u32, place: u32) -> bool {
        self.accounts.servers(account).binary_search(&place).is_ok()
    }

    /// A server's members who pass `keep`, which holds only for
    /// `candidates` (`count` of them), walking whichever is smaller: the
    /// server's members, or the candidates.
    fn members_among<'a>(
        &self,
        place: &Place,
        candidates: impl Iterator<Item = &'a str>,
        count: usize,
        keep: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        if count < place.members.len() {
            candidates
                .filter(|id| keep(id) && self.accounts.number(id).is_some_and(|n| self.in_place(n, place.number)))
                .map(str::to_string)
                .collect()
        } else {
            place.members.iter().filter(|&&n| self.accounts.with_id(n, &keep)).map(|&n| self.accounts.id(n)).collect()
        }
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
                tracing::warn!(from = %other, to = %shard, "a server moved shards");
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
        let Some(place) = inner.places.get(server_id).map(|p| p.number) else { return };
        let account = inner.accounts.number_or_new(account_id);
        inner.accounts.add(account, place);
        let added = inner.places.get_mut(server_id).is_some_and(|p| p.members.insert(account));
        if added && let Some(server) = inner.servers.get_mut(server_id) {
            server.member_count += 1;
        }
    }

    /// Someone left. Hearing of it twice counts it once.
    pub fn leave(&self, account_id: &str, server_id: &str) {
        let mut inner = self.write();
        let Some(account) = inner.accounts.number(account_id) else { return };
        let Some(place) = inner.places.get_mut(server_id) else { return };
        let (number, removed) = (place.number, place.members.remove(&account));
        inner.accounts.remove(account, number);
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
        self.read().server_ids(account_id)
    }

    /// The servers an account is a member of, oldest first.
    pub fn joined(&self, account_id: &str) -> Vec<pb::Server> {
        let inner = self.read();
        inner.server_ids(account_id).iter().filter_map(|id| inner.servers.get(id).cloned()).collect()
    }

    pub fn is_member(&self, account_id: &str, server_id: &str) -> bool {
        let inner = self.read();
        match (inner.accounts.number(account_id), inner.places.get(server_id)) {
            (Some(account), Some(place)) => inner.in_place(account, place.number),
            _ => false,
        }
    }

    /// Whether two accounts share a server.
    pub fn share_a_server(&self, a: &str, b: &str) -> bool {
        !self.read().shared(a, b).is_empty()
    }

    /// Everyone who shares a server with `account_id` and passes `keep`, with
    /// the servers they share. Leaves `account_id` out. `keep` holds only for
    /// `candidates` (`count` of them): each server's members are walked only
    /// when it has fewer than `count`, and a bigger one is asked about each
    /// candidate instead.
    pub fn neighbours<'a>(
        &self,
        account_id: &str,
        candidates: impl Iterator<Item = &'a str> + Clone,
        count: usize,
        keep: impl Fn(&str) -> bool,
    ) -> HashMap<String, Vec<String>> {
        let inner = self.read();
        let mut found: HashMap<String, Vec<String>> = HashMap::new();
        let Some(account) = inner.accounts.number(account_id) else { return found };
        for &number in inner.accounts.servers(account) {
            let server_id = inner.place_id(number);
            let Some(place) = inner.places.get(server_id) else { continue };
            for member in inner.members_among(place, candidates.clone(), count, |id| id != account_id && keep(id)) {
                found.entry(member).or_default().push(server_id.to_string());
            }
        }
        found
    }

    /// Everyone who shares a server with `account_id` and passes `keep`, with
    /// the servers they share (0 `large`: no server is large). A large server's members aren't
    /// walked: only people found through a smaller one, or in `also`, are
    /// looked for in it, so a change there costs what's on screen, not its size.
    pub fn audience(
        &self,
        account_id: &str,
        large: usize,
        keep: impl Fn(&str) -> bool,
        also: &HashSet<String>,
    ) -> HashMap<String, Vec<String>> {
        let inner = self.read();
        let accounts = &inner.accounts;
        let mut found: HashMap<String, Vec<String>> = HashMap::new();
        let Some(account) = accounts.number(account_id) else { return found };
        let mut big = Vec::new();
        for &number in accounts.servers(account) {
            let server_id = inner.place_id(number);
            let Some(place) = inner.places.get(server_id) else { continue };
            if large > 0 && place.members.len() >= large {
                big.push((server_id, place));
                continue;
            }
            for &member in &place.members {
                if member != account && accounts.with_id(member, &keep) {
                    found.entry(accounts.id(member)).or_default().push(server_id.to_string());
                }
            }
        }
        for (server_id, place) in big {
            // Walk whichever side is smaller: the server, or the people
            // already found plus `also`.
            let candidates: Vec<String> = if found.len() + also.len() < place.members.len() {
                found
                    .keys()
                    .chain(also.iter())
                    .filter(|id| accounts.number(id).is_some_and(|n| place.members.contains(&n)))
                    .cloned()
                    .collect()
            } else {
                place
                    .members
                    .iter()
                    .filter(|&&n| accounts.with_id(n, |id| found.contains_key(id) || also.contains(id)))
                    .map(|&n| accounts.id(n))
                    .collect()
            };
            for member in candidates {
                if member != account_id && keep(&member) {
                    let entry = found.entry(member).or_default();
                    if !entry.iter().any(|id| id == server_id) {
                        entry.push(server_id.to_string());
                    }
                }
            }
        }
        found
    }

    /// The servers of `account_id` smaller than `large` (0: every one),
    /// smallest first while their members add up to at most `budget`, and
    /// the room left.
    pub fn smallest_servers(&self, account_id: &str, large: usize, budget: usize) -> (HashMap<String, usize>, usize) {
        let inner = self.read();
        let numbers = inner.accounts.number(account_id).map_or(&[][..], |n| inner.accounts.servers(n));
        let mut sizes: Vec<(usize, &str)> = numbers
            .iter()
            .map(|&n| inner.place_id(n))
            .filter_map(|id| inner.places.get(id).map(|p| (p.members.len(), id)))
            .filter(|(size, _)| large == 0 || *size < large)
            .collect();
        sizes.sort();
        let (mut shown, mut room) = (HashMap::new(), budget);
        for (size, id) in sizes {
            if size > room {
                break;
            }
            room -= size;
            shown.insert(id.to_string(), size);
        }
        (shown, room)
    }

    /// How many members a server has here.
    pub fn size(&self, server_id: &str) -> usize {
        self.read().places.get(server_id).map_or(0, |p| p.members.len())
    }

    /// The servers two accounts share, in id order.
    pub fn shared_servers(&self, a: &str, b: &str) -> Vec<String> {
        let inner = self.read();
        let mut ids: Vec<String> = inner.shared(a, b).into_iter().map(|n| inner.place_id(n).to_string()).collect();
        ids.sort();
        ids
    }

    /// The members of a server who pass `keep`.
    pub fn members_where(&self, server_id: &str, keep: impl Fn(&str) -> bool) -> Vec<String> {
        let inner = self.read();
        let Some(place) = inner.places.get(server_id) else { return vec![] };
        inner.members_among(place, std::iter::empty(), usize::MAX, keep)
    }

    /// The members of a server who pass `keep`, which holds only for
    /// `candidates` (`count` of them). Costs the smaller of the two, not the
    /// server's size.
    pub fn members_among<'a>(
        &self,
        server_id: &str,
        candidates: impl Iterator<Item = &'a str>,
        count: usize,
        keep: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        let inner = self.read();
        let Some(place) = inner.places.get(server_id) else { return vec![] };
        inner.members_among(place, candidates, count, keep)
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
    fn audience_in_large_servers_is_only_their_members() {
        let index = Index::default();
        index.insert(server("big", 3), vec!["owner".into(), "mika".into(), "nana".into()], vec![], None);
        index.insert(server("small", 2), vec!["mika".into(), "kai".into()], vec![], None);
        let mut also: HashSet<String> = (0..100).map(|n| format!("elsewhere{n}")).collect();
        also.insert("nana".into());
        let audience = index.audience("mika", 3, |_| true, &also);
        let mut got: Vec<_> = audience.into_iter().collect();
        got.sort();
        assert_eq!(got, vec![("kai".to_string(), vec!["small".to_string()]), ("nana".into(), vec!["big".into()])]);
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
    fn ulid_accounts_come_back_as_they_went_in() {
        let index = Index::default();
        let ids: Vec<String> = (0..5).map(|_| crate::id::new_id()).collect();
        index.insert(server("s1", 5), ids.clone(), vec![], None);
        index.insert(server("s2", 2), vec![ids[0].clone(), "text-id".into()], vec![], None);
        let mut members = index.members_where("s1", |_| true);
        members.sort();
        assert_eq!(members, ids);
        assert!(index.is_member(&ids[0], "s2"));
        // Lowercase is another id, not the same account.
        assert!(!index.is_member(&ids[0].to_lowercase(), "s1"));
        assert_eq!(index.shared_servers(&ids[0], "text-id"), ["s2"]);
        assert_eq!(index.joined_ids(&ids[0]), ["s1", "s2"]);
        index.remove("s1");
        assert_eq!(index.joined_ids(&ids[1]), Vec::<String>::new());
        assert_eq!(index.joined_ids(&ids[0]), ["s2"]);
        // A freed number goes to someone new without mixing them up.
        index.join("newcomer", "s2");
        assert_eq!(index.size("s2"), 3);
        assert!(!index.is_member(&ids[1], "s2"));
    }

    #[test]
    fn members_among_walks_the_smaller_side() {
        let index = Index::default();
        let members: Vec<String> = (0..50).map(|n| format!("m{n}")).collect();
        index.insert(server("big", 50), members, vec![], None);
        let few = ["m1", "m7", "stranger"];
        let mut got = index.members_among("big", few.iter().copied(), few.len(), |id| few.contains(&id) && id != "m7");
        got.sort();
        assert_eq!(got, ["m1"]);
        // More candidates than members: the members are walked instead, alike.
        let many: Vec<String> = (0..100).map(|n| format!("m{n}")).collect();
        let got = index.members_among("big", many.iter().map(String::as_str), many.len(), |id| id == "m3");
        assert_eq!(got, ["m3"]);

        index.insert(server("small", 2), vec!["m1".into(), "x".into()], vec![], None);
        let near = index.neighbours("m1", few.iter().copied(), few.len(), |id| few.contains(&id));
        assert_eq!(near.into_keys().collect::<Vec<_>>(), ["m7"]);
        let near = index.neighbours("m1", std::iter::empty(), 0, |_| false);
        assert!(near.is_empty());
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
