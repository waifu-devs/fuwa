//! Shared channels (like Slack Connect): one server, the channel's home,
//! keeps the channel and its messages; another shows it as a channel of its
//! own. The small decisions the screens share, the web's `lib/shared.ts`,
//! and the calls to `SharedChannelService` its `actions.ts` makes.

use tonic::Code;

use crate::core::Core;
use crate::core::api::Problem;
use crate::pb;
use crate::rpc;

fn missing() -> Problem {
    Problem::new(Code::NotFound, "That instance isn't here.")
}

/// What the home can let a guest server's people do; seeing the channel comes with being shown it.
pub const SHAREABLE: [(pb::Permission, &str, &str); 3] = [
    (pb::Permission::SendMessages, "Send messages", "message-square"),
    (pb::Permission::EmbedLinks, "Embed links", "link"),
    (pb::Permission::AttachFiles, "Attach files", "image"),
];

pub fn waiting(c: &pb::SharedConnection) -> bool {
    c.state() == pb::SharedConnectionState::Waiting
}

/// The 26 letters of a ULID, which a share code starts with (Crockford's base 32, any case).
fn ulid_char(c: u8) -> bool {
    matches!(c.to_ascii_uppercase(), b'0'..=b'9' | b'A'..=b'H' | b'J' | b'K' | b'M' | b'N' | b'P'..=b'T' | b'V'..=b'Z')
}

/// How long the code at the start of `s` is, if one starts there.
fn code_at(s: &[u8]) -> Option<usize> {
    if s.len() < 43 || !s[..26].iter().all(|&c| ulid_char(c)) || s[26] != b'-' {
        return None;
    }
    if !s[27..43].iter().all(u8::is_ascii_alphanumeric) {
        return None;
    }
    Some(43 + s.get(43).filter(|&&c| c == b'@').and_then(|_| instance_at(&s[44..])).map_or(0, |n| n + 1))
}

/// How long the instance after a code's "@" is: an optional scheme, a host, an optional port.
fn instance_at(s: &[u8]) -> Option<usize> {
    let lower = |n: usize| s.get(..n).map(|p| p.to_ascii_lowercase());
    let mut at = if lower(8).as_deref() == Some(b"https://") {
        8
    } else if lower(7).as_deref() == Some(b"http://") {
        7
    } else {
        0
    };
    let host = &s[at..];
    let len = if host.first() == Some(&b'[') {
        let end = host.iter().position(|&c| c == b']')?;
        if end < 2 || !host[1..end].iter().all(|&c| c.is_ascii_hexdigit() || c == b':' || c == b'.') {
            return None;
        }
        end + 1
    } else {
        let run = host.iter().take_while(|&&c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-').count();
        // A host ends on a letter or digit, so a full stop after it stays out.
        let run = host[..run].iter().rposition(u8::is_ascii_alphanumeric)? + 1;
        if !host[0].is_ascii_alphanumeric() {
            return None;
        }
        run
    };
    at += len;
    if s.get(at) == Some(&b':') {
        let digits = s[at + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if (1..=5).contains(&digits) {
            at += 1 + digits;
        }
    }
    Some(at)
}

/// A share code is the home server's id, a dash and 16 letters and digits, then "@" and
/// the home's instance when it's for servers on other instances too. People paste
/// them out of chats, so look for one anywhere in the text. "" when there's none.
pub fn find_share_code(text: &str) -> &str {
    let bytes = text.as_bytes();
    (0..bytes.len())
        .filter(|&n| text.is_char_boundary(n))
        .find_map(|n| code_at(&bytes[n..]).map(|len| &text[n..n + len]))
        .unwrap_or("")
}

/// The instance a share code names, as people read it, or "" for a code for this instance only.
pub fn share_code_instance(code: &str) -> &str {
    code.split_once('@').map_or("", |(_, at)| at.strip_prefix("https://").unwrap_or(at))
}

/// Who's on the other end of a shared channel, as the sidebar and header say it.
pub struct Label {
    pub home: bool,
    pub names: String,
    pub text: String,
}

pub fn shared_label(channel: &pb::Channel) -> Option<Label> {
    let shared = channel.shared.as_ref()?;
    if shared.home {
        let names = list_names(shared.guests.iter().map(|g| g.name.as_str()));
        let text = if names.is_empty() { "Shared".to_owned() } else { format!("Shared with {names}") };
        return Some(Label { home: true, names, text });
    }
    let names = shared.home_server.as_ref().map(|s| s.name.clone()).unwrap_or_default();
    let text = if names.is_empty() { "Shared".to_owned() } else { format!("Shared from {names}") };
    Some(Label { home: false, names, text })
}

/// What the header's pill says: who it's shown to, or where its messages live.
pub fn pill_text(channel: &pb::Channel) -> Option<String> {
    let label = shared_label(channel)?;
    Some(if label.home {
        label.text
    } else {
        let names = if label.names.is_empty() { "another server".to_owned() } else { label.names };
        format!("Shared · messages live on {names}")
    })
}

/// "A", "A and B", "A, B and C".
pub fn list_names<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    let clean: Vec<&str> = names.into_iter().filter(|n| !n.is_empty()).collect();
    match clean.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The server a message's author is from, when it isn't the one it's read in:
/// at the home that's set only for guests' messages; at a guest it's set on
/// every message, so the guest's own people are left untagged.
pub fn foreign_server<'a>(message: &'a pb::Message, server_id: &str) -> Option<&'a pb::SharedServer> {
    message.shared.as_ref()?.server.as_ref().filter(|s| !s.id.is_empty() && s.id != server_id)
}

/// How long a share code has left, in words: "6 days", "3 hours", "a few minutes".
pub fn code_left(ms: i64) -> String {
    if ms <= 0 {
        return "expired".into();
    }
    let hours = ms as f64 / 3_600_000.0;
    // A code made a moment ago "works for 7 days", not 6 and a bit.
    if hours >= 36.0 {
        return format!("{} days", (hours / 24.0).round());
    }
    if hours >= 2.0 {
        return format!("{} hours", hours.floor());
    }
    let minutes = ms / 60_000;
    if minutes >= 5 { format!("{minutes} minutes") } else { "a few minutes".into() }
}

impl Core {
    /// The server's shared channels both ways, requests waiting, codes and people kept out.
    /// Kept in the store, where the event stream finds them to read again.
    pub async fn list_connections(&self, key: &str, server_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res =
            rpc!(api.shared(), list_connections(pb::ListConnectionsRequest { server_id: server_id.into() })).await?;
        self.shared.instance(key, |i| {
            for user in res.blocks.iter().filter_map(|b| b.user.as_ref()) {
                i.users.insert(user.id.clone(), user.clone());
            }
            i.shared.insert(server_id.to_owned(), res);
        });
        Ok(())
    }

    /// Reads the list again where it's kept, after a change made here.
    async fn relist_shared(&self, key: &str, server_id: &str) {
        if self.shared.read(|s| s.instance(key).is_some_and(|i| i.shared.contains_key(server_id))) {
            let _ = self.list_connections(key, server_id).await;
        }
    }

    pub async fn create_share_code(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        other_instances: bool,
    ) -> Result<pb::ShareCode, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.shared(),
            create_share_code(pb::CreateShareCodeRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                other_instances,
            })
        )
        .await?;
        self.relist_shared(key, server_id).await;
        res.code.ok_or_else(|| Problem::new(Code::Internal, "The instance sent no code."))
    }

    pub async fn delete_share_code(&self, key: &str, server_id: &str, code: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.shared(),
            delete_share_code(pb::DeleteShareCodeRequest { server_id: server_id.into(), code: code.into() })
        )
        .await?;
        self.relist_shared(key, server_id).await;
        Ok(())
    }

    pub async fn preview_share(
        &self,
        key: &str,
        server_id: &str,
        code: &str,
    ) -> Result<pb::PreviewShareResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.shared(), preview_share(pb::PreviewShareRequest { server_id: server_id.into(), code: code.into() }))
            .await
    }

    /// Asks to show the channel here, named `name` (empty keeps the home's) under `parent_id`.
    pub async fn accept_share(
        &self,
        key: &str,
        server_id: &str,
        code: &str,
        name: &str,
        parent_id: &str,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.shared(),
            accept_share(pb::AcceptShareRequest {
                server_id: server_id.into(),
                code: code.into(),
                name: name.into(),
                parent_id: parent_id.into(),
            })
        )
        .await?;
        self.relist_shared(key, server_id).await;
        Ok(())
    }

    pub async fn review_share(
        &self,
        key: &str,
        server_id: &str,
        connection_id: &str,
        approve: bool,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.shared(),
            review_share(pb::ReviewShareRequest {
                server_id: server_id.into(),
                connection_id: connection_id.into(),
                approve,
            })
        )
        .await?;
        self.relist_shared(key, server_id).await;
        Ok(())
    }

    /// What a guest server's people may do in one of this server's channels.
    pub async fn update_connection(
        &self,
        key: &str,
        server_id: &str,
        connection_id: &str,
        allowed: Vec<pb::Permission>,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.shared(),
            update_connection(pb::UpdateConnectionRequest {
                server_id: server_id.into(),
                connection_id: connection_id.into(),
                allowed: allowed.into_iter().map(|p| p as i32).collect(),
            })
        )
        .await?;
        if let Some(updated) = res.connection {
            self.shared.instance(key, |i| {
                if let Some(c) =
                    i.shared.get_mut(server_id).and_then(|l| l.connections.iter_mut().find(|c| c.id == updated.id))
                {
                    *c = updated;
                }
            });
        }
        Ok(())
    }

    /// Ends a connection, or withdraws or turns down a request.
    pub async fn disconnect_shared(&self, key: &str, server_id: &str, connection_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.shared(),
            disconnect(pb::DisconnectRequest { server_id: server_id.into(), connection_id: connection_id.into() })
        )
        .await?;
        self.relist_shared(key, server_id).await;
        Ok(())
    }

    /// Keeps someone from another server out of one of this server's shared channels, or lets them back.
    pub async fn block_from_channel(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        user_id: &str,
        blocked: bool,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.shared(),
            block_from_channel(pb::BlockFromChannelRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                user_id: user_id.into(),
                blocked,
            })
        )
        .await?;
        self.relist_shared(key, server_id).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "01JABCDEFGHJKMNPQRSTVWXYZ0-AbCdEfGh23456789";

    #[test]
    fn a_share_code_is_found_in_whatever_was_pasted_around_it() {
        assert_eq!(find_share_code(CODE), CODE);
        assert_eq!(find_share_code(&format!("  here you go: {CODE}\n")), CODE);
        assert_eq!(find_share_code("not a code"), "");
        assert_eq!(find_share_code("01JABCDEFGHJKMNPQRSTVWXYZ0-short"), "");
        assert_eq!(find_share_code(&format!("ñ {CODE}")), CODE);
    }

    #[test]
    fn a_code_for_other_instances_keeps_the_instance_it_names() {
        let remote = format!("{CODE}@chat.example.com");
        assert_eq!(find_share_code(&format!("join us: {remote}.")), remote);
        assert_eq!(
            find_share_code(&format!("{CODE}@chat.example.com:8443 thanks")),
            format!("{CODE}@chat.example.com:8443")
        );
        assert_eq!(find_share_code(&format!("{CODE}@http://127.0.0.1:4000")), format!("{CODE}@http://127.0.0.1:4000"));
        assert_eq!(find_share_code(&format!("{CODE}@[::1]:4000")), format!("{CODE}@[::1]:4000"));
        assert_eq!(share_code_instance(&remote), "chat.example.com");
        assert_eq!(share_code_instance(&format!("{CODE}@http://127.0.0.1:4000")), "http://127.0.0.1:4000");
        assert_eq!(share_code_instance(CODE), "");
    }

    #[test]
    fn names_read_like_a_sentence() {
        assert_eq!(list_names([]), "");
        assert_eq!(list_names(["Cats"]), "Cats");
        assert_eq!(list_names(["Cats", "Dogs"]), "Cats and Dogs");
        assert_eq!(list_names(["Cats", "Dogs", "Birds"]), "Cats, Dogs and Birds");
    }

    fn server(id: &str, name: &str) -> pb::SharedServer {
        pb::SharedServer { id: id.into(), name: name.into(), icon_url: String::new() }
    }

    #[test]
    fn the_label_says_which_way_a_channel_is_shared() {
        let mut channel = pb::Channel::default();
        assert!(shared_label(&channel).is_none());
        channel.shared =
            Some(pb::SharedChannel { home: true, guests: vec![server("g", "Guests")], ..Default::default() });
        assert_eq!(shared_label(&channel).unwrap().text, "Shared with Guests");
        assert_eq!(pill_text(&channel).unwrap(), "Shared with Guests");
        channel.shared = Some(pb::SharedChannel {
            home: false,
            home_server: Some(server("h", "Home")),
            home_channel_name: "general".into(),
            guests: vec![],
        });
        assert_eq!(shared_label(&channel).unwrap().text, "Shared from Home");
        assert_eq!(pill_text(&channel).unwrap(), "Shared · messages live on Home");
    }

    #[test]
    fn only_authors_from_another_server_get_a_tag() {
        let mut m = pb::Message::default();
        assert!(foreign_server(&m, "here").is_none());
        m.shared = Some(pb::SharedAuthor { user: None, server: Some(server("here", "Us")) });
        assert!(foreign_server(&m, "here").is_none());
        m.shared = Some(pb::SharedAuthor { user: None, server: Some(server("there", "Them")) });
        assert_eq!(foreign_server(&m, "here").unwrap().name, "Them");
    }

    #[test]
    fn time_left_on_a_code_reads_plainly() {
        assert_eq!(code_left(7 * 24 * 3_600_000), "7 days");
        assert_eq!(code_left(7 * 24 * 3_600_000 - 60_000), "7 days");
        assert_eq!(code_left(5 * 3_600_000), "5 hours");
        assert_eq!(code_left(30 * 60_000), "30 minutes");
        assert_eq!(code_left(60_000), "a few minutes");
        assert_eq!(code_left(0), "expired");
    }
}
