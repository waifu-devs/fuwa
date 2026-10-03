//! A server's AutoMod rules (`AutoModService`), and acting on what they catch
//! when a message is sent or edited.

use prost::Message as _;
use tonic::{Request, Response, Status};

use super::{Api, respond, text};
use crate::automod;
use crate::error::{Error, Result};
use crate::id::{millis, new_id, now_ms, timestamp};
use crate::pb::{
    self, AutoModActionKind as Kind, AutoModTrigger as Trigger, Permission, auto_mod_service_server::AutoModService,
};
use crate::permissions::Access;
use crate::servers::{self as store, Audit, Payload, UsageChange, load_channel};

/// As many of each kind as Discord allows.
const MAX_KEYWORD_RULES: usize = 6;
const MAX_KEYWORDS: usize = 1000;
const MAX_KEYWORD: usize = 60;
const MAX_ALLOWED: usize = 100;
const MAX_EXEMPT_ROLES: usize = 20;
const MAX_EXEMPT_CHANNELS: usize = 50;
const MAX_BLOCK_MESSAGE: usize = 150;
const MAX_MENTION_LIMIT: i32 = 50;
const MIN_TIME_OUT: i32 = 60;
const MAX_TIME_OUT: i32 = 28 * 24 * 60 * 60;

/// What a rule is called when nobody names it.
fn default_name(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Keywords => "Blocked words",
        Trigger::MentionSpam => "Mention spam",
        Trigger::Links => "Links",
        Trigger::Unspecified => "Rule",
    }
}

/// A list as it may be saved: trimmed, lowercase, once each, nothing blank.
fn checked_list(what: &str, list: &[String], max_items: usize, max_len: usize) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for item in list {
        let item = item.trim().to_lowercase();
        if item.is_empty() || out.contains(&item) {
            continue;
        }
        if item.chars().count() > max_len {
            return Err(Error::invalid(format!("{what} can be at most {max_len} characters each")));
        }
        out.push(item);
    }
    if out.len() > max_items {
        return Err(Error::invalid(format!("a rule can have up to {max_items} {what}")));
    }
    Ok(out)
}

/// A rule as it may be saved, checked against the server it's for. Fields
/// that don't belong to its trigger are cleared.
async fn checked_rule(conn: &turso::Connection, server_id: &str, rule: pb::AutoModRule) -> Result<pb::AutoModRule> {
    let trigger = match Trigger::try_from(rule.trigger) {
        Ok(trigger @ (Trigger::Keywords | Trigger::MentionSpam | Trigger::Links)) => trigger,
        _ => return Err(Error::invalid("a rule looks for keywords, mention spam or links")),
    };
    let name = match rule.name.trim() {
        "" => default_name(trigger).to_string(),
        name => text("a rule's name", name, 1, 100)?,
    };
    let (keywords, allowed, mention_limit) = match trigger {
        Trigger::Keywords => {
            let keywords = checked_list("keywords", &rule.keywords, MAX_KEYWORDS, MAX_KEYWORD)?;
            if keywords.iter().all(|k| k.trim_matches('*').is_empty()) {
                return Err(Error::invalid("add a word or phrase for the rule to look for"));
            }
            (keywords, checked_list("allowed words", &rule.allowed, MAX_ALLOWED, MAX_KEYWORD)?, 0)
        }
        Trigger::MentionSpam => {
            if !(1..=MAX_MENTION_LIMIT).contains(&rule.mention_limit) {
                return Err(Error::invalid(format!("the ping limit is 1 to {MAX_MENTION_LIMIT}")));
            }
            (vec![], vec![], rule.mention_limit)
        }
        _ => {
            let sites: Vec<String> = rule.allowed.iter().map(|s| automod::site(s)).collect();
            if sites.iter().any(|s| !s.is_empty() && !s.contains('.')) {
                return Err(Error::invalid("allowed sites look like example.com"));
            }
            (vec![], checked_list("allowed sites", &sites, MAX_ALLOWED, 253)?, 0)
        }
    };
    if rule.actions.is_empty() {
        return Err(Error::invalid("a rule needs something to do: block, alert or time out"));
    }
    let mut actions: Vec<pb::AutoModAction> = Vec::new();
    for action in rule.actions {
        let kind = Kind::try_from(action.kind).unwrap_or(Kind::Unspecified);
        if actions.iter().any(|a| a.kind == action.kind) {
            return Err(Error::invalid("a rule does each thing once"));
        }
        actions.push(match kind {
            Kind::Block => pb::AutoModAction {
                kind: action.kind,
                message: text("the blocked message's note", &action.message, 0, MAX_BLOCK_MESSAGE)?,
                ..Default::default()
            },
            Kind::Alert => {
                let channel = load_channel(conn, server_id, &action.channel_id)
                    .await?
                    .ok_or_else(|| Error::invalid("pick a channel for alerts"))?;
                if !matches!(
                    pb::ChannelType::try_from(channel.r#type),
                    Ok(pb::ChannelType::Text | pb::ChannelType::Announcement)
                ) {
                    return Err(Error::invalid("alerts go in a text channel"));
                }
                if channel.shared.as_ref().is_some_and(|s| !s.home) {
                    return Err(Error::invalid("alerts can't go in a channel shared from another server"));
                }
                pb::AutoModAction { kind: action.kind, channel_id: channel.id, ..Default::default() }
            }
            Kind::TimeOut => {
                if !(MIN_TIME_OUT..=MAX_TIME_OUT).contains(&action.duration_seconds) {
                    return Err(Error::invalid("time-outs last a minute to 28 days"));
                }
                pb::AutoModAction { kind: action.kind, duration_seconds: action.duration_seconds, ..Default::default() }
            }
            Kind::Unspecified => return Err(Error::invalid("a rule blocks, alerts or times out")),
        });
    }
    let roles = crate::permissions::roles(conn, server_id).await?;
    let mut exempt_role_ids: Vec<String> = Vec::new();
    for id in &rule.exempt_role_ids {
        let role = roles.iter().find(|r| &r.id == id && r.id != server_id).ok_or(Error::NotFound("role"))?;
        if !exempt_role_ids.contains(&role.id) {
            exempt_role_ids.push(role.id.clone());
        }
    }
    let mut exempt_channel_ids: Vec<String> = Vec::new();
    for id in &rule.exempt_channel_ids {
        let channel = load_channel(conn, server_id, id).await?.ok_or(Error::NotFound("channel"))?;
        if !exempt_channel_ids.contains(&channel.id) {
            exempt_channel_ids.push(channel.id);
        }
    }
    if exempt_role_ids.len() > MAX_EXEMPT_ROLES || exempt_channel_ids.len() > MAX_EXEMPT_CHANNELS {
        return Err(Error::invalid(format!(
            "a rule leaves out up to {MAX_EXEMPT_ROLES} roles and {MAX_EXEMPT_CHANNELS} channels"
        )));
    }
    Ok(pb::AutoModRule {
        id: rule.id,
        server_id: server_id.to_string(),
        name,
        enabled: rule.enabled,
        trigger: trigger as i32,
        keywords,
        allowed,
        mention_limit,
        actions,
        exempt_role_ids,
        exempt_channel_ids,
        creator_id: String::new(),
        created_at: None,
        updated_at: None,
    })
}

/// How many of a server's rules look for `trigger`, besides the rule `except`.
fn count(rules: &[pb::AutoModRule], trigger: Trigger, except: &str) -> usize {
    rules.iter().filter(|r| r.trigger == trigger as i32 && r.id != except).count()
}

/// What AutoMod did about a message.
#[derive(Debug, Default)]
pub(super) struct Verdict {
    /// Why it wasn't sent, for its author; `None` when it was.
    pub blocked: Option<String>,
}

/// Runs a server's rules over a message about to be sent or saved, inside
/// that write: posts alerts and times its author out as the rules say. The
/// caller doesn't save the message when it comes back blocked.
pub(super) async fn review(
    conn: &turso::Connection,
    server_id: &str,
    member: &pb::Member,
    access: &Access,
    channel: &pb::Channel,
    content: &str,
    events: &mut Vec<Payload>,
) -> Result<Verdict> {
    // Managers and administrators are trusted, as on Discord.
    if access.has(Permission::ManageServer) || content.trim().is_empty() {
        return Ok(Verdict::default());
    }
    let rules = store::load_automod(conn).await?;
    let author_id = member.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
    let mut caught: Vec<(pb::AutoModRule, automod::Hit)> = Vec::new();
    for rule in rules {
        let exempt = !rule.enabled
            || rule.exempt_channel_ids.iter().any(|id| *id == channel.id || *id == channel.parent_id)
            || rule.exempt_role_ids.iter().any(|id| member.role_ids.contains(id));
        if exempt {
            continue;
        }
        if let Some(hit) = automod::check(&rule, content) {
            caught.push((rule, hit));
        }
    }
    if caught.is_empty() {
        return Ok(Verdict::default());
    }
    let action = |rule: &pb::AutoModRule, kind: Kind| rule.actions.iter().find(|a| a.kind == kind as i32).cloned();
    let blocked = caught.iter().find_map(|(rule, _)| action(rule, Kind::Block)).map(|block| {
        if block.message.is_empty() {
            "AutoMod: this server doesn't allow that message".to_string()
        } else {
            format!("AutoMod: {}", block.message)
        }
    });
    let now = now_ms();
    let time_out = caught.iter().filter_map(|(rule, _)| action(rule, Kind::TimeOut)).map(|a| a.duration_seconds).max();
    if let Some(seconds) = time_out {
        let until = now + i64::from(seconds) * 1000;
        let current = member.timed_out_until.as_ref().map_or(0, millis);
        if until > current {
            conn.execute("UPDATE members SET timed_out_until = ?2 WHERE user_id = ?1", (author_id.as_str(), until))
                .await?;
            let mut updated = member.clone();
            updated.timed_out_until = Some(timestamp(until));
            events.push(Payload::MemberUpdated(pb::MemberUpdated { member: Some(updated) }));
            let rule = caught.iter().find(|(r, _)| action(r, Kind::TimeOut).is_some()).map(|(r, _)| r.name.as_str());
            let entry = Audit::new(pb::AuditAction::AutoModTimeOut, &author_id)
                .channel(channel.name.clone())
                .reason(rule.unwrap_or_default())
                .change("timed_out_until", current, until);
            store::audit(conn, &author_id, entry).await?;
        }
    }
    for (rule, hit) in &caught {
        let Some(alert) = action(rule, Kind::Alert) else { continue };
        let Some(alert_channel) = load_channel(conn, server_id, &alert.channel_id).await? else { continue };
        let message = pb::Message {
            id: new_id(),
            server_id: server_id.to_string(),
            channel_id: alert_channel.id.clone(),
            author_id: author_id.clone(),
            created_at: Some(timestamp(now)),
            kind: pb::MessageKind::AutoModAlert as i32,
            auto_mod: Some(pb::AutoModAlert {
                rule_id: rule.id.clone(),
                rule_name: rule.name.clone(),
                trigger: rule.trigger,
                channel_id: channel.id.clone(),
                content: content.to_string(),
                matched: hit.matched.clone(),
                blocked: blocked.is_some(),
                timed_out_seconds: action(rule, Kind::TimeOut).map_or(0, |a| a.duration_seconds),
            }),
            ..Default::default()
        };
        super::messages::insert_system(conn, &message).await?;
        store::add_usage(conn, UsageChange { messages: 1, messages_sent: 1, ..Default::default() }).await?;
        events.push(Payload::MessageCreated(pb::MessageCreated { message: Some(message) }));
    }
    Ok(Verdict { blocked })
}

#[tonic::async_trait]
impl AutoModService for Api {
    async fn list_auto_mod_rules(
        &self,
        request: Request<pb::ListAutoModRulesRequest>,
    ) -> Result<Response<pb::ListAutoModRulesResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.with(&account, &request.get_ref().server_id, Permission::ManageServer).await?.sdb;
                Ok(pb::ListAutoModRulesResponse { rules: store::load_automod(&sdb.read()?).await? })
            }
            .await,
        )
    }

    async fn save_auto_mod_rule(
        &self,
        request: Request<pb::SaveAutoModRuleRequest>,
    ) -> Result<Response<pb::SaveAutoModRuleResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageServer).await?.sdb;
                let draft = req.rule.unwrap_or_default();
                let rule = sdb
                    .write(&account.id, async |conn, _events| {
                        let mut rule = checked_rule(conn, &sdb.id, draft.clone()).await?;
                        let rules = store::load_automod(conn).await?;
                        let before = rules.iter().find(|r| r.id == rule.id).cloned();
                        if !rule.id.is_empty() && before.is_none() {
                            return Err(Error::NotFound("rule"));
                        }
                        let trigger = Trigger::try_from(rule.trigger).unwrap_or(Trigger::Unspecified);
                        let room = if trigger == Trigger::Keywords { MAX_KEYWORD_RULES } else { 1 };
                        if count(&rules, trigger, &rule.id) >= room {
                            return Err(Error::FailedPrecondition(match trigger {
                                Trigger::Keywords => {
                                    format!("a server can have up to {MAX_KEYWORD_RULES} keyword rules")
                                }
                                _ => "a server has one rule of this kind; change that one instead".into(),
                            }));
                        }
                        let now = now_ms();
                        let created_at = before.as_ref().and_then(|b| b.created_at).unwrap_or(timestamp(now));
                        rule.updated_at = Some(timestamp(now));
                        rule.created_at = Some(created_at);
                        rule.creator_id = before.as_ref().map_or_else(|| account.id.clone(), |b| b.creator_id.clone());
                        let action = match &before {
                            Some(_) => pb::AuditAction::AutoModRuleUpdate,
                            None => {
                                rule.id = new_id();
                                pb::AuditAction::AutoModRuleCreate
                            }
                        };
                        conn.execute(
                            "INSERT INTO automod_rules (id, rule, created_at) VALUES (?1, ?2, ?3)
                             ON CONFLICT (id) DO UPDATE SET rule = excluded.rule",
                            (rule.id.as_str(), rule.encode_to_vec(), millis(&created_at)),
                        )
                        .await?;
                        // Keywords stay between managers: the entry says what changed, never which words.
                        let mut entry = Audit::new(action, &rule.id).change(
                            "name",
                            before.as_ref().map(|b| b.name.as_str()).unwrap_or_default(),
                            &rule.name,
                        );
                        if let Some(before) = &before {
                            entry = entry
                                .change("enabled", before.enabled, rule.enabled)
                                .change("keywords", before.keywords.len(), rule.keywords.len())
                                .change("allowed", before.allowed.len(), rule.allowed.len())
                                .change("mention_limit", before.mention_limit, rule.mention_limit)
                                .change("actions", before.actions.len(), rule.actions.len());
                        }
                        store::audit(conn, &account.id, entry).await?;
                        Ok(rule)
                    })
                    .await?;
                Ok(pb::SaveAutoModRuleResponse { rule: Some(rule) })
            }
            .await,
        )
    }

    async fn delete_auto_mod_rule(
        &self,
        request: Request<pb::DeleteAutoModRuleRequest>,
    ) -> Result<Response<pb::DeleteAutoModRuleResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageServer).await?.sdb;
                sdb.write(&account.id, async |conn, _events| {
                    let rules = store::load_automod(conn).await?;
                    let rule = rules.iter().find(|r| r.id == req.rule_id).ok_or(Error::NotFound("rule"))?;
                    conn.execute("DELETE FROM automod_rules WHERE id = ?1", [rule.id.as_str()]).await?;
                    let entry = Audit::new(pb::AuditAction::AutoModRuleDelete, &rule.id).change("name", &rule.name, "");
                    store::audit(conn, &account.id, entry).await?;
                    Ok(())
                })
                .await?;
                Ok(pb::DeleteAutoModRuleResponse {})
            }
            .await,
        )
    }

    async fn test_auto_mod_rule(
        &self,
        request: Request<pb::TestAutoModRuleRequest>,
    ) -> Result<Response<pb::TestAutoModRuleResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                self.with(&account, &req.server_id, Permission::ManageServer).await?;
                if req.content.chars().count() > super::messages::MAX_MESSAGE_LENGTH {
                    return Err(Error::invalid("that's longer than a message can be"));
                }
                let rule = req.rule.unwrap_or_default();
                let hit = automod::check(&rule, &req.content);
                Ok(pb::TestAutoModRuleResponse {
                    matched: hit.is_some(),
                    matches: hit.map(|h| h.matched).unwrap_or_default(),
                })
            }
            .await,
        )
    }
}
