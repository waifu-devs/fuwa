//! Mod View (`ModerationService`, docs/mod-view.md): the cases AutoMod and
//! members' reports open in a server's file (`mod_cases`, `mod_reports`),
//! moderators' notes (`mod_notes`), and what a moderator sees of one person.
//! A case about a message in a channel the moderator can't see doesn't exist
//! for them, and a person's messages are listed from their channels alone.

use std::collections::{HashMap, HashSet};

use prost::Message as _;
use tonic::{Request, Response, Status};

use super::messages::{MESSAGE_COLUMNS, load_message, message_row, reviewed_text, with_extras};
use super::{Api, Seat, polls, reactions, respond, shared, text, threads, users};
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb::{
    self, ModCaseKind as Kind, ModCaseStatus as CaseStatus, Permission, moderation_service_server::ModerationService,
};
use crate::permissions::Access;
use crate::servers::{self as store, Audit, Payload, USER_COLUMNS, load_channel, load_channels};

/// A report's note, and a moderator's reason for closing a case.
const MAX_REASON: usize = 500;
const MAX_NOTE: usize = 1000;
/// How much of a person's history and notes their overview shows.
const HISTORY: i64 = 50;
const NOTES: i64 = 100;
/// Rows read at a time while filling a page with what the caller can see.
const BATCH: i64 = 200;
/// How long a search for someone may be.
const MAX_QUERY: usize = 100;

/// What a person's history in Mod View shows: what moderators, and AutoMod,
/// did to them.
const HISTORY_ACTIONS: [pb::AuditAction; 13] = [
    pb::AuditAction::MemberUpdate,
    pb::AuditAction::MemberTimeOut,
    pb::AuditAction::MemberKick,
    pb::AuditAction::MemberBan,
    pb::AuditAction::MemberUnban,
    pb::AuditAction::MessageDelete,
    pb::AuditAction::MemberRolesUpdate,
    pb::AuditAction::ApplicationApprove,
    pb::AuditAction::ApplicationReject,
    pb::AuditAction::AutoModTimeOut,
    pb::AuditAction::AutoModMessageDelete,
    pb::AuditAction::ThreadDelete,
    pb::AuditAction::ModCaseUpdate,
];

const CASE_COLUMNS: &str = "id, kind, status, subject_id, channel_id, message_id, thread_id, content, auto_mod, \
     report_count, created_at, updated_at, closed_by, closed_at, resolution";

fn case_row(server_id: &str) -> impl Fn(&turso::Row) -> turso::Result<(pb::ModCase, Option<Vec<u8>>)> + '_ {
    move |r| {
        Ok((
            pb::ModCase {
                id: r.get(0)?,
                server_id: server_id.to_string(),
                kind: r.get(1)?,
                status: r.get(2)?,
                subject_id: r.get(3)?,
                channel_id: r.get(4)?,
                message_id: r.get(5)?,
                thread_id: r.get(6)?,
                content: r.get(7)?,
                auto_mod: None,
                report_count: r.get(9)?,
                reasons: vec![],
                created_at: Some(timestamp(r.get(10)?)),
                updated_at: Some(timestamp(r.get(11)?)),
                closed_by_id: r.get(12)?,
                closed_at: r.get::<Option<i64>>(13)?.map(timestamp),
                resolution: r.get(14)?,
            },
            r.get::<Option<Vec<u8>>>(8)?,
        ))
    }
}

fn with_alert((mut case, alert): (pb::ModCase, Option<Vec<u8>>)) -> Result<pb::ModCase> {
    if let Some(bytes) = alert {
        case.auto_mod = Some(pb::AutoModAlert::decode(bytes.as_slice())?);
    }
    Ok(case)
}

/// Fills in each case's reasons, from its reports.
async fn add_reasons(conn: &turso::Connection, cases: &mut [pb::ModCase]) -> Result<()> {
    let ids: Vec<&str> = cases.iter().filter(|c| c.report_count > 0).map(|c| c.id.as_str()).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    let rows = query_all(
        conn,
        &format!("SELECT DISTINCT case_id, reason FROM mod_reports WHERE case_id IN ({placeholders}) ORDER BY reason"),
        ids.iter().map(|id| turso::Value::from(*id)).collect::<Vec<_>>(),
        |r| Ok((r.get::<String>(0)?, r.get::<i32>(1)?)),
    )
    .await?;
    let mut by_case: HashMap<String, Vec<i32>> = HashMap::new();
    for (case_id, reason) in rows {
        by_case.entry(case_id).or_default().push(reason);
    }
    for case in cases {
        case.reasons = by_case.remove(&case.id).unwrap_or_default();
    }
    Ok(())
}

async fn load_case(conn: &turso::Connection, server_id: &str, case_id: &str) -> Result<Option<pb::ModCase>> {
    let Some(row) =
        query_one(conn, &format!("SELECT {CASE_COLUMNS} FROM mod_cases WHERE id = ?1"), [case_id], case_row(server_id))
            .await?
    else {
        return Ok(None);
    };
    let mut case = with_alert(row)?;
    add_reasons(conn, std::slice::from_mut(&mut case)).await?;
    Ok(Some(case))
}

/// Whether a moderator can see a case: one about a message only if they can
/// see its channel.
fn shown(access: &Access, case: &pb::ModCase) -> bool {
    case.channel_id.is_empty() || access.can_see(&case.channel_id)
}

/// Whether someone gets an event about a case or a note: moderators, and for
/// a case about a message only those who can see its channel.
pub(super) fn event_shown(access: &Access, payload: &Payload) -> bool {
    if !access.has(Permission::Moderate) {
        return false;
    }
    match payload {
        Payload::ModCaseUpdated(pb::ModCaseUpdated { mod_case }) => mod_case.as_ref().is_some_and(|c| shown(access, c)),
        _ => true,
    }
}

async fn insert_case(conn: &turso::Connection, case: &pb::ModCase, open_key: Option<&str>) -> Result<()> {
    let alert = case.auto_mod.as_ref().map_or(turso::Value::Null, |a| turso::Value::Blob(a.encode_to_vec()));
    let at = case.created_at.as_ref().map_or_else(now_ms, crate::id::millis);
    conn.execute(
        "INSERT INTO mod_cases (id, kind, status, subject_id, channel_id, message_id, thread_id, content, auto_mod,
                                open_key, report_count, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
        vec![
            turso::Value::from(case.id.as_str()),
            turso::Value::from(i64::from(case.kind)),
            turso::Value::from(i64::from(case.status)),
            turso::Value::from(case.subject_id.as_str()),
            turso::Value::from(case.channel_id.as_str()),
            turso::Value::from(case.message_id.as_str()),
            turso::Value::from(case.thread_id.as_str()),
            turso::Value::from(case.content.as_str()),
            alert,
            open_key.map_or(turso::Value::Null, turso::Value::from),
            turso::Value::from(i64::from(case.report_count)),
            turso::Value::from(at),
        ],
    )
    .await?;
    Ok(())
}

/// A message AutoMod caught: a case for the moderators, in the write that
/// acted on it. `message_id` is empty when it was never sent.
pub(super) async fn auto_mod_case(
    conn: &turso::Connection,
    server_id: &str,
    subject_id: &str,
    message_id: &str,
    thread_id: &str,
    alert: pb::AutoModAlert,
    events: &mut Vec<Payload>,
) -> Result<()> {
    let now = Some(timestamp(now_ms()));
    let case = pb::ModCase {
        id: new_id(),
        server_id: server_id.to_string(),
        kind: Kind::AutoMod as i32,
        status: CaseStatus::Open as i32,
        subject_id: subject_id.to_string(),
        channel_id: alert.channel_id.clone(),
        message_id: message_id.to_string(),
        thread_id: thread_id.to_string(),
        content: alert.content.clone(),
        auto_mod: Some(alert),
        created_at: now,
        updated_at: now,
        ..Default::default()
    };
    insert_case(conn, &case, None).await?;
    events.push(Payload::ModCaseUpdated(pb::ModCaseUpdated { mod_case: Some(case) }));
    Ok(())
}

/// Adds `reporter`'s report to the open case under `key`, opening `fresh`
/// when there's none. Every report updates the case's row, so reports at
/// once clash and run again, and the count stays right.
#[allow(clippy::too_many_arguments)]
async fn report(
    conn: &turso::Connection,
    server_id: &str,
    key: &str,
    fresh: pb::ModCase,
    reporter_id: &str,
    reason: pb::ModReportReason,
    note: &str,
    events: &mut Vec<Payload>,
) -> Result<()> {
    let now = now_ms();
    let open = query_one(conn, "SELECT id FROM mod_cases WHERE open_key = ?1", [key], |r| r.get::<String>(0)).await?;
    let case_id = match open {
        Some(id) => id,
        None => {
            insert_case(conn, &fresh, Some(key)).await?;
            fresh.id
        }
    };
    let again = query_one(
        conn,
        "SELECT 1 FROM mod_reports WHERE case_id = ?1 AND reporter_id = ?2",
        (case_id.as_str(), reporter_id),
        |r| r.get::<i64>(0),
    )
    .await?
    .is_some();
    if again {
        conn.execute(
            "UPDATE mod_reports SET reason = ?3, note = ?4 WHERE case_id = ?1 AND reporter_id = ?2",
            (case_id.as_str(), reporter_id, reason as i64, note),
        )
        .await?;
    } else {
        conn.execute(
            "INSERT INTO mod_reports (case_id, reporter_id, reason, note, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            (case_id.as_str(), reporter_id, reason as i64, note, now),
        )
        .await?;
    }
    conn.execute(
        "UPDATE mod_cases SET report_count = report_count + ?2, updated_at = ?3 WHERE id = ?1",
        (case_id.as_str(), i64::from(!again), now),
    )
    .await?;
    let case = load_case(conn, server_id, &case_id).await?.ok_or(Error::NotFound("case"))?;
    events.push(Payload::ModCaseUpdated(pb::ModCaseUpdated { mod_case: Some(case) }));
    Ok(())
}

fn reason_of(value: i32) -> Result<pb::ModReportReason> {
    match pb::ModReportReason::try_from(value) {
        Ok(pb::ModReportReason::Unspecified) | Err(_) => Err(Error::invalid("pick a reason for the report")),
        Ok(reason) => Ok(reason),
    }
}

/// Forgets a deleted account in a server's Mod View: the cases and notes
/// about them, and their reports (a report case nobody's report is left in
/// goes too). Notes they wrote stay, under the placeholder they become.
pub(crate) async fn forget_person(
    conn: &turso::Connection,
    server_id: &str,
    account_id: &str,
    events: &mut Vec<Payload>,
) -> Result<()> {
    let about =
        query_all(conn, "SELECT id FROM mod_cases WHERE subject_id = ?1", [account_id], |r| r.get::<String>(0)).await?;
    for case_id in &about {
        conn.execute("DELETE FROM mod_reports WHERE case_id = ?1", [case_id.as_str()]).await?;
        conn.execute("DELETE FROM mod_cases WHERE id = ?1", [case_id.as_str()]).await?;
    }
    conn.execute("DELETE FROM mod_notes WHERE subject_id = ?1", [account_id]).await?;
    let reported =
        query_all(conn, "SELECT case_id FROM mod_reports WHERE reporter_id = ?1", [account_id], |r| r.get::<String>(0))
            .await?;
    conn.execute("DELETE FROM mod_reports WHERE reporter_id = ?1", [account_id]).await?;
    for case_id in &reported {
        conn.execute(
            "UPDATE mod_cases SET report_count = report_count - 1 WHERE id = ?1 AND report_count > 0",
            [case_id.as_str()],
        )
        .await?;
        conn.execute(
            "DELETE FROM mod_cases WHERE id = ?1 AND report_count = 0 AND kind != ?2",
            (case_id.as_str(), Kind::AutoMod as i64),
        )
        .await?;
    }
    for case_id in &reported {
        if let Some(case) = load_case(conn, server_id, case_id).await? {
            events.push(Payload::ModCaseUpdated(pb::ModCaseUpdated { mod_case: Some(case) }));
        }
    }
    Ok(())
}

/// Cases matching a request, newest first, that `access` can see: a page of
/// at most `limit`, and whether there are more.
async fn page_cases(
    conn: &turso::Connection,
    server_id: &str,
    access: &Access,
    req: &pb::ListModCasesRequest,
    limit: usize,
) -> Result<(Vec<pb::ModCase>, bool)> {
    let mut cases = Vec::new();
    let mut cursor = req.before_id.clone();
    loop {
        let rows = query_all(
            conn,
            &format!(
                "SELECT {CASE_COLUMNS} FROM mod_cases
                 WHERE (?1 = '' OR id < ?1) AND (?2 = 0 OR status = ?2) AND (?3 = '' OR subject_id = ?3)
                   AND (?4 = 0 OR kind = ?4)
                 ORDER BY id DESC LIMIT ?5"
            ),
            (cursor.as_str(), i64::from(req.status), req.subject_id.as_str(), i64::from(req.kind), BATCH),
            case_row(server_id),
        )
        .await?;
        let full = rows.len() as i64 == BATCH;
        for row in rows {
            let case = with_alert(row)?;
            cursor.clone_from(&case.id);
            if !shown(access, &case) {
                continue;
            }
            if cases.len() == limit {
                return Ok((cases, true));
            }
            cases.push(case);
        }
        if !full {
            return Ok((cases, false));
        }
    }
}

/// How many cases are open and in all about someone (or everyone, when
/// `subject_id` is empty), in channels `access` can see.
async fn count_cases(conn: &turso::Connection, access: &Access, subject_id: &str) -> Result<(i32, i32)> {
    let rows = query_all(
        conn,
        "SELECT channel_id, status, COUNT(*) FROM mod_cases WHERE (?1 = '' OR subject_id = ?1)
         GROUP BY channel_id, status",
        [subject_id],
        |r| Ok((r.get::<String>(0)?, r.get::<i32>(1)?, r.get::<i64>(2)?)),
    )
    .await?;
    let (mut open, mut total) = (0, 0);
    for (channel_id, status, count) in rows {
        if !channel_id.is_empty() && !access.can_see(&channel_id) {
            continue;
        }
        let count = i32::try_from(count).unwrap_or(i32::MAX);
        total += count;
        if status == CaseStatus::Open as i32 {
            open += count;
        }
    }
    Ok((open, total))
}

/// The messages `author_id` wrote, newest first, in channels `access` can
/// see (or only `channel_id`): a page of at most `limit`, and whether there
/// are more.
async fn page_messages(
    conn: &turso::Connection,
    server_id: &str,
    access: &Access,
    req: &pb::ListMemberMessagesRequest,
    limit: usize,
) -> Result<(Vec<pb::Message>, bool)> {
    if !req.channel_id.is_empty() && !access.can_see(&req.channel_id) {
        return Err(Error::NotFound("channel"));
    }
    let mut messages = Vec::new();
    let mut cursor = req.before_id.clone();
    loop {
        let rows = query_all(
            conn,
            &format!(
                "SELECT {MESSAGE_COLUMNS} FROM messages
                 WHERE author_id = ?1 AND kind = 0 AND (?2 = '' OR id < ?2) AND (?3 = '' OR channel_id = ?3)
                 ORDER BY id DESC LIMIT ?4"
            ),
            (req.user_id.as_str(), cursor.as_str(), req.channel_id.as_str(), BATCH),
            message_row(server_id),
        )
        .await?;
        let full = rows.len() as i64 == BATCH;
        for row in rows {
            cursor.clone_from(&row.0.id);
            if !access.can_see(&row.0.channel_id) {
                continue;
            }
            if messages.len() == limit {
                return Ok((messages, true));
            }
            messages.push(with_extras(row)?);
        }
        if !full {
            return Ok((messages, false));
        }
    }
}

/// People the server knows (members, and anyone it kept a profile of: who
/// left, was banned, applied or wrote in a channel it shares) whose name,
/// username or nickname holds `query`, or whose id it is: an exact username
/// or id first, then members, then names that start with it. `query` is
/// lowercase; folding is SQLite's `lower`, ASCII only.
async fn find_people(conn: &turso::Connection, query: &str, id: &str, limit: i64) -> Result<Vec<pb::ModPerson>> {
    query_all(
        conn,
        &format!(
            "SELECT {USER_COLUMNS}, members.user_id IS NOT NULL, COALESCE(members.nickname, ''), bans.user_id IS NOT NULL
             FROM users LEFT JOIN members ON members.user_id = users.id LEFT JOIN bans ON bans.user_id = users.id
             WHERE users.id = ?2 OR instr(lower(users.username), ?1) > 0 OR instr(lower(users.display_name), ?1) > 0
               OR instr(lower(COALESCE(members.nickname, '')), ?1) > 0
             ORDER BY users.id = ?2 OR lower(users.username) = ?1 DESC, members.user_id IS NULL,
               instr(lower(users.username), ?1) = 1 OR instr(lower(users.display_name), ?1) = 1
                 OR instr(lower(COALESCE(members.nickname, '')), ?1) = 1 DESC,
               lower(users.display_name), users.id
             LIMIT ?3"
        ),
        (query, id, limit),
        |r| {
            Ok(pb::ModPerson {
                user: Some(store::user_row(r)?),
                member: r.get::<i64>(8)? != 0,
                nickname: r.get(9)?,
                banned: r.get::<i64>(10)? != 0,
            })
        },
    )
    .await
}

fn limit_of(asked: i32) -> usize {
    if asked <= 0 { 50 } else { asked.min(100) as usize }
}

/// A channel that can have reports: the server's own, not secure.
async fn reportable(conn: &turso::Connection, server_id: &str, channel_id: &str) -> Result<pb::Channel> {
    let channel = load_channel(conn, server_id, channel_id).await?.ok_or(Error::NotFound("message"))?;
    if channel.r#type == pb::ChannelType::Secure as i32 {
        return Err(Error::invalid("messages in secure channels are end-to-end encrypted, so they can't be reported"));
    }
    if shared::link_of(conn, &channel.id).await?.is_some() {
        return Err(Error::invalid("messages in a channel shared from another server can't be reported yet"));
    }
    Ok(channel)
}

#[tonic::async_trait]
impl ModerationService for Api {
    async fn report_message(
        &self,
        request: Request<pb::ReportMessageRequest>,
    ) -> Result<Response<pb::ReportMessageResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                let reason = reason_of(req.reason)?;
                let note = text("a report's note", &req.note, 0, MAX_REASON)?;
                if !access.can_see(&req.channel_id) {
                    return Err(Error::NotFound("message"));
                }
                sdb.write(&account.id, async |conn, events| {
                    let channel = reportable(conn, &sdb.id, &req.channel_id).await?;
                    let message = load_message(conn, &sdb.id, &req.message_id)
                        .await?
                        .filter(|m| m.channel_id == channel.id)
                        .ok_or(Error::NotFound("message"))?;
                    if message.kind != pb::MessageKind::Unspecified as i32 {
                        return Err(Error::invalid("only messages people wrote can be reported"));
                    }
                    if message.author_id == account.id {
                        return Err(Error::invalid("you can't report your own message"));
                    }
                    let now = Some(timestamp(now_ms()));
                    let fresh = pb::ModCase {
                        id: new_id(),
                        server_id: sdb.id.clone(),
                        kind: Kind::MessageReport as i32,
                        status: CaseStatus::Open as i32,
                        subject_id: message.author_id.clone(),
                        channel_id: channel.id.clone(),
                        message_id: message.id.clone(),
                        thread_id: message.thread_id.clone(),
                        content: reviewed_text(&message).into_owned(),
                        created_at: now,
                        updated_at: now,
                        ..Default::default()
                    };
                    let key = format!("m:{}", message.id);
                    report(conn, &sdb.id, &key, fresh, &account.id, reason, &note, events).await
                })
                .await?;
                Ok(pb::ReportMessageResponse {})
            }
            .await,
        )
    }

    async fn report_member(
        &self,
        request: Request<pb::ReportMemberRequest>,
    ) -> Result<Response<pb::ReportMemberResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, .. } = self.membership(&account, &req.server_id).await?;
                let reason = reason_of(req.reason)?;
                let note = text("a report's note", &req.note, 0, MAX_REASON)?;
                if req.user_id == account.id {
                    return Err(Error::invalid("you can't report yourself"));
                }
                sdb.write(&account.id, async |conn, events| {
                    store::user(conn, &req.user_id).await?.ok_or(Error::NotFound("member"))?;
                    let now = Some(timestamp(now_ms()));
                    let fresh = pb::ModCase {
                        id: new_id(),
                        server_id: sdb.id.clone(),
                        kind: Kind::MemberReport as i32,
                        status: CaseStatus::Open as i32,
                        subject_id: req.user_id.clone(),
                        created_at: now,
                        updated_at: now,
                        ..Default::default()
                    };
                    let key = format!("u:{}", req.user_id);
                    report(conn, &sdb.id, &key, fresh, &account.id, reason, &note, events).await
                })
                .await?;
                Ok(pb::ReportMemberResponse {})
            }
            .await,
        )
    }

    async fn list_mod_cases(
        &self,
        request: Request<pb::ListModCasesRequest>,
    ) -> Result<Response<pb::ListModCasesResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::Moderate).await?;
                let conn = sdb.read()?;
                let (mut cases, has_more) = page_cases(&conn, &sdb.id, &access, &req, limit_of(req.limit)).await?;
                add_reasons(&conn, &mut cases).await?;
                let (open_count, _) = count_cases(&conn, &access, "").await?;
                let ids: Vec<&str> =
                    cases.iter().flat_map(|c| [c.subject_id.as_str(), c.closed_by_id.as_str()]).collect();
                let users = users(&conn, &ids).await?;
                Ok(pb::ListModCasesResponse { cases, users, has_more, open_count })
            }
            .await,
        )
    }

    async fn get_mod_case(
        &self,
        request: Request<pb::GetModCaseRequest>,
    ) -> Result<Response<pb::GetModCaseResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::Moderate).await?;
                let conn = sdb.read()?;
                let case = load_case(&conn, &sdb.id, &req.case_id)
                    .await?
                    .filter(|c| shown(&access, c))
                    .ok_or(Error::NotFound("case"))?;
                let reports = query_all(
                    &conn,
                    "SELECT reporter_id, reason, note, created_at FROM mod_reports WHERE case_id = ?1
                     ORDER BY created_at, reporter_id",
                    [case.id.as_str()],
                    |r| {
                        Ok(pb::ModReport {
                            reporter_id: r.get(0)?,
                            reason: r.get(1)?,
                            note: r.get(2)?,
                            created_at: Some(timestamp(r.get(3)?)),
                        })
                    },
                )
                .await?;
                let mut message = match case.message_id.as_str() {
                    "" => None,
                    id => load_message(&conn, &sdb.id, id).await?.filter(|m| access.can_see(&m.channel_id)),
                };
                if let Some(message) = message.as_mut() {
                    reactions::attach(&conn, &account.id, std::slice::from_mut(message)).await?;
                    threads::attach(&conn, std::slice::from_mut(message)).await?;
                }
                let mut ids = vec![case.subject_id.as_str(), case.closed_by_id.as_str()];
                ids.extend(reports.iter().map(|r| r.reporter_id.as_str()));
                let users = users(&conn, &ids).await?;
                Ok(pb::GetModCaseResponse { mod_case: Some(case), reports, message, users })
            }
            .await,
        )
    }

    async fn update_mod_case(
        &self,
        request: Request<pb::UpdateModCaseRequest>,
    ) -> Result<Response<pb::UpdateModCaseResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::Moderate).await?;
                let status = match CaseStatus::try_from(req.status) {
                    Ok(s @ (CaseStatus::Open | CaseStatus::Resolved | CaseStatus::Dismissed)) => s,
                    _ => return Err(Error::invalid("a case is open, resolved or dismissed")),
                };
                let resolution = text("the reason", &req.resolution, 0, MAX_REASON)?;
                let case = sdb
                    .write(&account.id, async |conn, events| {
                        let before = load_case(conn, &sdb.id, &req.case_id)
                            .await?
                            .filter(|c| shown(&access, c))
                            .ok_or(Error::NotFound("case"))?;
                        let now = now_ms();
                        if status == CaseStatus::Open {
                            if before.status == CaseStatus::Open as i32 {
                                return Ok(before);
                            }
                            conn.execute(
                                "UPDATE mod_cases SET status = ?2, closed_by = '', closed_at = NULL, resolution = '',
                                 updated_at = ?3 WHERE id = ?1",
                                (before.id.as_str(), status as i64, now),
                            )
                            .await?;
                        } else {
                            conn.execute(
                                "UPDATE mod_cases SET status = ?2, open_key = NULL, closed_by = ?3, closed_at = ?4,
                                 resolution = ?5, updated_at = ?4 WHERE id = ?1",
                                (before.id.as_str(), status as i64, account.id.as_str(), now, resolution.as_str()),
                            )
                            .await?;
                        }
                        let mut entry = Audit::new(pb::AuditAction::ModCaseUpdate, &before.subject_id)
                            .reason(&resolution)
                            .change("status", before.status, status as i32);
                        if let Some(channel) = load_channel(conn, &sdb.id, &before.channel_id).await? {
                            entry = entry.channel(channel.name);
                        }
                        store::audit(conn, &account.id, entry).await?;
                        let case = load_case(conn, &sdb.id, &before.id).await?.ok_or(Error::NotFound("case"))?;
                        events.push(Payload::ModCaseUpdated(pb::ModCaseUpdated { mod_case: Some(case.clone()) }));
                        Ok(case)
                    })
                    .await?;
                Ok(pb::UpdateModCaseResponse { mod_case: Some(case) })
            }
            .await,
        )
    }

    async fn get_member_overview(
        &self,
        request: Request<pb::GetMemberOverviewRequest>,
    ) -> Result<Response<pb::GetMemberOverviewResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::Moderate).await?;
                let conn = sdb.read()?;
                let user = store::user(&conn, &req.user_id).await?.ok_or(Error::NotFound("member"))?;
                let mut member = store::member(&conn, &sdb.id, &req.user_id).await?;
                if let Some(member) = member.as_mut() {
                    store::scrub_sso(member, &account.id, access.has(Permission::ManageServer));
                }
                let account_created_at = ulid::Ulid::from_string(&req.user_id)
                    .ok()
                    .map(|id| timestamp(i64::try_from(id.timestamp_ms()).unwrap_or_default()));
                let ban = query_one(
                    &conn,
                    "SELECT reason, banned_by, created_at FROM bans WHERE user_id = ?1",
                    [req.user_id.as_str()],
                    |r| {
                        Ok(pb::ModOverviewBan {
                            reason: r.get(0)?,
                            banned_by_id: r.get(1)?,
                            created_at: Some(timestamp(r.get(2)?)),
                        })
                    },
                )
                .await?;
                let (open_cases, total_cases) = count_cases(&conn, &access, &req.user_id).await?;
                let actions = HISTORY_ACTIONS.iter().map(|a| (*a as i32).to_string()).collect::<Vec<_>>().join(", ");
                let rows = query_all(
                    &conn,
                    &format!(
                        "SELECT id, actor_id, action, target_id, channel_name, reason, changes, created_at, role_name
                         FROM audit WHERE target_id = ?1 AND action IN ({actions}) ORDER BY id DESC LIMIT ?2"
                    ),
                    (req.user_id.as_str(), HISTORY),
                    |r| {
                        Ok((
                            pb::AuditEntry {
                                id: r.get(0)?,
                                actor_id: r.get(1)?,
                                action: r.get(2)?,
                                target_id: r.get(3)?,
                                channel_name: r.get(4)?,
                                reason: r.get(5)?,
                                changes: vec![],
                                created_at: Some(timestamp(r.get(7)?)),
                                role_name: r.get(8)?,
                            },
                            r.get::<Option<Vec<u8>>>(6)?,
                        ))
                    },
                )
                .await?;
                // The log keeps a channel's name, not its id: one the caller
                // can't see by that name isn't named.
                let seen: HashSet<String> = load_channels(&conn, &sdb.id)
                    .await?
                    .into_iter()
                    .filter(|c| access.can_see(&c.id))
                    .map(|c| c.name)
                    .collect();
                let history = rows
                    .into_iter()
                    .map(|(mut entry, changes)| {
                        entry.changes = store::audit_changes(changes)?;
                        if !seen.contains(&entry.channel_name) {
                            entry.channel_name.clear();
                        }
                        Ok(entry)
                    })
                    .collect::<Result<Vec<_>>>()?;
                let notes = query_all(
                    &conn,
                    "SELECT id, subject_id, author_id, content, created_at FROM mod_notes WHERE subject_id = ?1
                     ORDER BY id DESC LIMIT ?2",
                    (req.user_id.as_str(), NOTES),
                    note_row,
                )
                .await?;
                let counts = query_all(
                    &conn,
                    "SELECT channel_id, COUNT(*) FROM messages WHERE author_id = ?1 AND kind = 0 GROUP BY channel_id",
                    [req.user_id.as_str()],
                    |r| Ok((r.get::<String>(0)?, r.get::<i64>(1)?)),
                )
                .await?;
                let message_count = counts.iter().filter(|(c, _)| access.can_see(c)).map(|(_, n)| n).sum();
                let mut ids: Vec<&str> = history.iter().map(|e| e.actor_id.as_str()).collect();
                ids.extend(notes.iter().map(|n| n.author_id.as_str()));
                ids.extend(ban.iter().map(|b| b.banned_by_id.as_str()));
                let users = users(&conn, &ids).await?;
                Ok(pb::GetMemberOverviewResponse {
                    user: Some(user),
                    member,
                    account_created_at,
                    ban,
                    open_cases,
                    total_cases,
                    history,
                    notes,
                    message_count,
                    users,
                })
            }
            .await,
        )
    }

    async fn list_member_messages(
        &self,
        request: Request<pb::ListMemberMessagesRequest>,
    ) -> Result<Response<pb::ListMemberMessagesResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::Moderate).await?;
                let conn = sdb.read()?;
                let (mut messages, has_more) =
                    page_messages(&conn, &sdb.id, &access, &req, limit_of(req.limit)).await?;
                polls::attach(&conn, &mut messages).await?;
                shared::mark_guests(&conn, &mut messages).await?;
                reactions::attach(&conn, &account.id, &mut messages).await?;
                threads::attach(&conn, &mut messages).await?;
                let authors = users(&conn, &[req.user_id.as_str()]).await?;
                Ok(pb::ListMemberMessagesResponse { messages, has_more, authors })
            }
            .await,
        )
    }

    async fn find_people(
        &self,
        request: Request<pb::FindPeopleRequest>,
    ) -> Result<Response<pb::FindPeopleResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, .. } = self.with(&account, &req.server_id, Permission::Moderate).await?;
                let query = text("a search", req.query.trim().trim_start_matches('@'), 1, MAX_QUERY)?;
                let limit = if req.limit <= 0 { 20 } else { req.limit.min(50) };
                let conn = sdb.read()?;
                let people = find_people(&conn, &query.to_lowercase(), &query, limit.into()).await?;
                Ok(pb::FindPeopleResponse { people })
            }
            .await,
        )
    }

    async fn add_mod_note(
        &self,
        request: Request<pb::AddModNoteRequest>,
    ) -> Result<Response<pb::AddModNoteResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, .. } = self.with(&account, &req.server_id, Permission::Moderate).await?;
                let content = text("a note", &req.content, 1, MAX_NOTE)?;
                let note = sdb
                    .write(&account.id, async |conn, events| {
                        store::user(conn, &req.user_id).await?.ok_or(Error::NotFound("member"))?;
                        let note = pb::ModNote {
                            id: new_id(),
                            subject_id: req.user_id.clone(),
                            author_id: account.id.clone(),
                            content: content.clone(),
                            created_at: Some(timestamp(now_ms())),
                        };
                        conn.execute(
                            "INSERT INTO mod_notes (id, subject_id, author_id, content, created_at)
                             VALUES (?1, ?2, ?3, ?4, ?5)",
                            (
                                note.id.as_str(),
                                note.subject_id.as_str(),
                                note.author_id.as_str(),
                                note.content.as_str(),
                                note.created_at.as_ref().map_or(0, crate::id::millis),
                            ),
                        )
                        .await?;
                        store::audit(conn, &account.id, Audit::new(pb::AuditAction::ModNoteAdd, &req.user_id)).await?;
                        events.push(Payload::ModNoteAdded(pb::ModNoteAdded { note: Some(note.clone()) }));
                        Ok(note)
                    })
                    .await?;
                Ok(pb::AddModNoteResponse { note: Some(note) })
            }
            .await,
        )
    }

    async fn delete_mod_note(
        &self,
        request: Request<pb::DeleteModNoteRequest>,
    ) -> Result<Response<pb::DeleteModNoteResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::Moderate).await?;
                sdb.write(&account.id, async |conn, events| {
                    let note = query_one(
                        conn,
                        "SELECT id, subject_id, author_id, content, created_at FROM mod_notes WHERE id = ?1",
                        [req.note_id.as_str()],
                        note_row,
                    )
                    .await?
                    .ok_or(Error::NotFound("note"))?;
                    if note.author_id != account.id && !access.has(Permission::ManageServer) {
                        return Err(Error::denied("you can delete only your own notes"));
                    }
                    conn.execute("DELETE FROM mod_notes WHERE id = ?1", [note.id.as_str()]).await?;
                    store::audit(conn, &account.id, Audit::new(pb::AuditAction::ModNoteDelete, &note.subject_id))
                        .await?;
                    events.push(Payload::ModNoteDeleted(pb::ModNoteDeleted {
                        subject_id: note.subject_id,
                        note_id: note.id,
                    }));
                    Ok(())
                })
                .await?;
                Ok(pb::DeleteModNoteResponse {})
            }
            .await,
        )
    }
}

fn note_row(r: &turso::Row) -> turso::Result<pb::ModNote> {
    Ok(pb::ModNote {
        id: r.get(0)?,
        subject_id: r.get(1)?,
        author_id: r.get(2)?,
        content: r.get(3)?,
        created_at: Some(timestamp(r.get(4)?)),
    })
}
