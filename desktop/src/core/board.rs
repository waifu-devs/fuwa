//! Boards (docs/boards.md): channels whose columns hold cards people move
//! along, as the web app's `lib/board.ts` and `fuwa/board.ts`. A board is
//! read whenever it's opened (its events may have been missed while it was
//! away) and kept in step by its events after that. A card's body is a
//! message, kept beside it; its comments are that message's thread, which
//! the thread code already keeps. A move or an edit shows at once, with a
//! rank worked out here as the server works it out (`between`, the same as
//! `server/src/api/boards.rs`), and goes back if the server says no.

use std::collections::HashMap;

use crate::core::api::Problem;
use crate::core::{Core, reports};
use crate::pb;
use crate::pb::event::Payload;
use crate::rpc;

/// As the server takes them.
pub const TITLE_MAX: usize = 200;
pub const ASSIGNEES_MAX: usize = 10;
pub const BODY_MAX: usize = 4000;
/// What a board's settings take, as the web's `lib/board.ts`.
pub const COLUMN_NAME_MAX: usize = 50;
pub const LABEL_NAME_MAX: usize = 30;
pub const COLUMNS_MAX: usize = 20;
pub const LABELS_MAX: usize = 50;
pub const CARD_LIMIT_MAX: i32 = 10_000;
/// The colors the board's settings offer for columns and labels.
pub const PALETTE: [i32; 12] = [
    0xf06292, 0xb388ff, 0x7986cb, 0x4fc3f7, 0x4db6ac, 0x81c784, 0xdce775, 0xffd54f, 0xffb74d, 0xff8a65, 0xa1887f,
    0x90a4ae,
];
/// Archived cards asked for at a time.
const PAGE: i32 = 50;

/// One board as this app holds it.
#[derive(Debug, Clone, Default)]
pub struct Board {
    pub columns: Vec<pb::BoardColumn>,
    pub labels: Vec<pb::BoardLabel>,
    /// Not archived, sorted by rank.
    pub cards: Vec<pb::Card>,
    /// Each card's body, by the card's id.
    pub bodies: HashMap<String, pb::Message>,
    pub loaded: bool,
    pub error: Option<String>,
    /// The archived cards, the latest first, while their list is open.
    pub archived: Option<Archived>,
}

#[derive(Debug, Clone, Default)]
pub struct Archived {
    pub cards: Vec<pb::Card>,
    pub bodies: HashMap<String, pb::Message>,
    pub more: bool,
    pub loading: bool,
}

impl Board {
    /// A column's cards, in order.
    pub fn cards_in<'a>(&'a self, column: &'a str) -> impl Iterator<Item = &'a pb::Card> + 'a {
        self.cards.iter().filter(move |c| c.column_id == column)
    }

    /// A card on the board, or in the archived list.
    pub fn card(&self, id: &str) -> Option<&pb::Card> {
        self.cards.iter().find(|c| c.id == id).or_else(|| self.archived.as_ref()?.cards.iter().find(|c| c.id == id))
    }

    /// A card's body, on the board or in the archived list.
    pub fn body(&self, id: &str) -> Option<&pb::Message> {
        self.bodies.get(id).or_else(|| self.archived.as_ref()?.bodies.get(id))
    }

    /// Whether a column holds more cards than its limit says.
    pub fn over_limit(&self, column: &pb::BoardColumn) -> bool {
        column.card_limit > 0 && self.cards_in(&column.id).count() > column.card_limit as usize
    }
}

// ───────────────────────── Ranks ─────────────────────────

const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn digit(c: u8) -> usize {
    DIGITS.iter().position(|d| *d == c).unwrap_or(0)
}

/// A rank between `lo` and `hi`; "" is before everything (lo) or after everything (hi).
pub fn between(lo: &str, hi: &str) -> String {
    let lo: Vec<usize> = lo.bytes().map(digit).collect();
    let hi: Option<Vec<usize>> = (!hi.is_empty()).then(|| hi.bytes().map(digit).collect());
    midpoint(&lo, hi.as_deref()).into_iter().map(|d| DIGITS[d] as char).collect()
}

fn midpoint(lo: &[usize], hi: Option<&[usize]>) -> Vec<usize> {
    if let Some(hi) = hi {
        let shared = (0..hi.len()).take_while(|&i| lo.get(i).copied().unwrap_or(0) == hi[i]).count();
        if shared > 0 {
            let mut out = hi[..shared].to_vec();
            out.extend(midpoint(lo.get(shared..).unwrap_or(&[]), Some(&hi[shared..])));
            return out;
        }
    }
    let a = lo.first().copied().unwrap_or(0);
    let b = hi.map_or(DIGITS.len(), |hi| hi[0]);
    if b - a > 1 {
        return vec![(a + b) / 2];
    }
    if let Some(hi) = hi
        && hi.len() > 1
    {
        return vec![hi[0]];
    }
    let mut out = vec![a];
    out.extend(midpoint(lo.get(1..).unwrap_or(&[]), None));
    out
}

/// How cards sort in a column: by rank, then id, as the server sorts them.
fn by_rank(a: &pb::Card, b: &pb::Card) -> std::cmp::Ordering {
    a.rank.cmp(&b.rank).then_with(|| a.id.cmp(&b.id))
}

/// Puts a card in (or takes it off, archived), in its place.
pub fn put(board: &mut Board, card: pb::Card) {
    board.cards.retain(|c| c.id != card.id);
    if card.archived_at.is_none() {
        board.cards.push(card);
        board.cards.sort_by(by_rank);
    }
}

/// Where a card lands when dropped above `before` in a column (at the bottom
/// when it's empty): the rank the server will give it.
pub fn rank_for(board: &Board, column: &str, before: &str, moving: &str) -> String {
    let cards: Vec<&pb::Card> = board.cards_in(column).filter(|c| c.id != moving).collect();
    let at = if before.is_empty() { None } else { cards.iter().position(|c| c.id == before) };
    let index = at.unwrap_or(cards.len());
    let lo = if index > 0 { cards[index - 1].rank.as_str() } else { "" };
    let hi = cards.get(index).map_or("", |c| c.rank.as_str());
    // Two cards sharing a rank (moved there at once): the server spreads them out.
    if !hi.is_empty() && lo >= hi { lo.to_owned() } else { between(lo, hi) }
}

/// The board with a card moved, as it shows until the server answers.
pub fn moved(board: &mut Board, card: &str, column: &str, before: &str) {
    let Some(had) = board.cards.iter().find(|c| c.id == card).cloned() else { return };
    let rank = rank_for(board, column, before, card);
    put(board, pb::Card { column_id: column.to_owned(), rank, ..had });
}

/// A way to move a card with the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    Up,
    Down,
    Left,
    Right,
}

/// Where a card goes: up or down a place in its column, or to the top of the
/// column beside it, as (column, the card it goes above). None when it can't.
pub fn step(board: &Board, card: &str, way: Way) -> Option<(String, String)> {
    let card = board.cards.iter().find(|c| c.id == card)?;
    let here: Vec<&pb::Card> = board.cards_in(&card.column_id).collect();
    let i = here.iter().position(|c| c.id == card.id)?;
    match way {
        Way::Up => (i > 0).then(|| (card.column_id.clone(), here[i - 1].id.clone())),
        Way::Down => (i + 1 < here.len())
            .then(|| (card.column_id.clone(), here.get(i + 2).map(|c| c.id.clone()).unwrap_or_default())),
        Way::Left | Way::Right => {
            let c = board.columns.iter().position(|col| col.id == card.column_id)?;
            let next = if way == Way::Left { c.checked_sub(1)? } else { c + 1 };
            let next = board.columns.get(next)?;
            Some((next.id.clone(), board.cards_in(&next.id).next().map(|c| c.id.clone()).unwrap_or_default()))
        }
    }
}

// ───────────────────────── Filters and dates ─────────────────────────

/// What narrows the board: cards on someone, with a label, or saying something.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub assignee: Option<String>,
    pub label: Option<String>,
    pub query: String,
}

impl Filter {
    pub fn any(&self) -> bool {
        self.assignee.is_some() || self.label.is_some() || !self.query.trim().is_empty()
    }
}

/// Whether a card shows under a filter: its title or body says every word.
pub fn shows(card: &pb::Card, body: Option<&pb::Message>, filter: &Filter) -> bool {
    if filter.assignee.as_ref().is_some_and(|a| !card.assignee_ids.contains(a)) {
        return false;
    }
    if filter.label.as_ref().is_some_and(|l| !card.label_ids.contains(l)) {
        return false;
    }
    let query = filter.query.to_lowercase();
    let text = format!("{}\n{}", card.title, body.map_or("", |b| b.content.as_str())).to_lowercase();
    query.split_whitespace().all(|w| text.contains(w))
}

/// How a card's due date reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// Past, and not in a done column.
    Late,
    /// Within a day.
    Soon,
    Later,
}

/// Unix milliseconds of a timestamp, 0 for none.
pub fn ms(t: Option<&prost_types::Timestamp>) -> i64 {
    t.map_or(0, |t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000)
}

pub fn at_ms(ms: i64) -> prost_types::Timestamp {
    prost_types::Timestamp { seconds: ms.div_euclid(1000), nanos: (ms.rem_euclid(1000) * 1_000_000) as i32 }
}

pub fn due(card: &pb::Card, column: Option<&pb::BoardColumn>, now: i64) -> Option<Due> {
    let at = ms(card.due_at.as_ref());
    if at == 0 {
        return None;
    }
    Some(if column.is_some_and(|c| c.done) {
        Due::Later
    } else if at < now {
        Due::Late
    } else if at - now < 86_400_000 {
        Due::Soon
    } else {
        Due::Later
    })
}

// ───────────────────────── Events ─────────────────────────

/// The board an event is about, if it's one a board follows.
fn channel_of(p: &Payload) -> Option<&str> {
    match p {
        Payload::BoardUpdated(p) => Some(&p.channel_id),
        Payload::CardDeleted(p) => Some(&p.channel_id),
        Payload::ThreadUpdated(p) => Some(&p.channel_id),
        Payload::MessageDeleted(p) => Some(&p.channel_id),
        Payload::CardUpdated(p) => p.card.as_ref().map(|c| c.channel_id.as_str()),
        Payload::MessageCreated(pb::MessageCreated { message: Some(m) })
        | Payload::MessageUpdated(pb::MessageUpdated { message: Some(m) }) => Some(&m.channel_id),
        _ => None,
    }
}

/// Keeps the boards open here in step with their channel's events. Events
/// can come twice, so each one only sets what it says.
pub fn on_event(boards: &mut HashMap<String, Board>, p: &Payload) {
    let Some(board) = channel_of(p).and_then(|c| boards.get_mut(c)) else { return };
    apply(board, p);
}

pub fn apply(board: &mut Board, p: &Payload) {
    match p {
        Payload::BoardUpdated(p) => {
            board.columns = p.columns.clone();
            board.labels = p.labels.clone();
            let labels: Vec<&str> = p.labels.iter().map(|l| l.id.as_str()).collect();
            for card in &mut board.cards {
                card.label_ids.retain(|id| labels.contains(&id.as_str()));
            }
        }
        Payload::CardUpdated(pb::CardUpdated { card: Some(card) }) => {
            if let Some(archived) = &mut board.archived {
                archived.cards.retain(|c| c.id != card.id);
                if card.archived_at.is_some() {
                    archived.cards.insert(0, card.clone());
                    if let Some(body) = board.bodies.get(&card.id) {
                        archived.bodies.insert(card.id.clone(), body.clone());
                    }
                }
            }
            put(board, card.clone());
        }
        Payload::CardDeleted(p) => forget(board, &p.card_id),
        Payload::MessageDeleted(p) => forget(board, &p.message_id),
        Payload::MessageCreated(pb::MessageCreated { message: Some(m) })
        | Payload::MessageUpdated(pb::MessageUpdated { message: Some(m) }) => {
            // A body: a message on the board that isn't a comment.
            if !m.thread_id.is_empty() {
                return;
            }
            let had = board.body(&m.id).cloned();
            let mut body = m.clone();
            if let Some(had) = had {
                if body.thread.is_none() {
                    body.thread = had.thread;
                }
                body.reactions = had.reactions;
            }
            if let Some(archived) = board.archived.as_mut().filter(|a| a.bodies.contains_key(&m.id)) {
                archived.bodies.insert(m.id.clone(), body.clone());
            }
            board.bodies.insert(m.id.clone(), body);
        }
        Payload::ThreadUpdated(p) => {
            if let Some(body) = board.bodies.get_mut(&p.thread_id) {
                body.thread = p.thread.clone();
            }
        }
        _ => {}
    }
}

fn forget(board: &mut Board, id: &str) {
    board.cards.retain(|c| c.id != id);
    board.bodies.remove(id);
    if let Some(archived) = &mut board.archived {
        archived.cards.retain(|c| c.id != id);
        archived.bodies.remove(id);
    }
}

// ───────────────────────── Calls ─────────────────────────

/// What a card's editor changes; anything left out stays.
#[derive(Debug, Clone, Default)]
pub struct Edit {
    pub title: Option<String>,
    /// Some(0) takes the due date off.
    pub due: Option<i64>,
    pub labels: Option<Vec<String>>,
    pub assignees: Option<Vec<String>>,
    pub archived: Option<bool>,
    /// The body's text and its whole list of files.
    pub body: Option<(String, Vec<pb::Attachment>)>,
}

impl Edit {
    fn change(&self, emojis: Vec<pb::Emoji>) -> pb::CardChange {
        pb::CardChange {
            title: self.title.clone(),
            set_due: self.due.is_some(),
            due_at: self.due.filter(|d| *d != 0).map(at_ms),
            set_labels: self.labels.is_some(),
            label_ids: self.labels.clone().unwrap_or_default(),
            set_assignees: self.assignees.is_some(),
            assignee_ids: self.assignees.clone().unwrap_or_default(),
            archived: self.archived,
            set_body: self.body.is_some(),
            content: self.body.as_ref().map(|b| b.0.clone()).unwrap_or_default(),
            emojis,
            attachments: self.body.as_ref().map(|b| b.1.clone()).unwrap_or_default(),
        }
    }

    /// The card as it shows until the server answers.
    fn shown(&self, card: &pb::Card, now: i64) -> pb::Card {
        let mut card = card.clone();
        if let Some(title) = &self.title {
            card.title = title.clone();
        }
        if let Some(labels) = &self.labels {
            card.label_ids = labels.clone();
        }
        if let Some(assignees) = &self.assignees {
            card.assignee_ids = assignees.clone();
        }
        if let Some(due) = self.due {
            card.due_at = (due != 0).then(|| at_ms(due));
        }
        if let Some(archived) = self.archived {
            card.archived_at = archived.then(|| at_ms(now));
        }
        card
    }
}

fn missing() -> Problem {
    Problem::new(tonic::Code::NotFound, "That instance isn't here.")
}

impl Core {
    /// Reads a board, each time it comes on screen.
    pub async fn load_board(&self, key: &str, server: &str, channel: &str) {
        let Some(api) = self.api(key) else { return };
        self.shared.instance(key, |i| {
            i.boards.entry(channel.to_owned()).or_default();
        });
        let res =
            rpc!(api.boards(), get_board(pb::GetBoardRequest { server_id: server.into(), channel_id: channel.into() }))
                .await;
        self.shared.instance(key, |i| match res {
            Ok(res) => {
                learn(&mut i.users, res.users);
                let board = i.boards.entry(channel.to_owned()).or_default();
                let mut cards = res.cards;
                cards.sort_by(by_rank);
                board.columns = res.columns;
                board.labels = res.labels;
                board.cards = cards;
                board.bodies = res.bodies.into_iter().map(|m| (m.id.clone(), m)).collect();
                board.loaded = true;
                board.error = None;
            }
            Err(err) => i.boards.entry(channel.to_owned()).or_default().error = Some(err.message),
        });
    }

    /// Reads the archived cards: the first page, or with `more` the next.
    pub async fn load_archived(&self, key: &str, server: &str, channel: &str, more: bool) {
        let Some(api) = self.api(key) else { return };
        let Some(after) = self.shared.instance(key, |i| {
            let board = i.boards.entry(channel.to_owned()).or_default();
            let list = board.archived.get_or_insert_with(Archived::default);
            if !more {
                *list = Archived::default();
            }
            list.loading = true;
            if more { list.cards.last().map(|c| c.id.clone()).unwrap_or_default() } else { String::new() }
        }) else {
            return;
        };
        let res = rpc!(
            api.boards(),
            list_archived_cards(pb::ListArchivedCardsRequest {
                server_id: server.into(),
                channel_id: channel.into(),
                after_id: after.clone(),
                limit: PAGE,
            })
        )
        .await;
        self.shared.instance(key, |i| {
            let Some(list) = i.boards.get_mut(channel).and_then(|b| b.archived.as_mut()) else { return };
            list.loading = false;
            let Ok(res) = res else { return };
            // Closed and opened again while it was out: that read is the one to keep.
            if list.cards.last().map(|c| c.id.as_str()).unwrap_or_default() != after {
                return;
            }
            list.cards.extend(res.cards.into_iter().filter(|c| c.archived_at.is_some()));
            list.bodies.extend(res.bodies.into_iter().map(|m| (m.id.clone(), m)));
            list.more = res.has_more;
            learn(&mut i.users, res.users);
        });
    }

    /// Adds a card by its title at the bottom of a column.
    pub async fn create_card(
        &self,
        key: &str,
        server: &str,
        channel: &str,
        column: &str,
        title: &str,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        reports::used("board.card_create");
        let res = rpc!(
            api.boards(),
            create_card(pb::CreateCardRequest {
                server_id: server.into(),
                channel_id: channel.into(),
                column_id: column.into(),
                title: title.into(),
                ..Default::default()
            })
        )
        .await?;
        self.shared.instance(key, |i| {
            if let (Some(board), Some(card)) = (i.boards.get_mut(channel), res.card) {
                if let Some(body) = res.body {
                    board.bodies.insert(card.id.clone(), body);
                }
                put(board, card);
            }
        });
        Ok(())
    }

    /// Moves a card above `before` in a column (at its bottom when empty); it shows there at once.
    pub async fn move_card(
        &self,
        key: &str,
        server: &str,
        channel: &str,
        card: &str,
        column: &str,
        before: &str,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        reports::used("board.card_move");
        let had = self.shared.instance(key, |i| {
            let board = i.boards.get_mut(channel)?;
            let had = board.cards.clone();
            moved(board, card, column, before);
            Some(had)
        });
        let res = rpc!(
            api.boards(),
            move_card(pb::MoveCardRequest {
                server_id: server.into(),
                channel_id: channel.into(),
                card_id: card.into(),
                column_id: column.into(),
                before_id: before.into(),
            })
        )
        .await;
        self.shared.instance(key, |i| {
            let Some(board) = i.boards.get_mut(channel) else { return };
            match &res {
                Ok(pb::MoveCardResponse { card: Some(card) }) => put(board, card.clone()),
                Ok(_) => {}
                Err(_) => {
                    if let Some(Some(had)) = &had {
                        board.cards = had.clone();
                    }
                }
            }
        });
        res.map(|_| ())
    }

    /// Changes a card; what it changes shows at once.
    pub async fn update_card(
        &self,
        key: &str,
        server: &str,
        channel: &str,
        card: &str,
        edit: Edit,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        if let Some(archived) = edit.archived {
            reports::used(if archived { "board.card_archive" } else { "board.card_restore" });
        }
        let now = crate::core::dms::now_ms();
        let (had, emojis) = self
            .shared
            .instance(key, |i| {
                let board = i.boards.get_mut(channel)?;
                let before = board.card(card)?.clone();
                let emojis = board.body(card).map(|b| b.emojis.clone()).unwrap_or_default();
                let shown = edit.shown(&before, now);
                if board.cards.iter().any(|c| c.id == card) || shown.archived_at.is_none() {
                    put(board, shown.clone());
                }
                if let Some(archived) = &mut board.archived {
                    archived.cards.retain(|c| c.id != card);
                    if shown.archived_at.is_some() {
                        archived.cards.insert(0, shown);
                    }
                }
                Some((before, emojis))
            })
            .flatten()
            .ok_or_else(|| Problem::new(tonic::Code::NotFound, "That card isn't here anymore."))?;
        let res = rpc!(
            api.boards(),
            update_card(pb::UpdateCardRequest {
                server_id: server.into(),
                channel_id: channel.into(),
                card_id: card.into(),
                change: Some(edit.change(emojis)),
            })
        )
        .await;
        self.shared.instance(key, |i| {
            let Some(board) = i.boards.get_mut(channel) else { return };
            let card = match &res {
                Ok(res) => {
                    if let (Some(card), Some(body)) = (&res.card, &res.body) {
                        board.bodies.insert(card.id.clone(), body.clone());
                        if let Some(archived) = &mut board.archived {
                            archived.bodies.insert(card.id.clone(), body.clone());
                        }
                    }
                    res.card.clone()
                }
                Err(_) => Some(had.clone()),
            };
            if let Some(card) = card {
                apply(board, &Payload::CardUpdated(pb::CardUpdated { card: Some(card) }));
            }
        });
        res.map(|_| ())
    }
}

impl Core {
    /// Sets a board's columns and labels whole (Manage Channels): a column left
    /// out must hold no cards, a label left out comes off every card.
    pub async fn set_board(
        &self,
        key: &str,
        server: &str,
        channel: &str,
        columns: Vec<pb::BoardColumn>,
        labels: Vec<pb::BoardLabel>,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.boards(),
            set_board(pb::SetBoardRequest { server_id: server.into(), channel_id: channel.into(), columns, labels })
        )
        .await?;
        self.shared.instance(key, |i| {
            if let Some(board) = i.boards.get_mut(channel) {
                board.columns = res.columns;
                board.labels = res.labels;
            }
        });
        Ok(())
    }
}

/// People a board names, so names and pictures show.
fn learn(users: &mut HashMap<String, pb::User>, people: Vec<pb::User>) {
    for user in people {
        users.entry(user.id.clone()).or_insert(user);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, column: &str, rank: &str) -> pb::Card {
        pb::Card {
            id: id.into(),
            channel_id: "b".into(),
            column_id: column.into(),
            rank: rank.into(),
            ..Default::default()
        }
    }

    fn board() -> Board {
        let column = |id: &str, done| pb::BoardColumn { id: id.into(), name: id.into(), done, ..Default::default() };
        let mut b =
            Board { columns: vec![column("todo", false), column("done", true)], loaded: true, ..Default::default() };
        for c in [card("1", "todo", "V"), card("2", "todo", "k"), card("3", "todo", "t")] {
            put(&mut b, c);
        }
        b
    }

    fn order(b: &Board, column: &str) -> Vec<String> {
        b.cards_in(column).map(|c| c.id.clone()).collect()
    }

    #[test]
    fn ranks_fall_between() {
        let mut ranks = vec![between("", "")];
        for _ in 0..100 {
            let last = ranks.last().unwrap().clone();
            ranks.push(between(&last, ""));
            let first = ranks[0].clone();
            ranks.insert(0, between("", &first));
        }
        for _ in 0..100 {
            let mid = ranks.len() / 2;
            let r = between(&ranks[mid - 1], &ranks[mid]);
            ranks.insert(mid, r);
        }
        assert!(ranks.windows(2).all(|p| p[0] < p[1]), "{ranks:?}");
        assert!(ranks.iter().all(|r| !r.is_empty() && !r.ends_with('0')));
        // The same answers the server and the web give.
        assert_eq!(between("", ""), "V");
        assert_eq!(between("V", ""), "k");
        assert_eq!(between("", "V"), "F");
    }

    #[test]
    fn moving_puts_a_card_where_it_was_dropped() {
        let mut b = board();
        assert_eq!(order(&b, "todo"), ["1", "2", "3"]);
        moved(&mut b, "3", "todo", "1");
        assert_eq!(order(&b, "todo"), ["3", "1", "2"]);
        moved(&mut b, "3", "done", "");
        assert_eq!(order(&b, "todo"), ["1", "2"]);
        assert_eq!(order(&b, "done"), ["3"]);
        moved(&mut b, "1", "todo", "");
        assert_eq!(order(&b, "todo"), ["2", "1"]);
    }

    #[test]
    fn keys_step_cards_along() {
        let b = board();
        assert_eq!(step(&b, "1", Way::Up), None);
        assert_eq!(step(&b, "1", Way::Down), Some(("todo".into(), "3".into())));
        assert_eq!(step(&b, "3", Way::Down), None);
        assert_eq!(step(&b, "2", Way::Down), Some(("todo".into(), String::new())));
        assert_eq!(step(&b, "2", Way::Right), Some(("done".into(), String::new())));
        assert_eq!(step(&b, "2", Way::Left), None);
    }

    #[test]
    fn events_keep_a_board_in_step() {
        let mut b = board();
        let body =
            pb::Message { id: "1".into(), channel_id: "b".into(), content: "hello".into(), ..Default::default() };
        apply(&mut b, &Payload::MessageCreated(pb::MessageCreated { message: Some(body) }));
        // A comment isn't a body.
        let comment = pb::Message { id: "9".into(), thread_id: "1".into(), ..Default::default() };
        apply(&mut b, &Payload::MessageCreated(pb::MessageCreated { message: Some(comment) }));
        assert_eq!(b.bodies.len(), 1);
        let thread = pb::ThreadSummary { reply_count: 2, ..Default::default() };
        let updated = pb::ThreadUpdated { channel_id: "b".into(), thread_id: "1".into(), thread: Some(thread) };
        apply(&mut b, &Payload::ThreadUpdated(updated));
        assert_eq!(b.bodies["1"].thread.as_ref().map(|t| t.reply_count), Some(2));
        let mut archived = card("2", "todo", "k");
        archived.archived_at = Some(at_ms(5));
        apply(&mut b, &Payload::CardUpdated(pb::CardUpdated { card: Some(archived) }));
        assert_eq!(order(&b, "todo"), ["1", "3"]);
        apply(&mut b, &Payload::MessageDeleted(pb::MessageDeleted { channel_id: "b".into(), message_id: "1".into() }));
        assert_eq!(order(&b, "todo"), ["3"]);
        assert!(b.bodies.is_empty());
        let label = pb::BoardLabel { id: "l1".into(), ..Default::default() };
        b.cards[0].label_ids = vec!["l1".into(), "gone".into()];
        let columns = b.columns.clone();
        apply(
            &mut b,
            &Payload::BoardUpdated(pb::BoardUpdated { channel_id: "b".into(), columns, labels: vec![label] }),
        );
        assert_eq!(b.cards[0].label_ids, ["l1"]);
    }

    #[test]
    fn filters_and_due_dates() {
        let mut c = card("1", "todo", "V");
        c.title = "Fix the Lights".into();
        c.assignee_ids = vec!["me".into()];
        let body = pb::Message { content: "in the hall".into(), ..Default::default() };
        let words = |q: &str| Filter { query: q.into(), ..Default::default() };
        assert!(shows(&c, Some(&body), &words("lights HALL")));
        assert!(!shows(&c, Some(&body), &words("kitchen")));
        assert!(shows(&c, None, &Filter { assignee: Some("me".into()), ..Default::default() }));
        assert!(!shows(&c, None, &Filter { label: Some("l".into()), ..Default::default() }));
        let done = pb::BoardColumn { done: true, ..Default::default() };
        assert_eq!(due(&c, None, 0), None);
        c.due_at = Some(at_ms(1_000));
        assert_eq!(due(&c, None, 2_000), Some(Due::Late));
        assert_eq!(due(&c, Some(&done), 2_000), Some(Due::Later));
        assert_eq!(due(&c, None, 0), Some(Due::Soon));
    }
}
