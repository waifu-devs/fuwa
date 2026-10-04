//! Every community server on an instance, for its admins: what each holds,
//! its own caps, which region keeps it, its shared channels, its whole file,
//! and deleting it, whether or not they're in it. The web's
//! `settings/instance/Servers.tsx` and `lib/regions.ts`.

use std::io::Write as _;
use std::path::Path;

use tonic::Code;

use crate::core::Core;
use crate::core::api::{CALL_TIMEOUT, Problem};
use crate::pb;
use crate::rpc;

fn missing() -> Problem {
    Problem::new(Code::NotFound, "That instance isn't here.")
}

/// How the list is sorted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sort {
    Biggest,
    Members,
    Newest,
}

pub fn storage_of(s: &pb::InstanceServer) -> i64 {
    s.usage.as_ref().map_or(0, |u| u.storage_bytes)
}

pub fn members_of(s: &pb::InstanceServer) -> i64 {
    s.usage.as_ref().map(|u| u.members).or_else(|| s.server.as_ref().map(|s| s.member_count)).unwrap_or(0)
}

fn created_of(s: &pb::InstanceServer) -> i64 {
    s.server.as_ref().and_then(|s| s.created_at.as_ref()).map_or(0, |t| t.seconds)
}

/// The servers whose name or owner matches `query`, sorted.
pub fn shown<'a>(servers: &'a [pb::InstanceServer], query: &str, sort: Sort) -> Vec<&'a pb::InstanceServer> {
    let q = query.trim().to_lowercase();
    let mut kept: Vec<&pb::InstanceServer> = servers
        .iter()
        .filter(|s| {
            q.is_empty() || {
                let name = s.server.as_ref().map_or("", |s| s.name.as_str());
                let (user, display) =
                    s.owner.as_ref().map_or(("", ""), |o| (o.username.as_str(), o.display_name.as_str()));
                format!("{name} {user} {display}").to_lowercase().contains(&q)
            }
        })
        .collect();
    match sort {
        Sort::Biggest => kept.sort_by_key(|s| std::cmp::Reverse(storage_of(s))),
        Sort::Members => kept.sort_by_key(|s| std::cmp::Reverse(members_of(s))),
        Sort::Newest => kept.sort_by_key(|s| std::cmp::Reverse(created_of(s))),
    }
    kept
}

/// Whether there's a region to pick: more than one.
pub fn has_regions(regions: &[pb::Region]) -> bool {
    regions.len() > 1
}

/// The region a server is in, from its `region` label (empty for home).
pub fn region_of<'a>(regions: &'a [pb::Region], region: &str) -> Option<&'a pb::Region> {
    regions
        .iter()
        .find(|r| if region.is_empty() { r.home } else { r.id == region })
        .or_else(|| if region.is_empty() { regions.first() } else { None })
}

/// A region's name for people: its own, or the label when the instance doesn't list it.
pub fn region_name(regions: &[pb::Region], region: &str) -> String {
    match region_of(regions, region) {
        Some(r) if !r.name.is_empty() => r.name.clone(),
        _ if !region.is_empty() => region.to_owned(),
        _ => "Home".to_owned(),
    }
}

/// Whether `region` names the same place as the server's `current` one.
pub fn same_region(regions: &[pb::Region], region: &str, current: &str) -> bool {
    let (a, b) = (region_of(regions, region), region_of(regions, current));
    a.map(|r| &r.id) == b.map(|r| &r.id) && (a.is_some() || region == current)
}

/// A small flag-free mark for a region: "EU", "US", or its first letters.
pub fn region_mark(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let first = words.first().copied().unwrap_or("?");
    if (2..=3).contains(&first.len()) && first.chars().all(|c| c.is_ascii_uppercase()) {
        return first.to_owned();
    }
    if words.len() > 1 {
        return words.iter().filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase();
    }
    first.chars().take(2).collect::<String>().to_uppercase()
}

/// The six caps the page changes, by name: (label, bytes?, read, write).
type Read = fn(&pb::ServerLimits) -> Option<i64>;
type Write = fn(&mut pb::ServerLimits, Option<i64>);
pub const CAPS: [(&str, bool, Read, Write); 6] = [
    ("Members", false, |l| l.members, |l, v| l.members = v),
    ("Channels", false, |l| l.channels, |l, v| l.channels = v),
    ("Storage", true, |l| l.storage_bytes, |l, v| l.storage_bytes = v),
    ("Files", true, |l| l.attachment_bytes, |l, v| l.attachment_bytes = v),
    ("Emoji", false, |l| l.emojis, |l, v| l.emojis = v),
    ("Recordings", true, |l| l.recording_bytes, |l, v| l.recording_bytes = v),
];

/// What a server's file is saved as: its name as a slug and the day, like
/// the instance's own name for it ("cozy-corner-2026-10-04.db").
pub fn export_name(name: &str, today: &str) -> String {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_end_matches('-').chars().take(48).collect();
    format!("{}-{today}.db", if slug.is_empty() { "server" } else { &slug })
}

/// Today, as the file name above writes it.
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

impl Core {
    /// Every server here, with its owner, usage and caps. Admins only.
    pub async fn list_instance_servers(&self, key: &str) -> Result<Vec<pb::InstanceServer>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        Ok(rpc!(api.admin(), list_instance_servers(pb::ListInstanceServersRequest {})).await?.servers)
    }

    /// The instance's totals: default caps, and the pictures people keep here.
    pub async fn node_usage(&self, key: &str) -> Result<pb::GetNodeUsageResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.admin(), get_node_usage(pb::GetNodeUsageRequest {})).await
    }

    /// A server's own caps (unset ones follow the instance's defaults).
    pub async fn server_own_limits(&self, key: &str, server_id: &str) -> Result<pb::ServerLimits, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res =
            rpc!(api.servers(), get_server_usage(pb::GetServerUsageRequest { server_id: server_id.into() })).await?;
        Ok(res.own_limits.unwrap_or_default())
    }

    /// Sets a server's own caps; gives back the caps now in force.
    pub async fn set_server_limits(
        &self,
        key: &str,
        server_id: &str,
        limits: pb::ServerLimits,
    ) -> Result<pb::ServerLimits, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.admin(),
            set_server_limits(pb::SetServerLimitsRequest { server_id: server_id.into(), limits: Some(limits) })
        )
        .await?;
        Ok(res.limits.unwrap_or_default())
    }

    /// Moves a server to another region (empty for home).
    pub async fn move_server(&self, key: &str, server_id: &str, region: &str) -> Result<pb::Server, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.admin(),
            move_server(pb::MoveServerRequest { server_id: server_id.into(), region: region.into() })
        )
        .await?;
        res.server.ok_or_else(|| Problem::new(Code::Internal, "The instance sent no server."))
    }

    /// A server's shared channels, both ends.
    pub async fn list_server_shares(&self, key: &str, server_id: &str) -> Result<Vec<pb::SharedConnection>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res =
            rpc!(api.admin(), list_server_shares(pb::ListServerSharesRequest { server_id: server_id.into() })).await?;
        Ok(res.connections)
    }

    /// Ends one of a server's shared channels, from either end.
    pub async fn end_server_share(&self, key: &str, server_id: &str, connection_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.admin(),
            end_server_share(pb::EndServerShareRequest {
                server_id: server_id.into(),
                connection_id: connection_id.into(),
            })
        )
        .await?;
        Ok(())
    }

    /// Deletes a server for everyone.
    pub async fn delete_any_server(&self, key: &str, server_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.servers(), delete_server(pb::DeleteServerRequest { server_id: server_id.into() })).await?;
        self.shared.instance(key, |i| crate::core::store::remove_server(i, server_id));
        Ok(())
    }

    /// Saves a server's whole file to `path` as it arrives, telling
    /// `progress` how far along it is (0 to 1). A half-written file is removed.
    pub async fn export_server(
        &self,
        key: &str,
        server_id: &str,
        path: &Path,
        progress: impl Fn(f32) + Send,
    ) -> Result<u64, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let mut stream = tokio::time::timeout(
            CALL_TIMEOUT,
            api.admin().export_server(pb::ExportServerRequest { server_id: server_id.into() }),
        )
        .await
        .map_err(|_| Problem::new(Code::DeadlineExceeded, "The instance took too long to answer."))?
        .map_err(Problem::from)?
        .into_inner();
        let fail = |e: std::io::Error| Problem::new(Code::Internal, format!("Couldn't save the file: {e}"));
        let mut file = std::fs::File::create(path).map_err(fail)?;
        let (mut bytes, mut total) = (0u64, 0u64);
        let result = async {
            loop {
                let next = tokio::time::timeout(CALL_TIMEOUT, stream.message())
                    .await
                    .map_err(|_| Problem::new(Code::DeadlineExceeded, "The instance stopped sending the file."))?
                    .map_err(Problem::from)?;
                let Some(part) = next else { break };
                if part.size > 0 {
                    total = part.size as u64;
                }
                file.write_all(&part.chunk).map_err(fail)?;
                bytes += part.chunk.len() as u64;
                progress(if total > 0 { (bytes as f32 / total as f32).min(1.0) } else { 0.5 });
            }
            file.flush().map_err(fail)?;
            Ok::<_, Problem>(bytes)
        }
        .await;
        if result.is_err() {
            drop(file);
            let _ = std::fs::remove_file(path);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, owner: &str, storage: i64, members: i64, created: i64) -> pb::InstanceServer {
        pb::InstanceServer {
            server: Some(pb::Server {
                name: name.into(),
                created_at: Some(prost_types::Timestamp { seconds: created, nanos: 0 }),
                ..Default::default()
            }),
            owner: Some(pb::User { username: owner.into(), ..Default::default() }),
            usage: Some(pb::ServerUsage { storage_bytes: storage, members, ..Default::default() }),
            ..Default::default()
        }
    }

    #[test]
    fn search_and_sort_like_the_web() {
        let list = [entry("Cozy", "mika", 10, 5, 1), entry("Busy", "sora", 30, 2, 3), entry("Rust", "mika", 20, 9, 2)];
        let names =
            |v: Vec<&pb::InstanceServer>| v.iter().map(|s| s.server.as_ref().unwrap().name.clone()).collect::<Vec<_>>();
        assert_eq!(names(shown(&list, "", Sort::Biggest)), ["Busy", "Rust", "Cozy"]);
        assert_eq!(names(shown(&list, "", Sort::Members)), ["Rust", "Cozy", "Busy"]);
        assert_eq!(names(shown(&list, "", Sort::Newest)), ["Busy", "Rust", "Cozy"]);
        assert_eq!(names(shown(&list, " MIKA", Sort::Biggest)), ["Rust", "Cozy"]);
    }

    #[test]
    fn regions_read_like_the_web() {
        let regions = vec![
            pb::Region { id: String::new(), name: "United States".into(), home: true },
            pb::Region { id: "eu".into(), name: "Europe".into(), home: false },
        ];
        assert!(has_regions(&regions));
        assert_eq!(region_name(&regions, ""), "United States");
        assert_eq!(region_name(&regions, "eu"), "Europe");
        assert_eq!(region_name(&regions, "ap"), "ap");
        assert!(same_region(&regions, "", ""));
        assert!(!same_region(&regions, "eu", ""));
        assert_eq!(region_mark("United States"), "US");
        assert_eq!(region_mark("EU west"), "EU");
        assert_eq!(region_mark("Europe"), "EU");
        assert_eq!(export_name("Cozy Corner!", "2026-10-04"), "cozy-corner-2026-10-04.db");
        assert_eq!(export_name("☁️", "2026-10-04"), "server-2026-10-04.db");
    }
}
