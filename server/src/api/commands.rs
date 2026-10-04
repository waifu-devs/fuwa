//! Slash commands and buttons: how people use agents (docs/commands.md).
//!
//! An agent sets its commands per server; running one, or pressing a button
//! on one of its messages, makes an interaction that only that agent hears
//! about (InteractionCreated, filtered in `events.rs`). It answers with an
//! ordinary message carrying the interaction's id, checked by `answer`.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use prost::Message as _;
use tonic::{Request, Response, Status};

use super::{Api, Seat, respond, users};
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{now_ms, timestamp};
use crate::pb::{self, Permission, command_service_server::CommandService};
use crate::servers::{self as store, Payload, load_channel};

/// Most commands an agent sets in one server.
const MAX_COMMANDS: usize = 50;
/// Most options a command takes.
const MAX_OPTIONS: usize = 10;
/// Most choices a STRING option offers.
const MAX_CHOICES: usize = 25;
/// Longest STRING argument, and all arguments together, in characters.
const MAX_ARGUMENT: usize = 1000;
const MAX_ARGUMENTS: usize = 4000;
/// How long an agent has to answer an interaction, and how many times.
pub(super) const ANSWER_WITHIN_MS: i64 = 15 * 60_000;
const MAX_ANSWERS: i64 = 5;
/// Buttons: rows on a message, buttons in a row, and their fields' lengths.
const MAX_ROWS: usize = 5;
const MAX_BUTTONS: usize = 5;
const MAX_CUSTOM_ID: usize = 100;
const MAX_LABEL: usize = 80;
const MAX_URL: usize = 512;

/// What people type after "/": 1 to 32 of a-z, 0-9, _ and -.
fn name(field: &str, value: &str) -> Result<String> {
    let value = value.trim();
    let shaped = (1..=32).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if !shaped {
        return Err(Error::invalid(format!("{field} is 1 to 32 of a-z, 0-9, _ and -")));
    }
    Ok(value.to_string())
}

/// Text without control characters, trimmed, of `min` to `max` characters.
fn plain(field: &str, value: &str, min: usize, max: usize) -> Result<String> {
    let value: String = value.chars().filter(|c| !c.is_control()).collect();
    super::text(field, &value, min, max)
}

/// A command as an agent may set it: shaped, with required options first.
fn check_command(command: &pb::Command) -> Result<pb::Command> {
    let command_name = name("a command's name", &command.name)?;
    let description = plain("a command's description", &command.description, 1, 100)?;
    if command.options.len() > MAX_OPTIONS {
        return Err(Error::invalid(format!("a command takes at most {MAX_OPTIONS} options")));
    }
    let mut seen = HashSet::new();
    let mut optional = false;
    let mut options = Vec::with_capacity(command.options.len());
    for option in &command.options {
        let option_name = name("an option's name", &option.name)?;
        if !seen.insert(option_name.clone()) {
            return Err(Error::invalid(format!("/{command_name} has two options called {option_name}")));
        }
        if option.required && optional {
            return Err(Error::invalid(format!("/{command_name}: required options come first")));
        }
        optional |= !option.required;
        let kind = match pb::CommandOptionType::try_from(option.r#type) {
            Ok(pb::CommandOptionType::Unspecified) => pb::CommandOptionType::String,
            Ok(kind) => kind,
            Err(_) => return Err(Error::invalid("an option's type isn't one fuwa knows")),
        };
        if !option.choices.is_empty() && kind != pb::CommandOptionType::String {
            return Err(Error::invalid("only text options have choices"));
        }
        if option.choices.len() > MAX_CHOICES {
            return Err(Error::invalid(format!("an option offers at most {MAX_CHOICES} choices")));
        }
        let choices = option.choices.iter().map(|c| plain("a choice", c, 1, 100)).collect::<Result<Vec<_>>>()?;
        options.push(pb::CommandOption {
            name: option_name,
            description: plain("an option's description", &option.description, 1, 100)?,
            r#type: kind as i32,
            required: option.required,
            choices,
        });
    }
    Ok(pb::Command { name: command_name, description, options })
}

/// What someone filled in, checked against the command's options: each once,
/// required ones there, values of the right kind, choices from the list.
async fn check_arguments(
    conn: &turso::Connection,
    access: &crate::permissions::Access,
    command: &pb::Command,
    given: &[pb::CommandArgument],
) -> Result<Vec<pb::CommandArgument>> {
    let mut by_name: HashMap<&str, &str> = HashMap::new();
    for argument in given {
        if by_name.insert(argument.name.as_str(), argument.value.as_str()).is_some() {
            return Err(Error::invalid(format!("{} is filled in twice", argument.name)));
        }
    }
    let mut out = Vec::new();
    let mut total = 0;
    for option in &command.options {
        let Some(raw) = by_name.remove(option.name.as_str()) else {
            if option.required {
                return Err(Error::invalid(format!("/{} needs {}", command.name, option.name)));
            }
            continue;
        };
        let value: String = raw.chars().filter(|c| !c.is_control()).collect();
        let value = value.trim().to_string();
        if value.is_empty() {
            if option.required {
                return Err(Error::invalid(format!("/{} needs {}", command.name, option.name)));
            }
            continue;
        }
        let ok = match pb::CommandOptionType::try_from(option.r#type).unwrap_or_default() {
            pb::CommandOptionType::Unspecified | pb::CommandOptionType::String => {
                value.chars().count() <= MAX_ARGUMENT && (option.choices.is_empty() || option.choices.contains(&value))
            }
            pb::CommandOptionType::Integer => value.parse::<i64>().is_ok(),
            pb::CommandOptionType::Boolean => value == "true" || value == "false",
            pb::CommandOptionType::User => {
                query_one(conn, "SELECT 1 FROM members WHERE user_id = ?1", [value.as_str()], |_| Ok(()))
                    .await?
                    .is_some()
            }
            pb::CommandOptionType::Channel => access.can_see(&value),
            pb::CommandOptionType::Role => {
                query_one(conn, "SELECT 1 FROM roles WHERE id = ?1", [value.as_str()], |_| Ok(())).await?.is_some()
            }
        };
        if !ok {
            return Err(Error::invalid(format!("{} isn't something /{} takes", option.name, command.name)));
        }
        total += value.chars().count();
        out.push(pb::CommandArgument { name: option.name.clone(), value });
    }
    if let Some(extra) = by_name.keys().next() {
        return Err(Error::invalid(format!("/{} has no option called {extra}", command.name)));
    }
    if total > MAX_ARGUMENTS {
        return Err(Error::invalid(format!("what's filled in can be at most {MAX_ARGUMENTS} characters in all")));
    }
    Ok(out)
}

/// Buttons as an agent may send them: at most 5 rows of 5, custom ids unique
/// on the message, link buttons to https only.
pub(super) fn check_components(rows: &[pb::ComponentRow]) -> Result<Vec<pb::ComponentRow>> {
    if rows.len() > MAX_ROWS {
        return Err(Error::invalid(format!("a message has at most {MAX_ROWS} rows of buttons")));
    }
    let mut ids = HashSet::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if row.buttons.is_empty() || row.buttons.len() > MAX_BUTTONS {
            return Err(Error::invalid(format!("a row has 1 to {MAX_BUTTONS} buttons")));
        }
        let mut buttons = Vec::with_capacity(row.buttons.len());
        for button in &row.buttons {
            let label = plain("a button's label", &button.label, 1, MAX_LABEL)?;
            let style = match pb::ButtonStyle::try_from(button.style) {
                Ok(pb::ButtonStyle::Unspecified) => pb::ButtonStyle::Secondary,
                Ok(style) => style,
                Err(_) => return Err(Error::invalid("a button's style isn't one fuwa knows")),
            };
            let (custom_id, url) = if style == pb::ButtonStyle::Link {
                if !button.custom_id.is_empty() {
                    return Err(Error::invalid("a link button has a url, not a custom_id"));
                }
                (String::new(), link(&button.url)?)
            } else {
                if !button.url.is_empty() {
                    return Err(Error::invalid("only link buttons have a url"));
                }
                let id = plain("a button's custom_id", &button.custom_id, 1, MAX_CUSTOM_ID)?;
                if !ids.insert(id.clone()) {
                    return Err(Error::invalid("two buttons on a message have the same custom_id"));
                }
                (id, String::new())
            };
            buttons.push(pb::Button { custom_id, label, style: style as i32, url, disabled: button.disabled });
        }
        out.push(pb::ComponentRow { buttons });
    }
    Ok(out)
}

/// A link button's url: https, no user name or password in it, at most 512
/// characters. Nothing on the instance ever fetches it.
fn link(value: &str) -> Result<String> {
    let value = value.trim();
    let refuse = || Error::invalid("a link button needs an https link of at most 512 characters");
    if value.len() > MAX_URL || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(refuse());
    }
    let parsed = url::Url::parse(value).map_err(|_| refuse())?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none_or(str::is_empty)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(refuse());
    }
    Ok(value.to_string())
}

/// A fresh interaction id: 16 random bytes, hex.
fn interaction_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Commands run and buttons pressed per account per minute (the instance's
/// `commands_per_minute`, unlimited unless set), kept in memory.
static PACE: Mutex<Option<HashMap<String, (i64, i64)>>> = Mutex::new(None);

fn pace(account_id: &str, now: i64, per_minute: Option<i64>) -> Result<()> {
    let Some(per_minute) = per_minute else { return Ok(()) };
    let mut guard = PACE.lock().unwrap_or_else(|e| e.into_inner());
    let counts = guard.get_or_insert_with(HashMap::new);
    let minute = now / 60_000;
    if counts.len() > 10_000 {
        counts.retain(|_, (at, _)| *at == minute);
    }
    let entry = counts.entry(account_id.to_string()).or_insert((minute, 0));
    if entry.0 != minute {
        *entry = (minute, 0);
    }
    if entry.1 >= per_minute {
        let left = (minute + 1) * 60_000 - now;
        return Err(Error::Limited("you're using commands too fast; try again in a minute".into(), left));
    }
    entry.1 += 1;
    Ok(())
}

/// Where interactions can't happen: secure channels (what's typed would reach
/// the server as plain text) and channels shared between servers (not
/// relayed yet).
async fn check_channel(conn: &turso::Connection, server_id: &str, channel_id: &str) -> Result<()> {
    let channel = load_channel(conn, server_id, channel_id).await?.ok_or(Error::NotFound("channel"))?;
    if channel.r#type == pb::ChannelType::Secure as i32 {
        return Err(Error::invalid("commands and buttons don't work in secure channels"));
    }
    if super::shared::link_of(conn, channel_id).await?.is_some() || super::polls::shared_out(conn, channel_id).await? {
        return Err(Error::invalid("commands and buttons don't work in channels shared between servers yet"));
    }
    Ok(())
}

/// Records an interaction, inside a write, and tells its agent. Old ones,
/// past answering, go as new ones come.
async fn start(
    conn: &turso::Connection,
    interaction: &pb::Interaction,
    now: i64,
    events: &mut Vec<Payload>,
) -> Result<()> {
    conn.execute("DELETE FROM interactions WHERE created_at < ?1", [now - ANSWER_WITHIN_MS]).await?;
    let typed = pb::Interaction { arguments: interaction.arguments.clone(), ..Default::default() }.encode_to_vec();
    conn.execute(
        "INSERT INTO interactions (id, agent_id, user_id, channel_id, kind, command, arguments, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        (
            interaction.id.as_str(),
            interaction.agent_id.as_str(),
            interaction.user_id.as_str(),
            interaction.channel_id.as_str(),
            i64::from(interaction.kind),
            interaction.command.as_str(),
            typed,
            now,
        ),
    )
    .await?;
    // The log keeps the event as long as any other; what was typed stays in
    // the row, which goes after 15 minutes, and `fill_arguments` adds it back.
    let stored = pb::Interaction { arguments: Vec::new(), ..interaction.clone() };
    events.push(Payload::InteractionCreated(pb::InteractionCreated { interaction: Some(stored) }));
    Ok(())
}

/// What was typed for an interaction its agent is being shown, from its row
/// while it can still be answered; after that (or once the row is gone with
/// the person or the agent) the event carries none.
pub(super) async fn fill_arguments(conn: &turso::Connection, interaction: &mut pb::Interaction, now: i64) {
    if interaction.kind != pb::InteractionKind::Command as i32 || !interaction.arguments.is_empty() {
        return;
    }
    let typed = query_one(
        conn,
        "SELECT arguments FROM interactions WHERE id = ?1 AND agent_id = ?2 AND created_at >= ?3",
        (interaction.id.as_str(), interaction.agent_id.as_str(), now - ANSWER_WITHIN_MS),
        |r| r.get::<Option<Vec<u8>>>(0),
    )
    .await;
    if let Ok(Some(Some(bytes))) = typed
        && let Ok(typed) = pb::Interaction::decode(bytes.as_slice())
    {
        interaction.arguments = typed.arguments;
    }
}

/// An agent answering `interaction_id` with a message in `channel_id`,
/// inside the send's write: its own interaction, in the same channel, within
/// 15 minutes and 5 answers. What the message shows of it: never what was
/// typed.
pub(super) async fn answer(
    conn: &turso::Connection,
    agent_id: &str,
    interaction_id: &str,
    channel_id: &str,
    now: i64,
) -> Result<pb::MessageInteraction> {
    let row = query_one(
        conn,
        "SELECT agent_id, user_id, channel_id, kind, command, created_at, answers FROM interactions WHERE id = ?1",
        [interaction_id],
        |r| {
            Ok((
                r.get::<String>(0)?,
                r.get::<String>(1)?,
                r.get::<String>(2)?,
                r.get::<i64>(3)?,
                r.get::<String>(4)?,
                r.get::<i64>(5)?,
                r.get::<i64>(6)?,
            ))
        },
    )
    .await?;
    let Some((agent, user_id, channel, kind, command, created_at, answers)) = row.filter(|r| r.0 == agent_id) else {
        return Err(Error::NotFound("interaction"));
    };
    let _ = agent;
    if channel != channel_id {
        return Err(Error::invalid("answer an interaction in the channel it came from"));
    }
    if now - created_at > ANSWER_WITHIN_MS {
        return Err(Error::FailedPrecondition("it's too late to answer this interaction".into()));
    }
    if answers >= MAX_ANSWERS {
        return Err(Error::FailedPrecondition(format!("an interaction takes at most {MAX_ANSWERS} answers")));
    }
    conn.execute("UPDATE interactions SET answers = answers + 1 WHERE id = ?1", [interaction_id]).await?;
    Ok(pb::MessageInteraction { id: interaction_id.to_string(), kind: kind as i32, command, user_id })
}

/// The commands an agent set here, in its order.
async fn agent_commands(conn: &turso::Connection, agent_id: &str) -> Result<Vec<pb::Command>> {
    let rows = query_all(conn, "SELECT body FROM commands WHERE agent_id = ?1 ORDER BY position", [agent_id], |r| {
        r.get::<Vec<u8>>(0)
    })
    .await?;
    rows.iter().map(|body| Ok(pb::Command::decode(body.as_slice())?)).collect()
}

#[tonic::async_trait]
impl CommandService for Api {
    async fn set_commands(
        &self,
        request: Request<pb::SetCommandsRequest>,
    ) -> Result<Response<pb::SetCommandsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                if account.kind != pb::AccountKind::Agent {
                    return Err(Error::denied("only agents have commands"));
                }
                let req = request.into_inner();
                if req.commands.len() > MAX_COMMANDS {
                    return Err(Error::invalid(format!("an agent sets at most {MAX_COMMANDS} commands in a server")));
                }
                let commands = req.commands.iter().map(check_command).collect::<Result<Vec<_>>>()?;
                let mut names = HashSet::new();
                if let Some(twice) = commands.iter().find(|c| !names.insert(c.name.as_str())) {
                    return Err(Error::invalid(format!("there are two commands called /{}", twice.name)));
                }
                let Seat { sdb, .. } = self.membership(&account, &req.server_id).await?;
                sdb.write(&account.id, async |conn, _events| {
                    if store::member(conn, &sdb.id, &account.id).await?.is_none() {
                        return Err(Error::denied("join this server first"));
                    }
                    conn.execute("DELETE FROM commands WHERE agent_id = ?1", [account.id.as_str()]).await?;
                    for (position, command) in commands.iter().enumerate() {
                        conn.execute(
                            "INSERT INTO commands (agent_id, name, position, body) VALUES (?1, ?2, ?3, ?4)",
                            (account.id.as_str(), command.name.as_str(), position as i64, command.encode_to_vec()),
                        )
                        .await?;
                    }
                    Ok(())
                })
                .await?;
                Ok(pb::SetCommandsResponse { commands })
            }
            .await,
        )
    }

    async fn list_commands(
        &self,
        request: Request<pb::ListCommandsRequest>,
    ) -> Result<Response<pb::ListCommandsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, .. } = self.membership(&account, &req.server_id).await?;
                let conn = sdb.read()?;
                // Only agents still here; their rows go when they leave.
                let rows = query_all(
                    &conn,
                    "SELECT c.agent_id, c.body FROM commands c JOIN members m ON m.user_id = c.agent_id
                     ORDER BY c.agent_id, c.position",
                    (),
                    |r| Ok((r.get::<String>(0)?, r.get::<Vec<u8>>(1)?)),
                )
                .await?;
                let mut commands = Vec::with_capacity(rows.len());
                for (agent_id, body) in rows {
                    commands.push(pb::ServerCommand { agent_id, command: Some(pb::Command::decode(body.as_slice())?) });
                }
                let ids: Vec<&str> = commands.iter().map(|c| c.agent_id.as_str()).collect();
                let agents = users(&conn, &ids).await?;
                Ok(pb::ListCommandsResponse { commands, agents })
            }
            .await,
        )
    }

    async fn run_command(
        &self,
        request: Request<pb::RunCommandRequest>,
    ) -> Result<Response<pb::RunCommandResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, member, access } = self.membership(&account, &req.server_id).await?;
                super::messages::check_not_timed_out(&member)?;
                access.require_in(&req.channel_id, Permission::SendMessages)?;
                let now = now_ms();
                pace(&account.id, now, self.app.settings().limits.commands_per_minute)?;
                let id = interaction_id();
                sdb.write(&account.id, async |conn, events| {
                    check_channel(conn, &sdb.id, &req.channel_id).await?;
                    let command = agent_commands(conn, &req.agent_id)
                        .await?
                        .into_iter()
                        .find(|c| c.name == req.command)
                        .ok_or(Error::NotFound("command"))?;
                    // The agent has to be able to see the channel to answer in it.
                    let agent_sees = store::member_access(conn, &sdb.id, &req.agent_id)
                        .await?
                        .is_some_and(|(_, agent)| agent.can_see(&req.channel_id));
                    if !agent_sees {
                        return Err(Error::FailedPrecondition("that agent can't see this channel".into()));
                    }
                    let arguments = check_arguments(conn, &access, &command, &req.arguments).await?;
                    let interaction = pb::Interaction {
                        id: id.clone(),
                        server_id: sdb.id.clone(),
                        channel_id: req.channel_id.clone(),
                        agent_id: req.agent_id.clone(),
                        user_id: account.id.clone(),
                        kind: pb::InteractionKind::Command as i32,
                        command: command.name,
                        arguments,
                        message_id: String::new(),
                        custom_id: String::new(),
                        created_at: Some(timestamp(now)),
                    };
                    start(conn, &interaction, now, events).await
                })
                .await?;
                Ok(pb::RunCommandResponse { interaction_id: id })
            }
            .await,
        )
    }

    async fn press_button(
        &self,
        request: Request<pb::PressButtonRequest>,
    ) -> Result<Response<pb::PressButtonResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, member, access } = self.membership(&account, &req.server_id).await?;
                super::messages::check_not_timed_out(&member)?;
                if access.pending {
                    return Err(Error::FailedPrecondition("agree to the server's rules first".into()));
                }
                let now = now_ms();
                pace(&account.id, now, self.app.settings().limits.commands_per_minute)?;
                let id = interaction_id();
                sdb.write(&account.id, async |conn, events| {
                    let message = super::messages::load_message(conn, &sdb.id, &req.message_id)
                        .await?
                        .filter(|m| access.can_see(&m.channel_id))
                        .ok_or(Error::NotFound("message"))?;
                    let pressable = message
                        .components
                        .iter()
                        .flat_map(|row| &row.buttons)
                        .any(|b| !b.disabled && !b.custom_id.is_empty() && b.custom_id == req.custom_id);
                    if !pressable {
                        return Err(Error::NotFound("button"));
                    }
                    check_channel(conn, &sdb.id, &message.channel_id).await?;
                    // Only agents' messages have buttons, and only one still here hears them.
                    if store::member(conn, &sdb.id, &message.author_id).await?.is_none() {
                        return Err(Error::FailedPrecondition("the agent that sent this has left".into()));
                    }
                    let interaction = pb::Interaction {
                        id: id.clone(),
                        server_id: sdb.id.clone(),
                        channel_id: message.channel_id.clone(),
                        agent_id: message.author_id.clone(),
                        user_id: account.id.clone(),
                        kind: pb::InteractionKind::Button as i32,
                        command: String::new(),
                        arguments: vec![],
                        message_id: message.id.clone(),
                        custom_id: req.custom_id.clone(),
                        created_at: Some(timestamp(now)),
                    };
                    start(conn, &interaction, now, events).await
                })
                .await?;
                Ok(pb::PressButtonResponse { interaction_id: id })
            }
            .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_shaped() {
        let option = |name: &str, required: bool| pb::CommandOption {
            name: name.into(),
            description: "what".into(),
            required,
            ..Default::default()
        };
        let command = pb::Command {
            name: "roll".into(),
            description: "Rolls dice\u{7}".into(),
            options: vec![option("sides", true), option("times", false)],
        };
        let checked = check_command(&command).unwrap();
        assert_eq!(checked.description, "Rolls dice", "control characters dropped");
        assert_eq!(checked.options[0].r#type, pb::CommandOptionType::String as i32);
        assert!(check_command(&pb::Command { name: "Roll".into(), ..command.clone() }).is_err());
        let backwards = pb::Command { options: vec![option("times", false), option("sides", true)], ..command.clone() };
        assert!(check_command(&backwards).is_err(), "required options first");
    }

    #[test]
    fn link_buttons_go_only_to_https() {
        assert!(link("https://waifu.dev/docs").is_ok());
        for bad in ["http://waifu.dev", "javascript:alert(1)", "https://me:pw@waifu.dev", "https://", "https://a b"] {
            assert!(link(bad).is_err(), "{bad}");
        }
        assert!(link(&format!("https://waifu.dev/{}", "a".repeat(600))).is_err());
    }

    #[test]
    fn buttons_are_shaped() {
        let button = |id: &str| pb::Button { custom_id: id.into(), label: "Go".into(), ..Default::default() };
        let rows = check_components(&[pb::ComponentRow { buttons: vec![button("a"), button("b")] }]).unwrap();
        assert_eq!(rows[0].buttons[0].style, pb::ButtonStyle::Secondary as i32);
        assert!(check_components(&[pb::ComponentRow { buttons: vec![button("a"), button("a")] }]).is_err());
        assert!(check_components(&[pb::ComponentRow { buttons: vec![] }]).is_err());
        let link = pb::Button { style: pb::ButtonStyle::Link as i32, url: "https://waifu.dev".into(), ..button("") };
        assert!(check_components(&[pb::ComponentRow { buttons: vec![link.clone()] }]).is_ok());
        assert!(
            check_components(&[pb::ComponentRow { buttons: vec![pb::Button { custom_id: "x".into(), ..link }] }])
                .is_err()
        );
    }

    #[test]
    fn the_pace_says_how_long_to_wait() {
        let who = "pace-test-account";
        assert!(pace(who, 60_000, Some(1)).is_ok());
        match pace(who, 90_000, Some(1)) {
            Err(Error::Limited(_, wait)) => assert_eq!(wait, 30_000),
            other => panic!("{other:?}"),
        }
        assert!(pace(who, 120_000, Some(1)).is_ok(), "a new minute");
    }
}
