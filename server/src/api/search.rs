//! Searching a server's messages (`SearchService`), and the word index each
//! server keeps for it in its own file (`search_*` tables,
//! migrations/server/0026_search.sql; how text becomes words is
//! `crate::search`).
//!
//! The index is written only by the indexer, one task per process, which
//! follows every server's event log: it takes in message events after the
//! last one it saw, so sends, edits and deletes reach the index from every
//! path that makes them (members, webhooks, AutoMod, bans, shared channels)
//! without those paths knowing about it. Servers that had messages before
//! search have them added in the background, newest first, a batch at a time,
//! between the live work. A search reads the index, then the messages it
//! found, and keeps only those still there and still matching, so the index
//! being a moment behind never shows a message wrongly.
//!
//! Nothing about a search is stored or logged: not its words, not who asked.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use tonic::{Request, Response, Status};

use super::{Api, Seat, messages, respond, shared, users};
use crate::app::App;
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::millis;
use crate::pb::{self, search_service_server::SearchService};
use crate::search::{self as words, Term};
use crate::servers::{Payload, ServerDb};

/// Events taken in per write.
const EVENT_BATCH: i64 = 256;
/// Older messages added per write while building an index.
const BACKFILL_BATCH: i64 = 200;
/// The pause between backfill batches, so live work and other servers get in.
const BACKFILL_PAUSE: Duration = Duration::from_millis(5);
/// How long a server whose index failed waits before the indexer tries again.
const RETRY_AFTER: Duration = Duration::from_secs(60);
/// Most words the last word typed stands for when it's a prefix.
const MAX_PREFIX_WORDS: i64 = 64;
const MAX_QUERY: usize = 200;
const MAX_TERMS: usize = 16;
const DEFAULT_LIMIT: i32 = 25;
const MAX_LIMIT: i32 = 50;
/// Docs looked up at once when filtering what the words found.
const LOOKUP_CHUNK: usize = 400;

// ── The index ───────────────────────────────────────────────────────────────

/// Word ids already known, so a backfill doesn't look up the same words over
/// and over.
type Known = HashMap<String, i64>;

/// Word ids a write took out of the index (no message has them any more), so
/// no cache hands them out again.
type Gone = HashSet<i64>;

/// Where a server's index is.
#[derive(Debug, Clone, PartialEq)]
struct State {
    version: i64,
    sequence: i64,
    /// Messages with ids below this still have to be added; `None` once none do.
    backfill: Option<String>,
}

async fn state(conn: &turso::Connection) -> Result<State> {
    query_one(conn, "SELECT version, sequence, backfill FROM search_state WHERE id = 1", (), |r| {
        Ok(State { version: r.get(0)?, sequence: r.get(1)?, backfill: r.get(2)? })
    })
    .await?
    .ok_or_else(|| Error::internal("the search index has no state row"))
}

/// The ids of `list`, adding words the index hasn't seen. Words added go in
/// `added`, to be remembered only once the write commits.
async fn word_ids(
    conn: &turso::Connection,
    known: &Known,
    added: &mut Known,
    gone: &Gone,
    list: &[String],
) -> Result<Vec<i64>> {
    let mut ids = Vec::with_capacity(list.len());
    for word in list {
        if let Some(&id) = added.get(word).or_else(|| known.get(word)).filter(|id| !gone.contains(id)) {
            ids.push(id);
            continue;
        }
        let id =
            match query_one(conn, "SELECT id FROM search_words WHERE word = ?1", [word.as_str()], |r| r.get::<i64>(0))
                .await?
            {
                Some(id) => id,
                None => {
                    query_one(conn, "INSERT INTO search_words (word) VALUES (?1) RETURNING id", [word.as_str()], |r| {
                        r.get::<i64>(0)
                    })
                    .await?
                    .ok_or_else(|| Error::internal("adding a word to the search index returned nothing"))?
                }
            };
        added.insert(word.clone(), id);
        ids.push(id);
    }
    Ok(ids)
}

/// Puts a message in the index as it is now, in place of how it was.
async fn put(
    conn: &turso::Connection,
    known: &Known,
    added: &mut Known,
    gone: &mut Gone,
    message: &pb::Message,
) -> Result<()> {
    remove(conn, gone, &message.id).await?;
    // Join messages and AutoMod alerts aren't anyone's words.
    if message.kind != pb::MessageKind::Unspecified as i32 {
        return Ok(());
    }
    let list = words::message_words(message);
    let ids = word_ids(conn, known, added, gone, &list).await?;
    let sent = message.created_at.as_ref().map_or(0, millis);
    let mut doc = words::doc_of(&message.id, sent);
    while query_one(conn, "SELECT 1 FROM search_docs WHERE doc = ?1", [doc], |r| r.get::<i64>(0)).await?.is_some() {
        doc += 1;
    }
    conn.execute(
        "INSERT INTO search_docs (doc, message_id, channel_id, author_id, has, words) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (
            doc,
            message.id.as_str(),
            message.channel_id.as_str(),
            message.author_id.as_str(),
            words::message_has(message),
            words::pack(&ids),
        ),
    )
    .await?;
    for chunk in ids.chunks(200) {
        let placeholders = (0..chunk.len()).map(|i| format!("(?{}, ?1)", i + 2)).collect::<Vec<_>>().join(", ");
        let mut params = vec![turso::Value::Integer(doc)];
        params.extend(chunk.iter().map(|&id| turso::Value::Integer(id)));
        conn.execute(format!("INSERT INTO search_postings (word, doc) VALUES {placeholders}"), params).await?;
    }
    Ok(())
}

/// Takes a message out of the index, if it's in it.
async fn remove(conn: &turso::Connection, gone: &mut Gone, message_id: &str) -> Result<()> {
    let Some((doc, packed)) =
        query_one(conn, "SELECT doc, words FROM search_docs WHERE message_id = ?1", [message_id], |r| {
            Ok((r.get::<i64>(0)?, r.get::<Vec<u8>>(1)?))
        })
        .await?
    else {
        return Ok(());
    };
    unlist(conn, gone, doc, &packed).await
}

/// Takes one doc out: its postings, any word no other message has (so
/// deleted text doesn't linger in the file, its backups or exports), then
/// itself.
async fn unlist(conn: &turso::Connection, gone: &mut Gone, doc: i64, packed: &[u8]) -> Result<()> {
    for word in words::unpack(packed) {
        conn.execute("DELETE FROM search_postings WHERE word = ?1 AND doc = ?2", (word, doc)).await?;
        let used =
            query_one(conn, "SELECT 1 FROM search_postings WHERE word = ?1 LIMIT 1", [word], |r| r.get::<i64>(0))
                .await?;
        if used.is_none() {
            conn.execute("DELETE FROM search_words WHERE id = ?1", [word]).await?;
            gone.insert(word);
        }
    }
    conn.execute("DELETE FROM search_docs WHERE doc = ?1", [doc]).await?;
    Ok(())
}

/// Takes a deleted channel's messages out of the index.
async fn remove_channel(conn: &turso::Connection, gone: &mut Gone, channel_id: &str) -> Result<()> {
    let docs = query_all(conn, "SELECT doc, words FROM search_docs WHERE channel_id = ?1", [channel_id], |r| {
        Ok((r.get::<i64>(0)?, r.get::<Vec<u8>>(1)?))
    })
    .await?;
    for (doc, packed) in docs {
        unlist(conn, gone, doc, &packed).await?;
    }
    Ok(())
}

/// Takes a committed write's words into a cache, and forgets the ones it took out.
fn remember(known: &mut Known, added: Known, gone: &Gone) {
    if !gone.is_empty() {
        known.retain(|_, id| !gone.contains(id));
    }
    known.extend(added.into_iter().filter(|(_, id)| !gone.contains(id)));
}

// ── The indexer ─────────────────────────────────────────────────────────────

/// Starts the indexer, for a process that keeps servers.
pub fn spawn_search_indexer(app: Arc<App>) {
    let tap = app.hub.search_tap();
    tokio::spawn(async move {
        let shutdown = app.shutdown.clone();
        tokio::select! {
            _ = shutdown.cancelled() => {}
            _ = Indexer::new(app).run(tap) => {}
        }
    });
}

struct Indexer {
    app: Arc<App>,
    /// Servers with events the index hasn't taken in yet.
    dirty: BTreeSet<String>,
    /// Servers with older messages still to add, and the words each has
    /// looked up so far.
    building: HashMap<String, Building>,
    /// Servers whose index failed, and when to try again.
    failed: HashMap<String, Instant>,
    /// Which building server goes next, so each gets a turn.
    turn: usize,
}

struct Building {
    known: Known,
    started: Instant,
    added: u64,
}

impl Indexer {
    fn new(app: Arc<App>) -> Self {
        let dirty = app.servers.all().iter().map(|sdb| sdb.id.clone()).collect();
        Self { app, dirty, building: HashMap::new(), failed: HashMap::new(), turn: 0 }
    }

    async fn run(mut self, mut tap: tokio::sync::mpsc::UnboundedReceiver<Arc<pb::Event>>) {
        loop {
            while let Ok(event) = tap.try_recv() {
                self.note(&event);
            }
            let now = Instant::now();
            self.failed.retain(|_, until| *until > now);
            if let Some(id) = self.next_dirty() {
                if let Err(err) = self.catch_up(&id).await {
                    self.fail(&id, "search::catch_up", &err);
                }
                continue;
            }
            if let Some(id) = self.next_building() {
                if let Err(err) = self.backfill(&id).await {
                    self.fail(&id, "search::backfill", &err);
                }
                tokio::time::sleep(BACKFILL_PAUSE).await;
                continue;
            }
            let wait = self.failed.values().min().map_or(RETRY_AFTER, |until| until.saturating_duration_since(now));
            tokio::select! {
                event = tap.recv() => match event {
                    Some(event) => self.note(&event),
                    None => return,
                },
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }

    /// Marks a server whose log grew.
    fn note(&mut self, event: &pb::Event) {
        if event.sequence > 0 {
            self.dirty.insert(event.server_id.clone());
        }
    }

    fn next_dirty(&mut self) -> Option<String> {
        let id = self.dirty.iter().find(|id| !self.failed.contains_key(*id))?.clone();
        self.dirty.remove(&id);
        Some(id)
    }

    fn next_building(&mut self) -> Option<String> {
        let mut ids: Vec<&String> = self.building.keys().filter(|id| !self.failed.contains_key(*id)).collect();
        if ids.is_empty() {
            return None;
        }
        ids.sort();
        self.turn = self.turn.wrapping_add(1);
        Some(ids[self.turn % ids.len()].clone())
    }

    /// Notes a failure and tries the server again later. A server that went
    /// away (moved, deleted) is simply let go.
    fn fail(&mut self, id: &str, place: &str, err: &Error) {
        if !self.app.servers.holds(id) {
            self.building.remove(id);
            return;
        }
        if matches!(err, Error::Moving) {
            self.dirty.insert(id.to_string());
            self.failed.insert(id.to_string(), Instant::now() + Duration::from_secs(5));
            return;
        }
        tracing::warn!("couldn't update a server's search index; trying again in a minute");
        crate::reports::server_error("search_index", Some(place));
        self.dirty.insert(id.to_string());
        self.failed.insert(id.to_string(), Instant::now() + RETRY_AFTER);
    }

    async fn server(&self, id: &str) -> Option<Arc<ServerDb>> {
        self.app.servers.get(id).await.ok()
    }

    /// Takes in the server's events since the index last did.
    async fn catch_up(&mut self, id: &str) -> Result<()> {
        let Some(sdb) = self.server(id).await else {
            self.building.remove(id);
            return Ok(());
        };
        let mut current = state(&*sdb.read()?).await?;
        if current.version != words::VERSION {
            self.building.remove(id);
            current = rebuild(&sdb).await?;
        }
        loop {
            let events = sdb.events_after(current.sequence, EVENT_BATCH).await?;
            let Some(last) = events.last().map(|e| e.sequence) else { break };
            let known = self.building.get(id).map(|b| &b.known);
            let empty = Known::new();
            let known = known.unwrap_or(&empty);
            let (added, gone) = sdb
                .write_quiet(async |conn| {
                    let (mut added, mut gone) = (Known::new(), Gone::new());
                    for event in &events {
                        match &event.payload {
                            Some(Payload::MessageCreated(pb::MessageCreated { message: Some(message) }))
                            | Some(Payload::MessageUpdated(pb::MessageUpdated { message: Some(message) })) => {
                                put(conn, known, &mut added, &mut gone, message).await?
                            }
                            Some(Payload::MessageDeleted(deleted)) => {
                                remove(conn, &mut gone, &deleted.message_id).await?
                            }
                            Some(Payload::ChannelDeleted(deleted)) => {
                                remove_channel(conn, &mut gone, &deleted.channel_id).await?
                            }
                            _ => {}
                        }
                    }
                    conn.execute("UPDATE search_state SET sequence = ?1 WHERE id = 1", [last]).await?;
                    Ok((added, gone))
                })
                .await?;
            if let Some(building) = self.building.get_mut(id) {
                remember(&mut building.known, added, &gone);
            }
            current.sequence = last;
            if (events.len() as i64) < EVENT_BATCH {
                break;
            }
        }
        if current.backfill.is_some() {
            self.building.entry(id.to_string()).or_insert_with(|| Building {
                known: Known::new(),
                started: Instant::now(),
                added: 0,
            });
        }
        Ok(())
    }

    /// Adds the next batch of a server's older messages.
    async fn backfill(&mut self, id: &str) -> Result<()> {
        let Some(sdb) = self.server(id).await else {
            self.building.remove(id);
            return Ok(());
        };
        let current = state(&*sdb.read()?).await?;
        let Some(before) = current.backfill else {
            self.building.remove(id);
            return Ok(());
        };
        let started = Instant::now();
        let batch = messages::older(&*sdb.read()?, &sdb.id, &before, BACKFILL_BATCH).await?;
        let next = batch.last().map(|m| m.id.clone());
        let Some(building) = self.building.get(id) else { return Ok(()) };
        let known = &building.known;
        let added = sdb
            .write_quiet(async |conn| {
                let (mut added, mut gone) = (Known::new(), Gone::new());
                for message in &batch {
                    put(conn, known, &mut added, &mut gone, message).await?;
                }
                conn.execute("UPDATE search_state SET backfill = ?1 WHERE id = 1", [next.as_deref()]).await?;
                Ok((added, gone))
            })
            .await?;
        crate::reports::server_timing("search.backfill_batch", started.elapsed());
        let building = self.building.get_mut(id).expect("checked above");
        remember(&mut building.known, added.0, &added.1);
        building.added += batch.len() as u64;
        if next.is_none() {
            let done = self.building.remove(id).expect("checked above");
            tracing::info!(
                messages = done.added,
                took_ms = done.started.elapsed().as_millis() as u64,
                "built a server's search index"
            );
            crate::reports::server_timing("search.backfill", done.started.elapsed());
            crate::reports::server_used("search.backfilled_messages", done.added);
        }
        Ok(())
    }
}

/// Empties a server's index and starts it again, for a new way of cutting
/// words. Goes a few thousand rows at a time, so no one write grows huge.
async fn rebuild(sdb: &ServerDb) -> Result<State> {
    for table in ["search_postings", "search_docs", "search_words"] {
        loop {
            let gone = sdb
                .write_quiet(async |conn| {
                    Ok(conn
                        .execute(
                            format!("DELETE FROM {table} WHERE rowid IN (SELECT rowid FROM {table} LIMIT 5000)"),
                            (),
                        )
                        .await?)
                })
                .await?;
            if gone == 0 {
                break;
            }
        }
    }
    let head = sdb.head_sequence().await?;
    sdb.write_quiet(async |conn| {
        conn.execute(
            "UPDATE search_state SET version = ?1, sequence = ?2,
             backfill = CASE WHEN EXISTS (SELECT 1 FROM messages) THEN '~' ELSE NULL END WHERE id = 1",
            (words::VERSION, head),
        )
        .await?;
        Ok(())
    })
    .await?;
    state(&*sdb.read()?).await
}

// ── Finding ─────────────────────────────────────────────────────────────────

/// What a search looks for, checked and narrowed to what the searcher sees.
struct Find {
    terms: Vec<Term>,
    /// For each person who must be mentioned, the names a mention of them is
    /// under (their username, and their id).
    mentions: Vec<Vec<String>>,
    authors: Vec<String>,
    /// Only these channels; `None` for every channel (they can see them all).
    channels: Option<Vec<String>>,
    has: i64,
    /// Docs from `lo` up to (not including) `hi`.
    lo: i64,
    hi: i64,
    limit: usize,
    /// Count every match (the first page).
    count: bool,
}

/// What the index found: the page's docs and message ids, newest first.
struct Found {
    page: Vec<(i64, String)>,
    total: i64,
    /// The count stopped at the read budget: there are at least `total`.
    total_at_least: bool,
    /// Where the next page starts (exclusive), if anything is left to look at.
    next: Option<i64>,
}

impl Found {
    fn none() -> Self {
        Self { page: vec![], total: 0, total_at_least: false, next: None }
    }
}

/// The ids of the words a term stands for.
async fn term_ids(conn: &turso::Connection, term: &Term) -> Result<Vec<i64>> {
    if term.prefix {
        query_all(
            conn,
            "SELECT id FROM search_words WHERE word >= ?1 AND word < ?2 ORDER BY word LIMIT ?3",
            (term.word.as_str(), format!("{}\u{10FFFF}", term.word), MAX_PREFIX_WORDS),
            |r| r.get::<i64>(0),
        )
        .await
    } else {
        Ok(query_one(conn, "SELECT id FROM search_words WHERE word = ?1", [term.word.as_str()], |r| r.get::<i64>(0))
            .await?
            .into_iter()
            .collect())
    }
}

fn placeholders(from: usize, count: usize) -> String {
    (from..from + count).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ")
}

/// Index rows one search may read, all queries together. Past it the search
/// stops where it got to and hands back a cursor, so no search costs more
/// than this however common its words or big the server.
const READ_BUDGET: usize = 50_000;
/// Postings counted per lookup when choosing the rarest one to walk.
const RARITY_CAP: i64 = 1_000;
/// Postings read per step of the walk, shared between a prefix's words.
const WALK_STEP: usize = 2_000;
/// Docs read per step of a search with filters and no words.
const SCAN_STEP: i64 = 1_000;

/// Counts what a search reads against `READ_BUDGET`.
struct Budget(usize);

impl Budget {
    fn spend(&mut self, rows: usize) {
        self.0 = self.0.saturating_sub(rows);
    }
    fn spent(&self) -> bool {
        self.0 == 0
    }
}

/// About how many docs in range have any of `ids`, counting no further than
/// `RARITY_CAP`.
async fn rarity(conn: &turso::Connection, ids: &[i64], lo: i64, hi: i64, budget: &mut Budget) -> Result<i64> {
    let mut seen = 0;
    for &id in ids {
        let n = query_one(
            conn,
            "SELECT count(*) FROM (SELECT 1 FROM search_postings WHERE word = ?1 AND doc >= ?2 AND doc < ?3 LIMIT ?4)",
            (id, lo, hi, RARITY_CAP - seen),
            |r| r.get::<i64>(0),
        )
        .await?
        .unwrap_or(0);
        budget.spend(n as usize);
        seen += n;
        if seen >= RARITY_CAP {
            break;
        }
    }
    Ok(seen)
}

/// The next docs below `cursor` having any of `ids`, newest first, and where
/// they reach down to: every such doc from there up to `cursor` is in the
/// list (`lo` once there are no more).
async fn walk(
    conn: &turso::Connection,
    ids: &[i64],
    lo: i64,
    cursor: i64,
    budget: &mut Budget,
) -> Result<(Vec<i64>, i64)> {
    let step = (WALK_STEP / ids.len()).max(50) as i64;
    let mut docs = Vec::new();
    let mut boundary = lo;
    for &id in ids {
        let got = query_all(
            conn,
            "SELECT doc FROM search_postings WHERE word = ?1 AND doc >= ?2 AND doc < ?3 ORDER BY doc DESC LIMIT ?4",
            (id, lo, cursor, step),
            |r| r.get::<i64>(0),
        )
        .await?;
        budget.spend(got.len());
        // A word with more below: only docs down to its last one are known.
        if got.len() as i64 == step
            && let Some(&last) = got.last()
        {
            boundary = boundary.max(last);
        }
        docs.extend(got);
    }
    docs.retain(|&doc| doc >= boundary);
    docs.sort_unstable_by(|a, b| b.cmp(a));
    docs.dedup();
    Ok((docs, boundary))
}

/// A doc's row, for the checks words can't do.
struct DocRow {
    message_id: String,
    channel_id: String,
    author_id: String,
    has: i64,
    words: Vec<u8>,
}

async fn doc_rows(conn: &turso::Connection, docs: &[i64]) -> Result<HashMap<i64, DocRow>> {
    Ok(query_all(
        conn,
        &format!(
            "SELECT doc, message_id, channel_id, author_id, has, words FROM search_docs WHERE doc IN ({})",
            placeholders(1, docs.len())
        ),
        docs.iter().map(|&d| turso::Value::Integer(d)).collect::<Vec<_>>(),
        |r| {
            Ok((
                r.get::<i64>(0)?,
                DocRow {
                    message_id: r.get(1)?,
                    channel_id: r.get(2)?,
                    author_id: r.get(3)?,
                    has: r.get(4)?,
                    words: r.get(5)?,
                },
            ))
        },
    )
    .await?
    .into_iter()
    .collect())
}

/// Collects a search's matches in order, counting them on the first page.
struct Collect<'a> {
    find: &'a Find,
    channels: Option<HashSet<&'a str>>,
    authors: HashSet<&'a str>,
    page: Vec<(i64, String)>,
    total: i64,
    /// Found one past the page.
    more: bool,
}

impl<'a> Collect<'a> {
    fn new(find: &'a Find) -> Self {
        Self {
            find,
            channels: find.channels.as_ref().map(|c| c.iter().map(String::as_str).collect()),
            authors: find.authors.iter().map(String::as_str).collect(),
            page: vec![],
            total: 0,
            more: false,
        }
    }

    fn passes(&self, row: &DocRow) -> bool {
        self.channels.as_ref().is_none_or(|c| c.contains(row.channel_id.as_str()))
            && (self.authors.is_empty() || self.authors.contains(row.author_id.as_str()))
            && row.has & self.find.has == self.find.has
    }

    fn take(&mut self, doc: i64, message_id: &str) {
        self.total += 1;
        if self.page.len() < self.find.limit {
            self.page.push((doc, message_id.to_string()));
        } else {
            self.more = true;
        }
    }

    /// Nothing more to look for: the page is full, and no count is wanted
    /// (or one past it was found).
    fn done(&self) -> bool {
        self.more && !self.find.count
    }

    /// The result, having looked at everything from `reached` up.
    fn finish(self, reached: i64, out_of_budget: bool) -> Found {
        let lo = self.find.lo;
        let next = if self.more {
            self.page.last().map(|(doc, _)| *doc)
        } else if out_of_budget && reached > lo {
            Some(reached)
        } else {
            None
        };
        Found {
            total: if self.find.count { self.total } else { 0 },
            total_at_least: self.find.count && out_of_budget && reached > lo,
            page: self.page,
            next,
        }
    }
}

/// Finds a page: walks the rarest word's postings newest first and checks
/// each doc's own word list for the others, so the work goes with the page
/// and the budget, never with how common the words are.
async fn find(conn: &turso::Connection, find: &Find) -> Result<Found> {
    let mut lookups: Vec<Vec<i64>> = Vec::new();
    for term in &find.terms {
        lookups.push(term_ids(conn, term).await?);
    }
    for names in &find.mentions {
        let mut ids = Vec::new();
        for name in names {
            ids.extend(term_ids(conn, &Term { word: words::mention_word(name), prefix: false }).await?);
        }
        lookups.push(ids);
    }
    if lookups.is_empty() {
        return find_by_filters(conn, find).await;
    }
    if lookups.iter().any(Vec::is_empty) {
        return Ok(Found::none());
    }
    let mut budget = Budget(READ_BUDGET);
    let mut rarest = (0, i64::MAX);
    for (i, ids) in lookups.iter().enumerate() {
        let n = rarity(conn, ids, find.lo, find.hi, &mut budget).await?;
        if n < rarest.1 {
            rarest = (i, n);
        }
    }
    let driver = lookups.swap_remove(rarest.0);
    let others: Vec<HashSet<i64>> = lookups.into_iter().map(|ids| ids.into_iter().collect()).collect();

    let mut collect = Collect::new(find);
    let mut cursor = find.hi;
    loop {
        let (docs, boundary) = walk(conn, &driver, find.lo, cursor, &mut budget).await?;
        for chunk in docs.chunks(LOOKUP_CHUNK) {
            let rows = doc_rows(conn, chunk).await?;
            budget.spend(chunk.len());
            for doc in chunk {
                let Some(row) = rows.get(doc) else { continue };
                if !collect.passes(row) {
                    continue;
                }
                if !others.is_empty() {
                    let has: HashSet<i64> = words::unpack(&row.words).into_iter().collect();
                    if !others.iter().all(|ids| ids.iter().any(|id| has.contains(id))) {
                        continue;
                    }
                }
                collect.take(*doc, &row.message_id);
                if collect.done() {
                    return Ok(collect.finish(*doc, false));
                }
            }
        }
        cursor = boundary;
        if boundary <= find.lo {
            return Ok(collect.finish(find.lo, false));
        }
        if budget.spent() {
            return Ok(collect.finish(boundary, true));
        }
    }
}

/// A search with filters and no words: through the docs, newest first, a
/// step at a time. One channel or one author narrows the read to theirs.
async fn find_by_filters(conn: &turso::Connection, find: &Find) -> Result<Found> {
    let (narrow, value) = match (&find.channels, find.authors.as_slice()) {
        (Some(channels), _) if channels.len() == 1 => (" AND channel_id = ?4", channels[0].clone()),
        (_, [author]) => (" AND author_id = ?4", author.clone()),
        _ => ("", String::new()),
    };
    let sql = format!(
        "SELECT doc, message_id, channel_id, author_id, has FROM search_docs
         WHERE doc >= ?1 AND doc < ?2{narrow} ORDER BY doc DESC LIMIT ?3"
    );
    let mut budget = Budget(READ_BUDGET);
    let mut collect = Collect::new(find);
    let mut cursor = find.hi;
    loop {
        let mut params =
            vec![turso::Value::Integer(find.lo), turso::Value::Integer(cursor), turso::Value::Integer(SCAN_STEP)];
        if !narrow.is_empty() {
            params.push(turso::Value::from(value.as_str()));
        }
        let rows = query_all(conn, &sql, params, |r| {
            Ok((
                r.get::<i64>(0)?,
                DocRow {
                    message_id: r.get(1)?,
                    channel_id: r.get(2)?,
                    author_id: r.get(3)?,
                    has: r.get(4)?,
                    words: vec![],
                },
            ))
        })
        .await?;
        budget.spend(rows.len());
        for (doc, row) in &rows {
            if collect.passes(row) {
                collect.take(*doc, &row.message_id);
                if collect.done() {
                    return Ok(collect.finish(*doc, false));
                }
            }
        }
        let Some(&(last, _)) = rows.last().filter(|_| rows.len() as i64 == SCAN_STEP) else {
            return Ok(collect.finish(find.lo, false));
        };
        cursor = last;
        if budget.spent() {
            return Ok(collect.finish(last, true));
        }
    }
}

/// How far along a server's backfill is, 0 to 100, by time: from its newest
/// message back to its oldest.
async fn indexed_percent(conn: &turso::Connection, backfill: &str) -> Result<i32> {
    let ms = |id: &str| ulid::Ulid::from_string(id).ok().map(|u| u.timestamp_ms() as i64);
    let Some((oldest, newest)) = query_one(conn, "SELECT min(id), max(id) FROM messages", (), |r| {
        Ok((r.get::<Option<String>>(0)?, r.get::<Option<String>>(1)?))
    })
    .await?
    else {
        return Ok(0);
    };
    let (Some(oldest), Some(newest), Some(at)) =
        (oldest.as_deref().and_then(ms), newest.as_deref().and_then(ms), ms(backfill))
    else {
        return Ok(0);
    };
    if newest <= oldest {
        return Ok(0);
    }
    Ok((((newest - at) * 100) / (newest - oldest)).clamp(0, 99) as i32)
}

// ── Limits ──────────────────────────────────────────────────────────────────

/// Searches each account may make: a burst of this many...
const BURST: f64 = 10.0;
/// ...then this many a second.
const PER_SECOND: f64 = 2.0;

/// Each account's searches lately, in memory only (account ids, never
/// addresses). Accounts idle for a minute are forgotten.
static LIMITS: LazyLock<Mutex<HashMap<String, (f64, Instant)>>> = LazyLock::new(Default::default);

fn allow(account_id: &str) -> bool {
    let now = Instant::now();
    let mut limits = LIMITS.lock().unwrap_or_else(|p| p.into_inner());
    if limits.len() > 4096 {
        limits.retain(|_, (_, at)| now.duration_since(*at) < Duration::from_secs(60));
    }
    let (tokens, at) = limits.entry(account_id.to_string()).or_insert((BURST, now));
    *tokens = (*tokens + now.duration_since(*at).as_secs_f64() * PER_SECOND).min(BURST);
    *at = now;
    if *tokens < 1.0 {
        return false;
    }
    *tokens -= 1.0;
    true
}

// ── The service ─────────────────────────────────────────────────────────────

/// Ids a client sent, checked, once each, at most `max`.
fn ids(field: &str, list: &[String], max: usize) -> Result<Vec<String>> {
    if list.len() > max {
        return Err(Error::invalid(format!("at most {max} {field}")));
    }
    let mut out = Vec::with_capacity(list.len());
    for id in list {
        if id.is_empty() || id.len() > 32 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(Error::invalid(format!("{field} must be ids")));
        }
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    Ok(out)
}

#[tonic::async_trait]
impl SearchService for Api {
    async fn search_messages(
        &self,
        request: Request<pb::SearchMessagesRequest>,
    ) -> Result<Response<pb::SearchMessagesResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                if req.query.chars().count() > MAX_QUERY {
                    return Err(Error::invalid(format!("searches can be at most {MAX_QUERY} characters")));
                }
                let terms = words::query_terms(&req.query);
                if terms.len() > MAX_TERMS {
                    return Err(Error::invalid(format!("search for at most {MAX_TERMS} words at once")));
                }
                let authors = ids("authors", &req.author_ids, 25)?;
                let requested = ids("channels", &req.channel_ids, 100)?;
                let mentions = ids("mentions", &req.mention_ids, 10)?;
                let has = req
                    .has
                    .iter()
                    .map(|&h| pb::SearchHas::try_from(h).map(words::has_bit).unwrap_or(0))
                    .fold(0, |bits, bit| bits | bit);
                let after = req.after.as_ref().map(millis);
                let before = req.before.as_ref().map(millis);
                let filtered = !authors.is_empty()
                    || !requested.is_empty()
                    || !mentions.is_empty()
                    || has != 0
                    || after.is_some()
                    || before.is_some();
                if terms.is_empty() && !filtered {
                    if req.query.trim().is_empty() {
                        return Err(Error::invalid("search for some words, or pick a filter"));
                    }
                    // Only words too short to look for.
                    return Ok(pb::SearchMessagesResponse::default());
                }
                let limit = if req.limit <= 0 { DEFAULT_LIMIT } else { req.limit.min(MAX_LIMIT) } as usize;
                let cursor = match req.cursor.as_str() {
                    "" => None,
                    c => Some(c.parse::<i64>().map_err(|_| Error::invalid("that cursor isn't one of ours"))?),
                };

                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                if !allow(&account.id) {
                    return Err(Error::ResourceExhausted("you're searching too fast; try again in a moment".into()));
                }
                let conn = sdb.read()?;

                // Only channels they can see, whatever they asked for.
                let visible = access.visible();
                let channels = if requested.is_empty() {
                    let all = query_all(&conn, "SELECT id FROM channels", (), |r| r.get::<String>(0)).await?;
                    if all.iter().all(|id| visible.contains(id)) {
                        None
                    } else {
                        Some(all.into_iter().filter(|id| visible.contains(id)).collect::<Vec<_>>())
                    }
                } else {
                    Some(requested.into_iter().filter(|id| visible.contains(id)).collect::<Vec<_>>())
                };
                if channels.as_ref().is_some_and(Vec::is_empty) {
                    return Ok(pb::SearchMessagesResponse::default());
                }

                // Mentions are written with usernames (and ids, from webhooks).
                let mentioned = users(&conn, &mentions.iter().map(String::as_str).collect::<Vec<_>>()).await?;
                let mentions = mentions
                    .iter()
                    .map(|id| {
                        let mut names = vec![id.clone()];
                        if let Some(user) = mentioned.iter().find(|u| &u.id == id) {
                            names.push(user.username.to_ascii_lowercase());
                        }
                        names
                    })
                    .collect();
                let lo = after.map_or(0, words::doc_floor);
                let hi = before.map_or(i64::MAX, words::doc_floor).min(cursor.unwrap_or(i64::MAX));
                let found = find(
                    &conn,
                    &Find {
                        terms: terms.clone(),
                        mentions,
                        authors,
                        channels,
                        has,
                        lo,
                        hi,
                        limit,
                        count: cursor.is_none(),
                    },
                )
                .await?;

                // The messages as they are now: gone ones drop out, and so do
                // ones edited a moment ago that no longer have the words.
                let page_ids: Vec<&str> = found.page.iter().map(|(_, id)| id.as_str()).collect();
                let mut by_id: HashMap<String, pb::Message> =
                    messages::by_ids(&conn, &sdb.id, &page_ids).await?.into_iter().map(|m| (m.id.clone(), m)).collect();
                let mut kept: Vec<pb::Message> = page_ids
                    .iter()
                    .filter_map(|id| by_id.remove(*id))
                    .filter(|m| access.can_see(&m.channel_id) && words::has_all(m, &terms))
                    .collect();
                shared::mark_guests(&conn, &mut kept).await?;
                let authors = users(&conn, &kept.iter().map(|m| m.author_id.as_str()).collect::<Vec<_>>()).await?;
                let results = kept
                    .into_iter()
                    .map(|message| pb::SearchResult {
                        highlights: words::highlights(&message.content, &terms),
                        message: Some(message),
                    })
                    .collect();

                let current = state(&conn).await?;
                let indexed_percent = match &current.backfill {
                    Some(backfill) => indexed_percent(&conn, backfill).await?,
                    None => 0,
                };
                Ok(pb::SearchMessagesResponse {
                    results,
                    authors,
                    total: found.total,
                    next_cursor: found.next.map(|doc| doc.to_string()).unwrap_or_default(),
                    total_at_least: found.total_at_least,
                    indexing: current.backfill.is_some() || current.version != words::VERSION,
                    indexed_percent,
                })
            }
            .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_each_account() {
        let id = "01LIMITTESTACCOUNT";
        for _ in 0..BURST as usize {
            assert!(allow(id));
        }
        assert!(!allow(id), "past the burst");
        assert!(allow("01SOMEONEELSE"), "others aren't held back");
    }
}
