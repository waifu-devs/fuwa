//! Agents' slash commands and buttons (docs/commands.md), as the web app's
//! `Commands.tsx`: the commands of the agents in a server, running one with
//! what was filled in, and pressing a button on an agent's message. The
//! agent answers with an ordinary message headed "used /name".
//!
//! Not in threads, secure channels, shared channels or private
//! conversations, and only on an instance that has `agent-commands`.

use std::collections::HashMap;

use crate::core::api::Problem;
use crate::core::{Core, reports};
use crate::pb;
use crate::rpc;

/// The most commands the "/" list shows.
pub const SHOWN: usize = 8;
/// How long a server's list is reused before asking again.
pub const FRESH: std::time::Duration = std::time::Duration::from_secs(30);
/// The longest text a STRING option takes.
pub const MAX_TEXT: usize = 1000;

/// One command people can pick: whose it is, and the command.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub agent_id: String,
    pub agent: Option<pb::User>,
    pub command: pb::Command,
}

impl Choice {
    /// Unique in a server: the agent and the command's name.
    pub fn key(&self) -> String {
        format!("{}/{}", self.agent_id, self.command.name)
    }
}

/// What you're typing in the "/" list: the name so far, lowercased, while
/// the box holds "/" and a name with no space yet.
pub fn query(text: &str) -> Option<String> {
    let name = text.strip_prefix('/')?;
    let ok = name.len() <= 32 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    ok.then(|| name.to_ascii_lowercase())
}

/// A server's commands as the list offers them, in the order the instance gave.
pub fn choices(list: &pb::ListCommandsResponse) -> Vec<Choice> {
    let agents: HashMap<&str, &pb::User> = list.agents.iter().map(|a| (a.id.as_str(), a)).collect();
    list.commands
        .iter()
        .filter_map(|c| {
            Some(Choice {
                agent_id: c.agent_id.clone(),
                agent: agents.get(c.agent_id.as_str()).map(|a| (*a).clone()),
                command: c.command.clone()?,
            })
        })
        .collect()
}

/// The commands that match what's typed: names that start with it first,
/// then those that have it anywhere, at most [`SHOWN`].
pub fn matching(choices: &[Choice], query: &str) -> Vec<Choice> {
    let starts = choices.iter().filter(|c| c.command.name.starts_with(query));
    let rest = choices.iter().filter(|c| !c.command.name.starts_with(query) && c.command.name.contains(query));
    starts.chain(rest).take(SHOWN).cloned().collect()
}

/// An option's type, unset being STRING.
pub fn kind(option: &pb::CommandOption) -> pb::CommandOptionType {
    match option.r#type() {
        pb::CommandOptionType::Unspecified => pb::CommandOptionType::String,
        other => other,
    }
}

/// Whether an option is picked from a list rather than typed.
pub fn picked(option: &pb::CommandOption) -> bool {
    match kind(option) {
        pb::CommandOptionType::String => !option.choices.is_empty(),
        pb::CommandOptionType::Integer => false,
        _ => true,
    }
}

/// Whether a value will do for its option: a whole number for INTEGER,
/// anything else as long as it's there. Empty is fine for optional ones.
pub fn valid(option: &pb::CommandOption, value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return !option.required;
    }
    match kind(option) {
        pb::CommandOptionType::Integer => value.parse::<i64>().is_ok(),
        pb::CommandOptionType::String if option.choices.is_empty() => value.chars().count() <= MAX_TEXT,
        _ => true,
    }
}

/// The options that still need something, by name.
pub fn missing(command: &pb::Command, values: &HashMap<String, String>) -> Vec<String> {
    command
        .options
        .iter()
        .filter(|o| !valid(o, values.get(&o.name).map_or("", String::as_str)))
        .map(|o| o.name.clone())
        .collect()
}

/// What's sent: each option that has a value, trimmed, in the command's order.
pub fn arguments(command: &pb::Command, values: &HashMap<String, String>) -> Vec<pb::CommandArgument> {
    command
        .options
        .iter()
        .filter_map(|o| {
            let value = values.get(&o.name)?.trim();
            (!value.is_empty()).then(|| pb::CommandArgument { name: o.name.clone(), value: value.to_owned() })
        })
        .collect()
}

/// A link button opens only an https link with somewhere to go, and nothing
/// in it a system opener could read as something else.
pub fn opens(button: &pb::Button) -> Option<&str> {
    let url = button.url.as_str();
    let https = url.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://")) && url.len() > 8;
    let plain = !url.chars().any(|c| c.is_whitespace() || c.is_control());
    (button.style() == pb::ButtonStyle::Link && https && plain).then_some(url)
}

/// Whether a channel takes commands: not shared, and never a secure one, whose
/// words mustn't reach the server in the clear.
pub fn takes_commands(channel: &pb::Channel) -> bool {
    channel.shared.is_none() && channel.r#type != pb::ChannelType::Secure as i32
}

impl Core {
    /// The commands of every agent in a server.
    pub async fn list_commands(&self, key: &str, server_id: &str) -> Result<Vec<Choice>, Problem> {
        let Some(api) = self.api(key) else { return Ok(Vec::new()) };
        let list = rpc!(api.commands(), list_commands(pb::ListCommandsRequest { server_id: server_id.into() })).await?;
        Ok(choices(&list))
    }

    /// Runs an agent's command in a channel with what was filled in.
    pub async fn run_command(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        choice: &Choice,
        arguments: Vec<pb::CommandArgument>,
    ) -> Result<(), Problem> {
        let Some(api) = self.api(key) else { return Ok(()) };
        reports::used("command.run");
        rpc!(
            api.commands(),
            run_command(pb::RunCommandRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                agent_id: choice.agent_id.clone(),
                command: choice.command.name.clone(),
                arguments,
            })
        )
        .await?;
        Ok(())
    }

    /// Presses a button on an agent's message.
    pub async fn press_button(
        &self,
        key: &str,
        server_id: &str,
        message_id: &str,
        custom_id: &str,
    ) -> Result<(), Problem> {
        let Some(api) = self.api(key) else { return Ok(()) };
        reports::used("command.press");
        rpc!(
            api.commands(),
            press_button(pb::PressButtonRequest {
                server_id: server_id.into(),
                message_id: message_id.into(),
                custom_id: custom_id.into(),
            })
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str, options: Vec<pb::CommandOption>) -> pb::Command {
        pb::Command { name: name.into(), description: "does it".into(), options }
    }

    fn option(name: &str, kind: pb::CommandOptionType, required: bool) -> pb::CommandOption {
        pb::CommandOption { name: name.into(), r#type: kind as i32, required, ..Default::default() }
    }

    #[test]
    fn the_list_opens_on_a_slash_and_a_name() {
        assert_eq!(query("/"), Some(String::new()));
        assert_eq!(query("/Ro_ll-2"), Some("ro_ll-2".into()));
        assert_eq!(query("/roll 3"), None);
        assert_eq!(query("roll"), None);
        assert_eq!(query(&format!("/{}", "a".repeat(33))), None);
    }

    #[test]
    fn names_that_start_with_it_come_first() {
        let list = pb::ListCommandsResponse {
            commands: ["unroll", "roll", "help", "rolls"]
                .map(|n| pb::ServerCommand { agent_id: "a".into(), command: Some(command(n, vec![])) })
                .into(),
            agents: vec![pb::User { id: "a".into(), username: "dice".into(), ..Default::default() }],
        };
        let all = choices(&list);
        assert_eq!(all[0].agent.as_ref().map(|a| a.username.as_str()), Some("dice"));
        let names: Vec<_> = matching(&all, "roll").into_iter().map(|c| c.command.name).collect();
        assert_eq!(names, ["roll", "rolls", "unroll"]);
        assert_eq!(matching(&all, "").len(), 4);
    }

    #[test]
    fn required_options_and_whole_numbers_must_be_there() {
        let c = command(
            "roll",
            vec![
                option("sides", pb::CommandOptionType::Integer, true),
                option("who", pb::CommandOptionType::User, false),
                option("note", pb::CommandOptionType::Unspecified, false),
            ],
        );
        let mut values = HashMap::new();
        assert_eq!(missing(&c, &values), ["sides"]);
        values.insert("sides".to_owned(), "six".to_owned());
        assert_eq!(missing(&c, &values), ["sides"]);
        values.insert("sides".to_owned(), " -6 ".to_owned());
        assert!(missing(&c, &values).is_empty());
        values.insert("note".to_owned(), "   ".to_owned());
        let sent = arguments(&c, &values);
        assert_eq!(sent, vec![pb::CommandArgument { name: "sides".into(), value: "-6".into() }]);
    }

    #[test]
    fn some_options_are_picked_from_a_list() {
        assert!(!picked(&option("t", pb::CommandOptionType::Unspecified, true)));
        assert!(!picked(&option("n", pb::CommandOptionType::Integer, true)));
        assert!(picked(&option("b", pb::CommandOptionType::Boolean, true)));
        assert!(picked(&option("c", pb::CommandOptionType::Channel, true)));
        let mut choice = option("s", pb::CommandOptionType::String, true);
        choice.choices = vec!["red".into(), "blue".into()];
        assert!(picked(&choice));
    }

    #[test]
    fn secure_and_shared_channels_take_no_commands() {
        let text = pb::Channel { r#type: pb::ChannelType::Text as i32, ..Default::default() };
        assert!(takes_commands(&text));
        assert!(!takes_commands(&pb::Channel { r#type: pb::ChannelType::Secure as i32, ..text.clone() }));
        let shared = pb::Channel { shared: Some(Default::default()), ..text };
        assert!(!takes_commands(&shared));
    }

    #[test]
    fn link_buttons_open_only_https() {
        let link = |url: &str| pb::Button {
            label: "Docs".into(),
            style: pb::ButtonStyle::Link as i32,
            url: url.into(),
            ..Default::default()
        };
        assert_eq!(opens(&link("https://example.com/a")), Some("https://example.com/a"));
        assert_eq!(opens(&link("HTTPS://example.com")), Some("HTTPS://example.com"));
        assert_eq!(opens(&link("https://")), None);
        assert_eq!(opens(&link("https://example.com/a b")), None);
        assert_eq!(opens(&link("https://example.com/\n--flag")), None);
        assert_eq!(opens(&link("https://example.com/\u{7}")), None);
        assert_eq!(opens(&link("http://example.com")), None);
        assert_eq!(opens(&link("javascript:alert(1)")), None);
        assert_eq!(opens(&pb::Button { url: "https://x.y".into(), ..Default::default() }), None);
    }
}
