//! Boards (`BoardService`, docs/boards.md): channels whose columns
//! (`board_columns`) hold cards (`board_cards`) carrying labels
//! (`board_labels`) and people (`board_assignees`). A card's body is the
//! message with the card's id, sent through [`Api::send_with`] and changed
//! here, so Markdown, files, mentions, AutoMod and reactions work as on any
//! message; its comments are the thread under that message. Deleting the
//! body deletes the card ([`forget_card`], from `threads::after_delete`).
//!
//! A column's cards sort by `rank`, then id: a fractional key ([`between`]),
//! so a move writes only the card moved.

use std::collections::HashSet;

use tonic::{Request, Response, Status};

use super::messages::{self, check_content, check_extras, check_files, check_not_timed_out};
use super::{Api, Seat, automod, reactions, respond, text, threads, users};
use crate::attachments;
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{millis, new_id, now_ms, timestamp};
use crate::media;
use crate::pb::{self, Permission, board_service_server::BoardService};
use crate::permissions::Access;
use crate::servers::{self as store, Audit, Payload, UsageChange, load_channel};

const MAX_TITLE: usize = 200;
const MAX_COLUMNS: usize = 20;
const MAX_COLUMN_NAME: usize = 50;
const MAX_LABELS: usize = 50;
const MAX_LABEL_NAME: usize = 30;
const MAX_ASSIGNEES: usize = 10;
const MAX_CARD_LIMIT: i32 = 10_000;
const PAGE: i32 = 50;
const MAX_PAGE: i32 = 100;

/// The columns a new board starts with: name and done.
const START: [(&str, bool); 3] = [("To do", false), ("Doing", false), ("Done", true)];

/// The digits of a rank, in the order they sort.
const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn digit(c: u8) -> usize {
    DIGITS.iter().position(|d| *d == c).unwrap_or(0)
}

/// A rank between `lo` and `hi` (exclusive), where an empty `lo` is before
/// everything and an empty `hi` after everything. Ranks never end in `0`,
/// so they sort as the fractions they stand for. `lo` must sort before `hi`.
pub(super) fn between(lo: &str, hi: &str) -> String {
    let lo: Vec<usize> = lo.bytes().map(digit).collect();
    let hi: Option<Vec<usize>> = (!hi.is_empty()).then(|| hi.bytes().map(digit).collect());
    midpoint(&lo, hi.as_deref()).into_iter().map(|d| DIGITS[d] as char).collect()
}

fn midpoint(lo: &[usize], hi: Option<&[usize]>) -> Vec<usize> {
    if let Some(hi) = hi {
        // The digits they share go first.
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
    // Next to each other: `hi`'s first digit alone when it has more, else
    // `lo`'s and something after the rest of it.
    if let Some(hi) = hi
        && hi.len() > 1
    {
        return vec![hi[0]];
    }
    let mut out = vec![a];
    out.extend(midpoint(lo.get(1..).unwrap_or(&[]), None));
    out
}

/// `count` ranks spread evenly, for a column whose ranks clashed.
fn spread(count: usize) -> Vec<String> {
    let mut width = 1;
    let mut room = DIGITS.len() as u128;
    while room <= (count as u128 + 1) * 2 {
        width += 1;
        room *= DIGITS.len() as u128;
    }
    let step = room / (count as u128 + 1);
    (1..=count as u128)
        .map(|i| {
            let mut n = step * i;
            let mut out = vec![b'0'; width];
            for slot in out.iter_mut().rev() {
                *slot = DIGITS[(n % DIGITS.len() as u128) as usize];
                n /= DIGITS.len() as u128;
            }
            while out.last() == Some(&b'0') {
                out.pop();
            }
            String::from_utf8(out).unwrap_or_default()
        })
        .collect()
}

/// A card being created, as [`Api::send_with`] stores it with its body.
pub(super) struct NewCard {
    pub column_id: String,
    pub title: String,
    pub label_ids: Vec<String>,
    pub assignee_ids: Vec<String>,
    pub due_at: i64,
    pub before_id: String,
}

const CARD_COLUMNS: &str = "id, channel_id, column_id, rank, title, label_ids, assignee_ids, due_at, author_id, created_at, updated_at, archived_at";

fn ids(list: &str) -> Vec<String> {
    list.split(' ').filter(|id| !id.is_empty()).map(str::to_string).collect()
}

fn when(ms: i64) -> Option<prost_types::Timestamp> {
    (ms > 0).then(|| timestamp(ms))
}

fn card_row(r: &turso::Row) -> turso::Result<pb::Card> {
    Ok(pb::Card {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        column_id: r.get(2)?,
        rank: r.get(3)?,
        title: r.get(4)?,
        label_ids: ids(&r.get::<String>(5)?),
        assignee_ids: ids(&r.get::<String>(6)?),
        due_at: when(r.get(7)?),
        author_id: r.get(8)?,
        created_at: Some(timestamp(r.get(9)?)),
        updated_at: Some(timestamp(r.get(10)?)),
        archived_at: when(r.get(11)?),
    })
}

async fn load_card(conn: &turso::Connection, channel_id: &str, card_id: &str) -> Result<Option<pb::Card>> {
    query_one(
        conn,
        &format!("SELECT {CARD_COLUMNS} FROM board_cards WHERE id = ?1 AND channel_id = ?2"),
        (card_id, channel_id),
        card_row,
    )
    .await
}

async fn columns(conn: &turso::Connection, channel_id: &str) -> Result<Vec<pb::BoardColumn>> {
    query_all(
        conn,
        "SELECT id, name, color, done, card_limit FROM board_columns WHERE channel_id = ?1 ORDER BY position, id",
        [channel_id],
        |r| {
            Ok(pb::BoardColumn {
                id: r.get(0)?,
                name: r.get(1)?,
                color: r.get::<i64>(2)? as i32,
                done: r.get(3)?,
                card_limit: r.get::<i64>(4)? as i32,
            })
        },
    )
    .await
}

async fn labels(conn: &turso::Connection, channel_id: &str) -> Result<Vec<pb::BoardLabel>> {
    query_all(
        conn,
        "SELECT id, name, color FROM board_labels WHERE channel_id = ?1 ORDER BY position, id",
        [channel_id],
        |r| Ok(pb::BoardLabel { id: r.get(0)?, name: r.get(1)?, color: r.get::<i64>(2)? as i32 }),
    )
    .await
}

/// Writes a card's row and its people again from `card`.
async fn save(conn: &turso::Connection, card: &pb::Card) -> Result<()> {
    let at = |t: &Option<prost_types::Timestamp>| t.as_ref().map_or(0, millis);
    conn.execute(
        "INSERT INTO board_cards (id, channel_id, column_id, rank, title, label_ids, assignee_ids, due_at, author_id,
           created_at, updated_at, archived_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (id) DO UPDATE SET column_id = excluded.column_id, rank = excluded.rank, title = excluded.title,
           label_ids = excluded.label_ids, assignee_ids = excluded.assignee_ids, due_at = excluded.due_at,
           updated_at = excluded.updated_at, archived_at = excluded.archived_at",
        (
            card.id.as_str(),
            card.channel_id.as_str(),
            card.column_id.as_str(),
            card.rank.as_str(),
            card.title.as_str(),
            card.label_ids.join(" "),
            card.assignee_ids.join(" "),
            at(&card.due_at),
            card.author_id.as_str(),
            at(&card.created_at),
            at(&card.updated_at),
            at(&card.archived_at),
        ),
    )
    .await?;
    conn.execute("DELETE FROM board_assignees WHERE card_id = ?1", [card.id.as_str()]).await?;
    for account_id in &card.assignee_ids {
        conn.execute(
            "INSERT INTO board_assignees (card_id, account_id) VALUES (?1, ?2)",
            (card.id.as_str(), account_id.as_str()),
        )
        .await?;
    }
    Ok(())
}

/// The rank for a card going just above `before_id` in a column (at the
/// bottom when it's empty), leaving out the card being moved. Spreads the
/// column's ranks out first if two cards share one there.
async fn rank_for(conn: &turso::Connection, column_id: &str, before_id: &str, moving: &str) -> Result<String> {
    let cards = query_all(
        conn,
        "SELECT id, rank FROM board_cards WHERE column_id = ?1 AND archived_at = 0 AND id <> ?2 ORDER BY rank, id",
        (column_id, moving),
        |r| Ok((r.get::<String>(0)?, r.get::<String>(1)?)),
    )
    .await?;
    let at = if before_id.is_empty() {
        cards.len()
    } else {
        cards.iter().position(|(id, _)| id == before_id).ok_or(Error::NotFound("card to place it above"))?
    };
    let lo = at.checked_sub(1).map_or("", |i| cards[i].1.as_str());
    let hi = cards.get(at).map_or("", |(_, rank)| rank.as_str());
    if hi.is_empty() || lo < hi {
        return Ok(between(lo, hi));
    }
    // Two cards moved to one place at once: give the column fresh ranks.
    let ranks = spread(cards.len());
    for ((id, _), rank) in cards.iter().zip(&ranks) {
        conn.execute("UPDATE board_cards SET rank = ?2 WHERE id = ?1", (id.as_str(), rank.as_str())).await?;
    }
    let lo = at.checked_sub(1).map_or("", |i| ranks[i].as_str());
    Ok(between(lo, ranks.get(at).map_or("", String::as_str)))
}

/// The column `column_id` names on a board, or its first one when empty.
async fn column_in(conn: &turso::Connection, channel_id: &str, column_id: &str) -> Result<String> {
    let found = if column_id.is_empty() {
        query_one(
            conn,
            "SELECT id FROM board_columns WHERE channel_id = ?1 ORDER BY position, id LIMIT 1",
            [channel_id],
            |r| r.get::<String>(0),
        )
        .await?
    } else {
        query_one(
            conn,
            "SELECT id FROM board_columns WHERE id = ?1 AND channel_id = ?2",
            (column_id, channel_id),
            |r| r.get::<String>(0),
        )
        .await?
    };
    found.ok_or(Error::NotFound("column"))
}

/// Checks a card's labels are the board's, inside a write.
async fn check_labels(conn: &turso::Connection, channel_id: &str, label_ids: &[String]) -> Result<()> {
    let known: HashSet<String> = labels(conn, channel_id).await?.into_iter().map(|l| l.id).collect();
    if label_ids.iter().any(|id| !known.contains(id)) {
        return Err(Error::NotFound("label"));
    }
    Ok(())
}

fn unique(list: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    list.iter().filter(|id| !id.is_empty() && seen.insert(id.as_str())).cloned().collect()
}

fn check_title(title: &str) -> Result<String> {
    text("title", title, 1, MAX_TITLE)
}

/// Stores a new card, inside the write that sends its body.
pub(super) async fn insert(
    conn: &turso::Connection,
    body: &pb::Message,
    new: &NewCard,
    now: i64,
    events: &mut Vec<Payload>,
) -> Result<pb::Card> {
    let column_id = column_in(conn, &body.channel_id, &new.column_id).await?;
    check_labels(conn, &body.channel_id, &new.label_ids).await?;
    let card = pb::Card {
        id: body.id.clone(),
        channel_id: body.channel_id.clone(),
        rank: rank_for(conn, &column_id, &new.before_id, &body.id).await?,
        column_id,
        title: new.title.clone(),
        label_ids: new.label_ids.clone(),
        assignee_ids: new.assignee_ids.clone(),
        due_at: when(new.due_at),
        author_id: body.author_id.clone(),
        created_at: Some(timestamp(now)),
        updated_at: Some(timestamp(now)),
        archived_at: None,
    };
    save(conn, &card).await?;
    // Its author and the people on it follow its comments.
    threads::follow_quietly(conn, &card.id, &card.author_id).await?;
    for account_id in &card.assignee_ids {
        threads::follow_quietly(conn, &card.id, account_id).await?;
    }
    events.push(Payload::CardUpdated(pb::CardUpdated { card: Some(card.clone()) }));
    Ok(card)
}

/// A new board's columns, inside the write that makes the channel.
pub(super) async fn start(conn: &turso::Connection, channel_id: &str, events: &mut Vec<Payload>) -> Result<()> {
    for (position, (name, done)) in START.iter().enumerate() {
        conn.execute(
            "INSERT INTO board_columns (id, channel_id, name, position, done) VALUES (?1, ?2, ?3, ?4, ?5)",
            (new_id(), channel_id, *name, position as i64, *done),
        )
        .await?;
    }
    events.push(Payload::BoardUpdated(pb::BoardUpdated {
        channel_id: channel_id.to_string(),
        columns: columns(conn, channel_id).await?,
        labels: vec![],
    }));
    Ok(())
}

/// Takes away the card whose body was just deleted, if it was one, inside
/// that write.
pub(super) async fn forget_card(
    conn: &turso::Connection,
    channel_id: &str,
    message_id: &str,
    events: &mut Vec<Payload>,
) -> Result<()> {
    if load_card(conn, channel_id, message_id).await?.is_none() {
        return Ok(());
    }
    conn.execute("DELETE FROM board_cards WHERE id = ?1", [message_id]).await?;
    conn.execute("DELETE FROM board_assignees WHERE card_id = ?1", [message_id]).await?;
    events.push(Payload::CardDeleted(pb::CardDeleted {
        channel_id: channel_id.to_string(),
        card_id: message_id.to_string(),
    }));
    Ok(())
}

/// Takes away a board with its channel, inside that write (its bodies and
/// comments go with the channel's messages).
pub(super) async fn forget_channel(conn: &turso::Connection, channel_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM board_assignees WHERE card_id IN (SELECT id FROM board_cards WHERE channel_id = ?1)",
        [channel_id],
    )
    .await?;
    conn.execute("DELETE FROM board_cards WHERE channel_id = ?1", [channel_id]).await?;
    conn.execute("DELETE FROM board_columns WHERE channel_id = ?1", [channel_id]).await?;
    conn.execute("DELETE FROM board_labels WHERE channel_id = ?1", [channel_id]).await?;
    Ok(())
}

/// Takes a deleted account off every card it was on, inside the write that
/// forgets it in this server.
pub(crate) async fn forget_assignee(
    conn: &turso::Connection,
    account_id: &str,
    events: &mut Vec<Payload>,
) -> Result<()> {
    let cards = query_all(
        conn,
        &format!(
            "SELECT {CARD_COLUMNS} FROM board_cards WHERE id IN (SELECT card_id FROM board_assignees WHERE account_id = ?1)"
        ),
        [account_id],
        card_row,
    )
    .await?;
    for mut card in cards {
        card.assignee_ids.retain(|id| id != account_id);
        save(conn, &card).await?;
        events.push(Payload::CardUpdated(pb::CardUpdated { card: Some(card) }));
    }
    Ok(())
}

/// The board a request names, in a channel the member can see.
async fn board(conn: &turso::Connection, server_id: &str, access: &Access, channel_id: &str) -> Result<pb::Channel> {
    access.require_in(channel_id, Permission::ViewChannels)?;
    let channel = load_channel(conn, server_id, channel_id).await?.ok_or(Error::NotFound("channel"))?;
    if channel.r#type != pb::ChannelType::Board as i32 {
        return Err(Error::invalid("that channel isn't a board"));
    }
    Ok(channel)
}

/// Checks that each person can see the board, before the write.
async fn check_assignees(sdb: &store::ServerDb, channel_id: &str, assignee_ids: &[String]) -> Result<()> {
    if assignee_ids.len() > MAX_ASSIGNEES {
        return Err(Error::invalid(format!("a card can have at most {MAX_ASSIGNEES} people")));
    }
    for id in assignee_ids {
        match sdb.member_access(id).await? {
            Some((_, access)) if access.can_see(channel_id) => {}
            _ => return Err(Error::invalid("only people who can see the board can be put on a card")),
        }
    }
    Ok(())
}

/// The bodies of `cards` as `viewer` reads them, and the people they name.
async fn bodies(
    conn: &turso::Connection,
    server_id: &str,
    viewer: &str,
    cards: &[pb::Card],
) -> Result<(Vec<pb::Message>, Vec<pb::User>)> {
    let ids: Vec<&str> = cards.iter().map(|c| c.id.as_str()).collect();
    let mut bodies = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(100) {
        bodies.extend(messages::by_ids(conn, server_id, chunk).await?);
    }
    reactions::attach(conn, viewer, &mut bodies).await?;
    threads::attach(conn, &mut bodies).await?;
    let mut people: Vec<&str> = bodies.iter().map(|m| m.author_id.as_str()).collect();
    people.extend(cards.iter().flat_map(|c| c.assignee_ids.iter().map(String::as_str)));
    let people = users(conn, &people).await?;
    Ok((bodies, people))
}

/// Checks a column or label color.
fn check_color(color: i32) -> Result<i32> {
    if !(0..=0xFF_FFFF).contains(&color) {
        return Err(Error::invalid("colors are 0xRRGGBB"));
    }
    Ok(color)
}

#[tonic::async_trait]
impl BoardService for Api {
    async fn get_board(&self, request: Request<pb::GetBoardRequest>) -> Result<Response<pb::GetBoardResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                let conn = sdb.read()?;
                board(&conn, &sdb.id, &access, &req.channel_id).await?;
                let cards = query_all(
                    &conn,
                    &format!(
                        "SELECT {CARD_COLUMNS} FROM board_cards WHERE channel_id = ?1 AND archived_at = 0 ORDER BY rank, id"
                    ),
                    [req.channel_id.as_str()],
                    card_row,
                )
                .await?;
                let (bodies, users) = bodies(&conn, &sdb.id, &account.id, &cards).await?;
                Ok(pb::GetBoardResponse {
                    columns: columns(&conn, &req.channel_id).await?,
                    labels: labels(&conn, &req.channel_id).await?,
                    cards,
                    bodies,
                    users,
                })
            }
            .await,
        )
    }

    async fn list_archived_cards(
        &self,
        request: Request<pb::ListArchivedCardsRequest>,
    ) -> Result<Response<pb::ListArchivedCardsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                let conn = sdb.read()?;
                board(&conn, &sdb.id, &access, &req.channel_id).await?;
                let limit = if req.limit <= 0 { PAGE } else { req.limit.min(MAX_PAGE) } as i64;
                let after = if req.after_id.is_empty() {
                    None
                } else {
                    Some(
                        load_card(&conn, &req.channel_id, &req.after_id)
                            .await?
                            .and_then(|c| c.archived_at)
                            .ok_or(Error::NotFound("card"))?,
                    )
                };
                let (at, id) = after.map_or((i64::MAX, String::new()), |t| (millis(&t), req.after_id.clone()));
                let mut cards = query_all(
                    &conn,
                    &format!(
                        "SELECT {CARD_COLUMNS} FROM board_cards WHERE channel_id = ?1 AND archived_at > 0
                           AND (archived_at < ?2 OR (archived_at = ?2 AND id < ?3))
                         ORDER BY archived_at DESC, id DESC LIMIT ?4"
                    ),
                    (req.channel_id.as_str(), at, if id.is_empty() { "~".to_string() } else { id }, limit + 1),
                    card_row,
                )
                .await?;
                let has_more = cards.len() as i64 > limit;
                cards.truncate(limit as usize);
                let (bodies, users) = bodies(&conn, &sdb.id, &account.id, &cards).await?;
                Ok(pb::ListArchivedCardsResponse { cards, bodies, users, has_more })
            }
            .await,
        )
    }

    async fn create_card(
        &self,
        request: Request<pb::CreateCardRequest>,
    ) -> Result<Response<pb::CreateCardResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.app.servers.get(&req.server_id).await?;
                let label_ids = unique(&req.label_ids);
                if label_ids.len() > MAX_LABELS {
                    return Err(Error::invalid(format!("a card can have at most {MAX_LABELS} labels")));
                }
                let assignee_ids = unique(&req.assignee_ids);
                check_assignees(&sdb, &req.channel_id, &assignee_ids).await?;
                let card = NewCard {
                    column_id: req.column_id,
                    title: check_title(&req.title)?,
                    label_ids,
                    assignee_ids,
                    due_at: req.due_at.as_ref().map_or(0, millis).max(0),
                    before_id: req.before_id,
                };
                let send = pb::SendMessageRequest {
                    server_id: req.server_id,
                    channel_id: req.channel_id,
                    content: req.content,
                    attachments: req.attachments,
                    emojis: req.emojis,
                    ..Default::default()
                };
                let (body, card) = self.send_with(account, send, Some(card)).await?;
                Ok(pb::CreateCardResponse { card, body: Some(body) })
            }
            .await,
        )
    }

    async fn update_card(
        &self,
        request: Request<pb::UpdateCardRequest>,
    ) -> Result<Response<pb::UpdateCardResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, member, access } = self.membership(&account, &req.server_id).await?;
                check_not_timed_out(&member)?;
                access.require_in(&req.channel_id, Permission::ManageCards)?;
                let mut change = req.change.unwrap_or_default();
                let title = change.title.as_deref().map(check_title).transpose()?;
                let label_ids = unique(&change.label_ids);
                if label_ids.len() > MAX_LABELS {
                    return Err(Error::invalid(format!("a card can have at most {MAX_LABELS} labels")));
                }
                let assignee_ids = unique(&change.assignee_ids);
                if change.set_assignees {
                    check_assignees(&sdb, &req.channel_id, &assignee_ids).await?;
                }
                // The body: its files as they are now, and the new ones,
                // checked as for a message.
                let before = messages::load_message(&*sdb.read()?, &sdb.id, &req.card_id)
                    .await?
                    .filter(|m| m.channel_id == req.channel_id)
                    .ok_or(Error::NotFound("card"))?;
                let mut added = Vec::new();
                let mut emojis = Vec::new();
                if change.set_body {
                    check_content(&change.content, true)?;
                    check_extras(&mut change.attachments, &[])?;
                    let had: HashSet<&str> = before.attachments.iter().map(|a| a.id.as_str()).collect();
                    for file in &change.attachments {
                        let id = media::id_in_url(file.url.trim()).unwrap_or_default();
                        if !had.contains(id.as_str()) {
                            added.push(file.clone());
                        }
                    }
                    if !added.is_empty() {
                        access.require_in(&req.channel_id, Permission::AttachFiles)?;
                    }
                    check_files(&self.app, &account.id, &sdb.id, &sdb.id, &mut added).await?;
                    emojis = messages::outside_emojis(
                        &self.app,
                        &account.id,
                        &sdb.id,
                        &change.content,
                        std::mem::take(&mut change.emojis),
                    )
                    .await;
                }
                let kept: Vec<String> = change
                    .attachments
                    .iter()
                    .filter_map(|a| media::id_in_url(a.url.trim()))
                    .filter(|id| before.attachments.iter().any(|had| &had.id == id))
                    .collect();
                let (card, body, dropped) = sdb
                    .write(&account.id, async |conn, events| {
                        let channel = board(conn, &sdb.id, &access, &req.channel_id).await?;
                        let mut card =
                            load_card(conn, &channel.id, &req.card_id).await?.ok_or(Error::NotFound("card"))?;
                        let mut body =
                            messages::load_message(conn, &sdb.id, &card.id).await?.ok_or(Error::NotFound("card"))?;
                        let now = now_ms();
                        let title_changed = title.as_ref().is_some_and(|t| *t != card.title);
                        let body_changed = change.set_body
                            && (body.content != change.content
                                || !added.is_empty()
                                || body.attachments.iter().any(|a| !kept.contains(&a.id)));
                        if title_changed || (body_changed && body.content != change.content) {
                            let new_title = title.clone().unwrap_or_else(|| card.title.clone());
                            let content = if change.set_body { change.content.as_str() } else { body.content.as_str() };
                            let read = format!("{new_title}\n{content}");
                            let verdict = automod::review(
                                conn,
                                &sdb.id,
                                &member,
                                &access,
                                &channel,
                                automod::Text { all: &read, content },
                                None,
                                automod::Sent { id: &card.id, thread_id: "", edit: true },
                                events,
                            )
                            .await?;
                            if let Some(why) = verdict.blocked {
                                return Ok(Err(why));
                            }
                        }
                        if let Some(title) = &title {
                            card.title = title.clone();
                        }
                        if change.set_due {
                            card.due_at = change.due_at.filter(|t| millis(t) > 0);
                        }
                        if change.set_labels {
                            check_labels(conn, &channel.id, &label_ids).await?;
                            card.label_ids = label_ids.clone();
                        }
                        if change.set_assignees {
                            for account_id in assignee_ids.iter().filter(|id| !card.assignee_ids.contains(id)) {
                                threads::follow_quietly(conn, &card.id, account_id).await?;
                            }
                            card.assignee_ids = assignee_ids.clone();
                        }
                        match change.archived {
                            Some(true) if card.archived_at.is_none() => card.archived_at = Some(timestamp(now)),
                            Some(false) if card.archived_at.is_some() => {
                                card.archived_at = None;
                                card.rank = rank_for(conn, &card.column_id, "", &card.id).await?;
                            }
                            _ => {}
                        }
                        card.updated_at = Some(timestamp(now));
                        save(conn, &card).await?;
                        events.push(Payload::CardUpdated(pb::CardUpdated { card: Some(card.clone()) }));
                        let mut dropped = Vec::new();
                        if body_changed {
                            let old_size = messages::stored_size(&body);
                            let old_files = body.attachments.len() as i64;
                            for file in body.attachments.iter().filter(|a| !kept.contains(&a.id)) {
                                dropped.extend(attachments::forget_file(conn, &file.id).await?);
                            }
                            body.attachments.retain(|a| kept.contains(&a.id));
                            let fresh = pb::Message { attachments: added.clone(), ..body.clone() };
                            attachments::add(conn, &fresh, now).await?;
                            body.attachments.extend(added.clone());
                            (body.mentions_everyone, body.mention_role_ids) =
                                messages::mentions(conn, &sdb.id, &access, &body.channel_id, &change.content).await?;
                            body.mention_user_ids = messages::mentioned_users(conn, &change.content).await?;
                            body.emojis = messages::kept_emojis(
                                std::mem::take(&mut body.emojis),
                                emojis.clone(),
                                &change.content,
                            );
                            body.content = change.content.clone();
                            body.edited_at = Some(timestamp(now));
                            messages::save_edit(conn, &body, old_size).await?;
                            let files = body.attachments.len() as i64;
                            conn.execute(
                                "UPDATE messages SET attachment_count = ?2 WHERE id = ?1",
                                (body.id.as_str(), files),
                            )
                            .await?;
                            store::add_usage(
                                conn,
                                UsageChange { attachments: files - old_files, ..Default::default() },
                            )
                            .await?;
                            reactions::attach(conn, &account.id, std::slice::from_mut(&mut body)).await?;
                            threads::attach(conn, std::slice::from_mut(&mut body)).await?;
                            events.push(Payload::MessageUpdated(pb::MessageUpdated { message: Some(body.clone()) }));
                        }
                        Ok(Ok((card, body, dropped)))
                    })
                    .await?
                    .map_err(Error::denied)?;
                for file in &added {
                    self.app.keep_picture(Some(&file.id), Some(&sdb.id)).await;
                }
                attachments::drop_soon(&self.app, &sdb.id, dropped);
                Ok(pb::UpdateCardResponse { card: Some(card), body: Some(body) })
            }
            .await,
        )
    }

    async fn move_card(&self, request: Request<pb::MoveCardRequest>) -> Result<Response<pb::MoveCardResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, member, access } = self.membership(&account, &req.server_id).await?;
                check_not_timed_out(&member)?;
                access.require_in(&req.channel_id, Permission::ManageCards)?;
                if req.before_id == req.card_id {
                    return Err(Error::invalid("a card can't go above itself"));
                }
                let card = sdb
                    .write(&account.id, async |conn, events| {
                        let channel = board(conn, &sdb.id, &access, &req.channel_id).await?;
                        let mut card =
                            load_card(conn, &channel.id, &req.card_id).await?.ok_or(Error::NotFound("card"))?;
                        if card.archived_at.is_some() {
                            return Err(Error::invalid("bring the card back before moving it"));
                        }
                        card.column_id = column_in(conn, &channel.id, &req.column_id).await?;
                        card.rank = rank_for(conn, &card.column_id, &req.before_id, &card.id).await?;
                        card.updated_at = Some(timestamp(now_ms()));
                        save(conn, &card).await?;
                        events.push(Payload::CardUpdated(pb::CardUpdated { card: Some(card.clone()) }));
                        Ok(card)
                    })
                    .await?;
                Ok(pb::MoveCardResponse { card: Some(card) })
            }
            .await,
        )
    }

    async fn set_board(&self, request: Request<pb::SetBoardRequest>) -> Result<Response<pb::SetBoardResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                access.require_in(&req.channel_id, Permission::ManageChannels)?;
                if req.columns.is_empty() || req.columns.len() > MAX_COLUMNS {
                    return Err(Error::invalid(format!("a board has 1 to {MAX_COLUMNS} columns")));
                }
                if req.labels.len() > MAX_LABELS {
                    return Err(Error::invalid(format!("a board has at most {MAX_LABELS} labels")));
                }
                let mut wanted_columns = Vec::with_capacity(req.columns.len());
                for column in &req.columns {
                    if !(0..=MAX_CARD_LIMIT).contains(&column.card_limit) {
                        return Err(Error::invalid(format!("a column's limit is 0 to {MAX_CARD_LIMIT}")));
                    }
                    wanted_columns.push(pb::BoardColumn {
                        id: column.id.clone(),
                        name: text("column name", &column.name, 1, MAX_COLUMN_NAME)?,
                        color: check_color(column.color)?,
                        done: column.done,
                        card_limit: column.card_limit,
                    });
                }
                let mut wanted_labels = Vec::with_capacity(req.labels.len());
                for label in &req.labels {
                    wanted_labels.push(pb::BoardLabel {
                        id: label.id.clone(),
                        name: text("label name", &label.name, 1, MAX_LABEL_NAME)?,
                        color: check_color(label.color)?,
                    });
                }
                let (columns, labels) = sdb
                    .write(&account.id, async |conn, events| {
                        let channel = board(conn, &sdb.id, &access, &req.channel_id).await?;
                        let had_columns = columns(conn, &channel.id).await?;
                        let had_labels = labels(conn, &channel.id).await?;
                        let mut new_columns = wanted_columns.clone();
                        for column in &mut new_columns {
                            if column.id.is_empty() {
                                column.id = new_id();
                            } else if !had_columns.iter().any(|c| c.id == column.id) {
                                return Err(Error::NotFound("column"));
                            }
                        }
                        let mut new_labels = wanted_labels.clone();
                        for label in &mut new_labels {
                            if label.id.is_empty() {
                                label.id = new_id();
                            } else if !had_labels.iter().any(|l| l.id == label.id) {
                                return Err(Error::NotFound("label"));
                            }
                        }
                        let first = new_columns[0].id.clone();
                        for gone in had_columns.iter().filter(|c| !new_columns.iter().any(|n| n.id == c.id)) {
                            let holds = query_one(
                                conn,
                                "SELECT 1 FROM board_cards WHERE column_id = ?1 AND archived_at = 0 LIMIT 1",
                                [gone.id.as_str()],
                                |_| Ok(()),
                            )
                            .await?;
                            if holds.is_some() {
                                return Err(Error::FailedPrecondition(format!(
                                    "move the cards out of “{}” before removing it",
                                    gone.name
                                )));
                            }
                            conn.execute(
                                "UPDATE board_cards SET column_id = ?2 WHERE column_id = ?1",
                                (gone.id.as_str(), first.as_str()),
                            )
                            .await?;
                            conn.execute("DELETE FROM board_columns WHERE id = ?1", [gone.id.as_str()]).await?;
                        }
                        for (position, column) in new_columns.iter().enumerate() {
                            conn.execute(
                                "INSERT INTO board_columns (id, channel_id, name, color, position, done, card_limit)
                                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                                 ON CONFLICT (id) DO UPDATE SET name = excluded.name, color = excluded.color,
                                   position = excluded.position, done = excluded.done, card_limit = excluded.card_limit",
                                (
                                    column.id.as_str(),
                                    channel.id.as_str(),
                                    column.name.as_str(),
                                    i64::from(column.color),
                                    position as i64,
                                    column.done,
                                    i64::from(column.card_limit),
                                ),
                            )
                            .await?;
                        }
                        let gone_labels: HashSet<&str> = had_labels
                            .iter()
                            .filter(|l| !new_labels.iter().any(|n| n.id == l.id))
                            .map(|l| l.id.as_str())
                            .collect();
                        if !gone_labels.is_empty() {
                            let cards = query_all(
                                conn,
                                &format!("SELECT {CARD_COLUMNS} FROM board_cards WHERE channel_id = ?1"),
                                [channel.id.as_str()],
                                card_row,
                            )
                            .await?;
                            for mut card in cards {
                                if card.label_ids.iter().any(|id| gone_labels.contains(id.as_str())) {
                                    card.label_ids.retain(|id| !gone_labels.contains(id.as_str()));
                                    save(conn, &card).await?;
                                    events.push(Payload::CardUpdated(pb::CardUpdated { card: Some(card) }));
                                }
                            }
                            for id in &gone_labels {
                                conn.execute("DELETE FROM board_labels WHERE id = ?1", [*id]).await?;
                            }
                        }
                        for (position, label) in new_labels.iter().enumerate() {
                            conn.execute(
                                "INSERT INTO board_labels (id, channel_id, name, color, position)
                                 VALUES (?1, ?2, ?3, ?4, ?5)
                                 ON CONFLICT (id) DO UPDATE SET name = excluded.name, color = excluded.color,
                                   position = excluded.position",
                                (
                                    label.id.as_str(),
                                    channel.id.as_str(),
                                    label.name.as_str(),
                                    i64::from(label.color),
                                    position as i64,
                                ),
                            )
                            .await?;
                        }
                        let columns_were = had_columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ");
                        let columns_now = new_columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ");
                        let labels_were = had_labels.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ");
                        let labels_now = new_labels.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ");
                        store::audit(
                            conn,
                            &account.id,
                            Audit::new(pb::AuditAction::ChannelUpdate, &channel.id)
                                .channel(&channel.name)
                                .change("columns", columns_were, columns_now)
                                .change("labels", labels_were, labels_now),
                        )
                        .await?;
                        events.push(Payload::BoardUpdated(pb::BoardUpdated {
                            channel_id: channel.id.clone(),
                            columns: new_columns.clone(),
                            labels: new_labels.clone(),
                        }));
                        Ok((new_columns, new_labels))
                    })
                    .await?;
                Ok(pb::SetBoardResponse { columns, labels })
            }
            .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_fall_between() {
        let mut ranks = vec![between("", "")];
        // Always at the bottom, always at the top, always in the middle.
        for _ in 0..200 {
            let last = ranks.last().unwrap().clone();
            ranks.push(between(&last, ""));
        }
        for _ in 0..200 {
            let first = ranks[0].clone();
            ranks.insert(0, between("", &first));
        }
        for _ in 0..200 {
            let (a, b) = (ranks[300].clone(), ranks[301].clone());
            ranks.insert(301, between(&a, &b));
        }
        for pair in ranks.windows(2) {
            assert!(pair[0] < pair[1], "{} < {}", pair[0], pair[1]);
            assert!(!pair[1].ends_with('0'));
        }
        assert!(ranks.iter().all(|r| r.len() < 60));
    }

    #[test]
    fn spread_ranks_are_ordered() {
        for count in [0, 1, 2, 61, 62, 500] {
            let ranks = spread(count);
            assert_eq!(ranks.len(), count);
            assert!(ranks.windows(2).all(|p| p[0] < p[1]));
            assert!(ranks.iter().all(|r| !r.is_empty() && !r.ends_with('0')));
        }
    }
}
