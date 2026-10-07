//! How your servers sit on the rail, on one instance: top to bottom, each a
//! server or a folder of them. The instance keeps it on your account
//! (`AccountService.GetServerArrangement`), so every device shows the same.
//! A port of the web app's `lib/rail.ts` (and the arranging calls in
//! `fuwa/actions.ts`); which folders are open stays on this computer
//! (`Prefs::rail_open`, the web's `lib/rail-open.ts`).

use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::rpc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RailFolder {
    pub id: String,
    pub name: String,
    /// 0xRRGGBB, or 0 for the theme's accent.
    pub color: u32,
    pub servers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RailEntry {
    Server(String),
    Folder(RailFolder),
}

pub type RailLayout = Vec<RailEntry>;

/// Where a dragged server or folder lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RailDrop {
    /// Into `folder` ("" for none), before `before` (a server in that folder,
    /// or a top-level server or folder id when `folder` is ""), at the end when it's None.
    Server { id: String, folder: String, before: Option<String> },
    /// Onto the top-level server `with`, making a folder of the two.
    Combine { id: String, with: String },
    /// A folder before the top-level entry `before`, or at the end.
    Folder { id: String, before: Option<String> },
}

impl RailDrop {
    /// What moves.
    pub fn id(&self) -> &str {
        match self {
            RailDrop::Server { id, .. } | RailDrop::Combine { id, .. } | RailDrop::Folder { id, .. } => id,
        }
    }
}

/// Folder names, as the instance takes them.
pub const FOLDER_NAME_MAX: usize = 32;

/// Folder colors to pick from, 0xRRGGBB; 0 is the theme's accent.
pub const FOLDER_COLORS: [u32; 9] = [0, 0xff6b9d, 0xff8a5c, 0xf5c044, 0x6ad48a, 0x4cc9d6, 0x6c8dff, 0xa77bff, 0x8a8f9c];

pub fn key_of(e: &RailEntry) -> &str {
    match e {
        RailEntry::Server(id) => id,
        RailEntry::Folder(f) => &f.id,
    }
}

/// Every server in the layout, top to bottom.
pub fn servers_in(layout: &[RailEntry]) -> Vec<&str> {
    layout
        .iter()
        .flat_map(|e| match e {
            RailEntry::Server(id) => vec![id.as_str()],
            RailEntry::Folder(f) => f.servers.iter().map(String::as_str).collect(),
        })
        .collect()
}

/// The layout to show: the saved one without servers you're no longer in,
/// then servers it doesn't place, in the order you joined them.
pub fn rail_layout(saved: Option<&[RailEntry]>, servers: &[String]) -> RailLayout {
    let mut seen = std::collections::HashSet::new();
    let mut keep = |id: &str| servers.iter().any(|s| s == id) && seen.insert(id.to_owned());
    let mut placed = Vec::new();
    for e in saved.unwrap_or_default() {
        match e {
            RailEntry::Server(id) => {
                if keep(id) {
                    placed.push(e.clone());
                }
            }
            RailEntry::Folder(f) => {
                let kept: Vec<String> = f.servers.iter().filter(|s| keep(s)).cloned().collect();
                if !kept.is_empty() {
                    placed.push(RailEntry::Folder(RailFolder { servers: kept, ..f.clone() }));
                }
            }
        }
    }
    for id in servers {
        if keep(id) {
            placed.push(RailEntry::Server(id.clone()));
        }
    }
    placed
}

fn insert<T>(list: &mut Vec<T>, item: T, at: Option<usize>) {
    match at {
        Some(at) if at <= list.len() => list.insert(at, item),
        _ => list.push(item),
    }
}

/// The layout after a drop. Dropping something where it already is gives back an equal layout.
pub fn move_rail(layout: &[RailEntry], drop: &RailDrop, new_id: impl FnOnce() -> String) -> RailLayout {
    let position = |list: &[RailEntry], before: &Option<String>| -> Option<usize> {
        before.as_ref().map(|b| list.iter().position(|e| key_of(e) == b).unwrap_or(usize::MAX))
    };
    if let RailDrop::Folder { id, before } = drop {
        let Some(at) = layout.iter().position(|e| matches!(e, RailEntry::Folder(f) if f.id == *id)) else {
            return layout.to_vec();
        };
        if before.as_deref() == Some(id.as_str()) {
            return layout.to_vec();
        }
        let mut rest = layout.to_vec();
        let moving = rest.remove(at);
        let to = position(&rest, before);
        insert(&mut rest, moving, to);
        return rest;
    }
    let id = drop.id();
    if !servers_in(layout).contains(&id) {
        return layout.to_vec();
    }
    match drop {
        RailDrop::Combine { with, .. } if with == id => return layout.to_vec(),
        RailDrop::Server { before: Some(b), .. } if b == id => return layout.to_vec(),
        _ => {}
    }
    // Out of wherever it was; a folder left empty goes too.
    let rest: RailLayout = layout
        .iter()
        .filter_map(|e| match e {
            RailEntry::Server(s) if s == id => None,
            RailEntry::Folder(f) if f.servers.iter().any(|s| s == id) => {
                let servers: Vec<String> = f.servers.iter().filter(|s| *s != id).cloned().collect();
                (!servers.is_empty()).then(|| RailEntry::Folder(RailFolder { servers, ..f.clone() }))
            }
            _ => Some(e.clone()),
        })
        .collect();
    match drop {
        RailDrop::Combine { with, .. } => {
            let Some(at) = rest.iter().position(|e| matches!(e, RailEntry::Server(s) if s == with)) else {
                return layout.to_vec();
            };
            let mut rest = rest;
            rest[at] = RailEntry::Folder(RailFolder {
                id: new_id(),
                name: String::new(),
                color: 0,
                servers: vec![with.clone(), id.to_owned()],
            });
            rest
        }
        RailDrop::Server { folder, before, .. } if folder.is_empty() => {
            let mut rest = rest;
            let to = position(&rest, before);
            insert(&mut rest, RailEntry::Server(id.to_owned()), to);
            rest
        }
        RailDrop::Server { folder, before, .. } => {
            if !rest.iter().any(|e| matches!(e, RailEntry::Folder(f) if f.id == *folder)) {
                return layout.to_vec();
            }
            rest.into_iter()
                .map(|e| match e {
                    RailEntry::Folder(mut f) if f.id == *folder => {
                        let to = before.as_ref().map(|b| f.servers.iter().position(|s| s == b).unwrap_or(usize::MAX));
                        insert(&mut f.servers, id.to_owned(), to);
                        RailEntry::Folder(f)
                    }
                    other => other,
                })
                .collect()
        }
        RailDrop::Folder { .. } => unreachable!(),
    }
}

/// One step up or down, for the keyboard. A server moves past its neighbour,
/// into an open folder it meets (and out of one at its ends); past a closed
/// folder it goes by. A folder moves past the next top-level entry. None
/// when it can't go further.
pub fn step_rail(layout: &[RailEntry], id: &str, by: isize, open: impl Fn(&str) -> bool) -> Option<RailLayout> {
    let key_at = |n: isize| -> Option<String> {
        usize::try_from(n).ok().and_then(|n| layout.get(n)).map(|e| key_of(e).to_owned())
    };
    let fixed = || -> String { unreachable!("folders are only made by combining") };
    if let Some(top) = layout.iter().position(|e| key_of(e) == id) {
        let top = top as isize;
        if matches!(layout[top as usize], RailEntry::Folder(_)) {
            let to = top + by;
            if to < 0 || to >= layout.len() as isize {
                return None;
            }
            let before = if by < 0 { key_at(to) } else { key_at(to + 1) };
            return Some(move_rail(layout, &RailDrop::Folder { id: id.to_owned(), before }, fixed));
        }
        let next = usize::try_from(top + by).ok().and_then(|n| layout.get(n))?;
        if let RailEntry::Folder(f) = next
            && open(&f.id)
        {
            let before = if by > 0 { f.servers.first().cloned() } else { None };
            let drop = RailDrop::Server { id: id.to_owned(), folder: f.id.clone(), before };
            return Some(move_rail(layout, &drop, fixed));
        }
        let before = if by < 0 { Some(key_of(next).to_owned()) } else { key_at(top + 2) };
        return Some(move_rail(layout, &RailDrop::Server { id: id.to_owned(), folder: String::new(), before }, fixed));
    }
    let at = layout.iter().position(|e| matches!(e, RailEntry::Folder(f) if f.servers.iter().any(|s| s == id)))?;
    let RailEntry::Folder(folder) = &layout[at] else { return None };
    let n = folder.servers.iter().position(|s| s == id)? as isize;
    if n + by >= 0 && n + by < folder.servers.len() as isize {
        let before = if by < 0 {
            folder.servers.get((n - 1) as usize).cloned()
        } else {
            folder.servers.get((n + 2) as usize).cloned()
        };
        let drop = RailDrop::Server { id: id.to_owned(), folder: folder.id.clone(), before };
        return Some(move_rail(layout, &drop, fixed));
    }
    // Out of the folder: just above it, or just below.
    let before = if by < 0 { Some(folder.id.clone()) } else { layout.get(at + 1).map(|e| key_of(e).to_owned()) };
    Some(move_rail(layout, &RailDrop::Server { id: id.to_owned(), folder: String::new(), before }, fixed))
}

/// A change to one folder.
#[derive(Debug, Clone, Default)]
pub struct FolderChange {
    pub name: Option<String>,
    pub color: Option<u32>,
}

/// Changes one folder; `None` dissolves it, leaving its servers where it was.
pub fn edit_folder(layout: &[RailEntry], id: &str, change: Option<FolderChange>) -> RailLayout {
    layout
        .iter()
        .flat_map(|e| match e {
            RailEntry::Folder(f) if f.id == id => match &change {
                None => f.servers.iter().map(|s| RailEntry::Server(s.clone())).collect(),
                Some(c) => vec![RailEntry::Folder(RailFolder {
                    name: clean_name(c.name.as_deref().unwrap_or(&f.name)),
                    color: c.color.unwrap_or(f.color),
                    ..f.clone()
                })],
            },
            _ => vec![e.clone()],
        })
        .collect()
}

/// Puts a server into a new folder of its own, where it is.
pub fn folder_of(layout: &[RailEntry], server: &str, new_id: impl FnOnce() -> String) -> RailLayout {
    let mut new_id = Some(new_id);
    layout
        .iter()
        .map(|e| match e {
            RailEntry::Server(s) if s == server => RailEntry::Folder(RailFolder {
                id: new_id.take().map(|f| f()).unwrap_or_default(),
                name: String::new(),
                color: 0,
                servers: vec![server.to_owned()],
            }),
            _ => e.clone(),
        })
        .collect()
}

/// A new folder's id: random, in the characters the instance takes.
pub fn folder_id() -> String {
    let mut bytes = [0u8; 9];
    if getrandom::fill(&mut bytes).is_err() {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed) ^ crate::core::dms::now_ms() as u64;
        bytes[..8].copy_from_slice(&n.to_le_bytes());
    }
    let digits: String = bytes.iter().map(|b| radix36(*b)).collect();
    format!("f-{}", digits.chars().take(14).collect::<String>())
}

/// A byte in base 36, two digits.
fn radix36(b: u8) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let (hi, lo) = (b / 36, b % 36);
    format!("{}{}", DIGITS[hi as usize] as char, DIGITS[lo as usize] as char)
}

/// A folder's name as shown: its own, or its servers' names.
pub fn folder_label(folder: &RailFolder, name_of: impl Fn(&str) -> Option<String>) -> String {
    if !folder.name.is_empty() {
        return folder.name.clone();
    }
    let names: Vec<String> = folder.servers.iter().filter_map(|id| name_of(id)).filter(|n| !n.is_empty()).collect();
    if names.len() > 3 {
        t_with(
            "workspace.rail.folder.more",
            &[("names", Arg::Str(&names[..3].join(", "))), ("count", Arg::Num(names.len() as i64 - 3))],
        )
    } else if names.is_empty() {
        t("workspace.rail.folder.default")
    } else {
        names.join(", ")
    }
}

/// A folder name as the instance takes it: no invisible or control characters, trimmed, 32 characters at most.
pub fn clean_name(name: &str) -> String {
    let plain: String = name.chars().filter(|c| !c.is_control() && !is_format(*c)).collect();
    plain.trim().chars().take(FOLDER_NAME_MAX).collect::<String>().trim().to_owned()
}

/// Unicode's format characters (Cf) that can hide in a name: zero-width ones,
/// direction marks and overrides, isolates, the byte order mark and the like.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

pub fn from_items(items: &[pb::ServerRailItem]) -> RailLayout {
    items
        .iter()
        .filter_map(|i| match &i.item {
            Some(pb::server_rail_item::Item::ServerId(id)) => Some(RailEntry::Server(id.clone())),
            Some(pb::server_rail_item::Item::Folder(f)) => Some(RailEntry::Folder(RailFolder {
                id: f.id.clone(),
                name: f.name.clone(),
                color: f.color,
                servers: f.server_ids.clone(),
            })),
            None => None,
        })
        .collect()
}

pub fn to_items(layout: &[RailEntry]) -> Vec<pb::ServerRailItem> {
    layout
        .iter()
        .map(|e| pb::ServerRailItem {
            item: Some(match e {
                RailEntry::Server(id) => pb::server_rail_item::Item::ServerId(id.clone()),
                RailEntry::Folder(f) => pb::server_rail_item::Item::Folder(pb::ServerFolder {
                    id: f.id.clone(),
                    name: f.name.clone(),
                    color: f.color,
                    server_ids: f.servers.clone(),
                }),
            }),
        })
        .collect()
}

/// Arrangements sent, so only the latest answer lands.
static RAIL_WRITES: AtomicU64 = AtomicU64::new(0);

impl Core {
    /// Reads how you arranged your servers again, for changes made on another device.
    /// An older instance without arrangements just keeps the order you joined in.
    pub(crate) async fn refresh_rail(&self, key: &str) {
        let Some(api) = self.api(key) else { return };
        let before = RAIL_WRITES.load(Ordering::SeqCst);
        let Ok(res) = rpc!(api.account(), get_server_arrangement(pb::GetServerArrangementRequest {})).await else {
            return;
        };
        // A change made here meanwhile is newer than what this read saw.
        if RAIL_WRITES.load(Ordering::SeqCst) != before {
            return;
        }
        let rail = res.updated_at.is_some().then(|| from_items(&res.items));
        self.shared.instance(key, |i| {
            if i.rail != rail {
                i.rail = rail;
            }
        });
    }

    /// Arranges your servers on an instance's rail, on every device: shown at
    /// once, put back if the instance refuses it.
    pub async fn arrange_servers(&self, key: &str, layout: RailLayout) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(tonic::Code::NotFound, "That instance isn't here."))?;
        let n = RAIL_WRITES.fetch_add(1, Ordering::SeqCst) + 1;
        let before = self.shared.instance(key, |i| i.rail.replace(layout.clone())).flatten();
        match rpc!(api.account(), set_server_arrangement(pb::SetServerArrangementRequest { items: to_items(&layout) }))
            .await
        {
            Ok(res) => {
                if RAIL_WRITES.load(Ordering::SeqCst) == n {
                    // The instance drops servers you left meanwhile; anything else stays as shown.
                    let saved = from_items(&res.items);
                    self.shared.instance(key, |i| {
                        if i.rail.as_ref() != Some(&saved) {
                            i.rail = Some(saved);
                        }
                    });
                }
                Ok(())
            }
            Err(err) => {
                if RAIL_WRITES.load(Ordering::SeqCst) == n {
                    self.shared.instance(key, |i| i.rail = before);
                }
                Err(err)
            }
        }
    }

    /// Whether a rail folder is open on this computer.
    pub fn folder_open(&self, key: &str, folder: &str) -> bool {
        self.prefs().rail_open.contains(&format!("{key}/{folder}"))
    }

    /// Opens or closes a rail folder, on this computer only.
    pub fn set_folder_open(&self, key: &str, folder: &str, open: bool) {
        let at = format!("{key}/{folder}");
        if self.prefs().rail_open.contains(&at) == open {
            return;
        }
        self.set_prefs(|p| {
            if open {
                p.rail_open.insert(at);
            } else {
                p.rail_open.remove(&at);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: &str) -> RailEntry {
        RailEntry::Server(id.into())
    }

    fn f(id: &str, servers: &[&str]) -> RailEntry {
        RailEntry::Folder(RailFolder {
            id: id.into(),
            name: String::new(),
            color: 0,
            servers: servers.iter().map(|s| s.to_string()).collect(),
        })
    }

    fn fixed() -> String {
        "new".into()
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn server(id: &str, folder: &str, before: Option<&str>) -> RailDrop {
        RailDrop::Server { id: id.into(), folder: folder.into(), before: before.map(str::to_owned) }
    }

    #[test]
    fn servers_you_left_drop_out_new_ones_go_last_folders_left_empty_go() {
        let saved = vec![s("b"), f("g", &["gone", "c"]), f("empty", &["gone"]), s("gone"), s("a")];
        assert_eq!(
            rail_layout(Some(&saved), &ids(&["a", "b", "c", "d"])),
            vec![s("b"), f("g", &["c"]), s("a"), s("d")]
        );
        assert_eq!(rail_layout(None, &ids(&["a", "b"])), vec![s("a"), s("b")]);
        // A server placed twice keeps its first place.
        assert_eq!(rail_layout(Some(&[s("a"), f("g", &["a", "b"])]), &ids(&["a", "b"])), vec![s("a"), f("g", &["b"])]);
    }

    #[test]
    fn servers_move_between_top_level_places_and_folders() {
        let layout = vec![s("a"), f("g", &["b", "c"]), s("d")];
        assert_eq!(move_rail(&layout, &server("d", "", Some("a")), fixed), vec![s("d"), s("a"), f("g", &["b", "c"])]);
        assert_eq!(move_rail(&layout, &server("a", "g", Some("c")), fixed), vec![f("g", &["b", "a", "c"]), s("d")]);
        assert_eq!(move_rail(&layout, &server("b", "", None), fixed), vec![s("a"), f("g", &["c"]), s("d"), s("b")]);
        // The last one out takes the folder with it.
        let lone = vec![f("g", &["b"]), s("a")];
        assert_eq!(move_rail(&lone, &server("b", "", None), fixed), vec![s("a"), s("b")]);
        // Dropping where it is changes nothing.
        assert_eq!(move_rail(&layout, &server("a", "", Some("g")), fixed), layout);
        assert_eq!(move_rail(&layout, &server("b", "g", Some("c")), fixed), layout);
    }

    #[test]
    fn dropping_a_server_onto_another_makes_a_folder_in_its_place() {
        let layout = vec![s("a"), s("b"), s("c")];
        let combine = |id: &str, with: &str| RailDrop::Combine { id: id.into(), with: with.into() };
        assert_eq!(move_rail(&layout, &combine("c", "a"), fixed), vec![f("new", &["a", "c"]), s("b")]);
        assert_eq!(move_rail(&layout, &combine("a", "a"), fixed), layout);
    }

    #[test]
    fn folders_move_as_a_whole() {
        let layout = vec![s("a"), f("g", &["b"]), s("c")];
        let folder = |before: Option<&str>| RailDrop::Folder { id: "g".into(), before: before.map(str::to_owned) };
        assert_eq!(move_rail(&layout, &folder(Some("a")), fixed), vec![f("g", &["b"]), s("a"), s("c")]);
        assert_eq!(move_rail(&layout, &folder(None), fixed), vec![s("a"), s("c"), f("g", &["b"])]);
    }

    #[test]
    fn the_keyboard_steps_into_open_folders_and_past_closed_ones() {
        let layout = vec![s("a"), f("g", &["b", "c"]), s("d")];
        let open = |_: &str| true;
        let closed = |_: &str| false;
        assert_eq!(step_rail(&layout, "a", 1, open), Some(vec![f("g", &["a", "b", "c"]), s("d")]));
        assert_eq!(step_rail(&layout, "a", 1, closed), Some(vec![f("g", &["b", "c"]), s("a"), s("d")]));
        assert_eq!(step_rail(&layout, "d", -1, open), Some(vec![s("a"), f("g", &["b", "c", "d"])]));
        assert_eq!(step_rail(&layout, "b", -1, open), Some(vec![s("a"), s("b"), f("g", &["c"]), s("d")]));
        assert_eq!(step_rail(&layout, "c", 1, open), Some(vec![s("a"), f("g", &["b"]), s("c"), s("d")]));
        assert_eq!(step_rail(&layout, "b", 1, open), Some(vec![s("a"), f("g", &["c", "b"]), s("d")]));
        assert_eq!(step_rail(&layout, "g", 1, open), Some(vec![s("a"), s("d"), f("g", &["b", "c"])]));
        assert_eq!(step_rail(&layout, "a", -1, open), None);
        assert_eq!(step_rail(&layout, "d", 1, open), None);
    }

    #[test]
    fn folders_are_renamed_recolored_and_dissolved_in_place() {
        let layout = vec![s("a"), f("g", &["b", "c"]), s("d")];
        let change = FolderChange { name: Some("  Games  ".into()), color: Some(0xff66aa) };
        assert_eq!(
            edit_folder(&layout, "g", Some(change))[1],
            RailEntry::Folder(RailFolder {
                id: "g".into(),
                name: "Games".into(),
                color: 0xff66aa,
                servers: ids(&["b", "c"])
            })
        );
        let sneaky = FolderChange { name: Some("\u{202e}gnp.exe\u{200b}".into()), color: None };
        assert!(matches!(&edit_folder(&layout, "g", Some(sneaky))[1], RailEntry::Folder(f) if f.name == "gnp.exe"));
        assert_eq!(edit_folder(&layout, "g", None), vec![s("a"), s("b"), s("c"), s("d")]);
        assert_eq!(folder_of(&layout, "d", fixed), vec![s("a"), f("g", &["b", "c"]), f("new", &["d"])]);
        let id = folder_id();
        assert!(id.starts_with("f-") && id.len() <= 32 && id[2..].chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn the_instances_items_round_trip() {
        let layout = vec![s("a"), f("g", &["b", "c"])];
        assert_eq!(from_items(&to_items(&layout)), layout);
    }
}
