//! A server's soundboard (`SoundboardService`, docs/soundboard.md): short
//! sounds, uploads on the instance (`MEDIA_PURPOSE_SOUND`, Ogg Opus) counted
//! in the server's attachments, that anyone in one of its voice channels
//! plays for everyone there. Playing one is the exception to "every write
//! pushes an event": like voice states, `SoundPlayed` goes to the `Hub` with
//! sequence 0, scoped by channel, and apps play the file themselves.

use std::collections::HashMap;
use std::sync::Mutex;

use tonic::{Request, Response, Status};

use super::{Api, PictureOwner, Seat, respond, text};
use crate::db::{is_unique_violation, query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb::{self, Permission, soundboard_service_server::SoundboardService};
use crate::servers::{self as store, Audit, Payload, UsageChange};

/// A sound's name, in characters.
const MAX_NAME: usize = 32;

const SOUND_COLUMNS: &str = "id, name, emoji, url, volume, duration_ms, creator_id, size, created_at";

fn sound_row(server_id: &str) -> impl Fn(&turso::Row) -> turso::Result<pb::Sound> + '_ {
    move |r| {
        Ok(pb::Sound {
            id: r.get(0)?,
            server_id: server_id.to_string(),
            name: r.get(1)?,
            emoji: r.get(2)?,
            url: r.get(3)?,
            volume: r.get::<f64>(4)? as f32,
            duration_ms: r.get::<i64>(5)? as i32,
            creator_id: r.get(6)?,
            size: r.get(7)?,
            created_at: Some(timestamp(r.get(8)?)),
        })
    }
}

/// The server's sounds, oldest first.
pub async fn load_sounds(conn: &turso::Connection, server_id: &str) -> Result<Vec<pb::Sound>> {
    query_all(conn, &format!("SELECT {SOUND_COLUMNS} FROM sounds ORDER BY created_at, id"), (), sound_row(server_id))
        .await
}

async fn load_sound(conn: &turso::Connection, server_id: &str, sound_id: &str) -> Result<pb::Sound> {
    query_one(conn, &format!("SELECT {SOUND_COLUMNS} FROM sounds WHERE id = ?1"), [sound_id], sound_row(server_id))
        .await?
        .ok_or(Error::NotFound("sound"))
}

fn checked_name(name: &str) -> Result<String> {
    text("a sound's name", name, 1, MAX_NAME)
}

/// What two names that differ only by case have in common.
fn name_key(name: &str) -> String {
    name.to_lowercase()
}

fn checked_volume(volume: f32) -> Result<f32> {
    if !(0.0..=1.0).contains(&volume) {
        return Err(Error::invalid("a sound's volume is from 0 to 1"));
    }
    Ok(volume)
}

fn name_taken(err: Error, name: &str) -> Error {
    if is_unique_violation(&err) {
        Error::AlreadyExists(format!("there's already a sound called \"{name}\""))
    } else {
        err
    }
}

/// Refuses a name another of the server's sounds has, whatever its case.
async fn check_free(conn: &turso::Connection, name: &str, except: &str) -> Result<()> {
    let taken =
        query_one(conn, "SELECT 1 FROM sounds WHERE name_key = ?1 AND id <> ?2", (name_key(name), except), |r| {
            r.get::<i64>(0)
        })
        .await?;
    match taken {
        Some(_) => Err(Error::AlreadyExists(format!("there's already a sound called \"{name}\""))),
        None => Ok(()),
    }
}

/// A sound's emoji: Unicode, or one of the server's own, or none.
async fn checked_emoji(conn: &turso::Connection, server_id: &str, emoji: &str) -> Result<String> {
    if emoji.trim().is_empty() {
        return Ok(String::new());
    }
    let emojis = store::load_emojis(conn, server_id).await?;
    // A moving emoji is kept as `<:name:id>` too: its id says it moves.
    let emoji = emoji.trim();
    let emoji = emoji.strip_prefix("<a:").map_or_else(|| emoji.to_string(), |rest| format!("<:{rest}"));
    super::join::checked_emoji(&emojis, &emoji, "sound")
}

/// Every sound the server has now, as the event members get.
async fn changed(conn: &turso::Connection, server_id: &str, events: &mut Vec<Payload>) -> Result<()> {
    let sounds = load_sounds(conn, server_id).await?;
    events.push(Payload::SoundsUpdated(pb::SoundsUpdated { sounds }));
    Ok(())
}

/// Plays per account per minute (the instance's `sound_plays_per_minute`,
/// unlimited unless set), kept in memory.
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
        return Err(Error::ResourceExhausted("you're playing sounds too fast; try again in a minute".into()));
    }
    entry.1 += 1;
    Ok(())
}

/// Whether someone in a call, as their voice state says, may be heard
/// playing a sound: nobody plays one muted or deafened, by themselves or a
/// moderator.
fn may_play(state: &pb::VoiceState) -> Result<()> {
    if state.server_mute || state.server_deaf {
        return Err(Error::denied("a moderator muted you in this channel"));
    }
    if state.suppress {
        return Err(crate::permissions::missing(Permission::Speak));
    }
    if state.self_mute || state.self_deaf {
        return Err(Error::FailedPrecondition("unmute to play sounds".into()));
    }
    Ok(())
}

#[tonic::async_trait]
impl SoundboardService for Api {
    async fn list_sounds(
        &self,
        request: Request<pb::ListSoundsRequest>,
    ) -> Result<Response<pb::ListSoundsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, .. } = self.membership(&account, &req.server_id).await?;
                Ok(pb::ListSoundsResponse { sounds: load_sounds(&*sdb.read()?, &sdb.id).await? })
            }
            .await,
        )
    }

    async fn create_sound(
        &self,
        request: Request<pb::CreateSoundRequest>,
    ) -> Result<Response<pb::CreateSoundResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageSoundboard).await?.sdb;
                let name = checked_name(&req.name)?;
                let volume = checked_volume(req.volume.unwrap_or(1.0))?;
                let duration_ms = req.duration_ms.clamp(0, crate::media::MAX_SOUND_MS as i32);
                let upload = self
                    .check_upload(&account, pb::MediaPurpose::Sound, req.url.trim(), Some(&sdb.id))
                    .await?
                    .ok_or_else(|| Error::invalid("upload the sound to this fuwa server first"))?;
                let limits = sdb.limits(&self.app.settings().limits).await?;
                if let Some(limit) = limits.attachment_bytes
                    && sdb.usage().await?.attachment_bytes + upload.size > limit
                {
                    return Err(Error::ResourceExhausted("this server is out of room for files".into()));
                }
                let sound = sdb
                    .write(&account.id, async |conn, events| {
                        check_free(conn, &name, "").await?;
                        let emoji = checked_emoji(conn, &sdb.id, &req.emoji).await?;
                        let now = now_ms();
                        let sound = pb::Sound {
                            id: new_id(),
                            server_id: sdb.id.clone(),
                            name: name.clone(),
                            emoji,
                            url: upload.url.clone(),
                            volume,
                            duration_ms,
                            creator_id: account.id.clone(),
                            size: upload.size,
                            created_at: Some(timestamp(now)),
                        };
                        conn.execute(
                            "INSERT INTO sounds (id, name, name_key, emoji, url, volume, duration_ms, creator_id, size, created_at)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                            (
                                sound.id.as_str(),
                                sound.name.as_str(),
                                name_key(&sound.name),
                                sound.emoji.as_str(),
                                sound.url.as_str(),
                                f64::from(sound.volume),
                                i64::from(sound.duration_ms),
                                sound.creator_id.as_str(),
                                sound.size,
                                now,
                            ),
                        )
                        .await
                        .map_err(|err| name_taken(err.into(), &name))?;
                        store::add_usage(
                            conn,
                            UsageChange { attachments: 1, attachment_bytes: sound.size, ..Default::default() },
                        )
                        .await?;
                        store::audit(
                            conn,
                            &account.id,
                            Audit::new(pb::AuditAction::SoundCreate, &sound.id).change("name", "", &name),
                        )
                        .await?;
                        changed(conn, &sdb.id, events).await?;
                        Ok(sound)
                    })
                    .await?;
                self.keep_picture(Some(&upload.id), Some(&sdb.id)).await;
                Ok(pb::CreateSoundResponse { sound: Some(sound) })
            }
            .await,
        )
    }

    async fn update_sound(
        &self,
        request: Request<pb::UpdateSoundRequest>,
    ) -> Result<Response<pb::UpdateSoundResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageSoundboard).await?.sdb;
                let name = req.name.as_deref().map(checked_name).transpose()?;
                let volume = req.volume.map(checked_volume).transpose()?;
                let sound = sdb
                    .write(&account.id, async |conn, events| {
                        let before = load_sound(conn, &sdb.id, &req.sound_id).await?;
                        let mut sound = before.clone();
                        if let Some(name) = &name {
                            if name_key(name) != name_key(&before.name) {
                                check_free(conn, name, &before.id).await?;
                            }
                            sound.name = name.clone();
                        }
                        if let Some(emoji) = &req.emoji {
                            sound.emoji = checked_emoji(conn, &sdb.id, emoji).await?;
                        }
                        if let Some(volume) = volume {
                            sound.volume = volume;
                        }
                        if sound == before {
                            return Ok(sound);
                        }
                        conn.execute(
                            "UPDATE sounds SET name = ?2, name_key = ?3, emoji = ?4, volume = ?5 WHERE id = ?1",
                            (
                                sound.id.as_str(),
                                sound.name.as_str(),
                                name_key(&sound.name),
                                sound.emoji.as_str(),
                                f64::from(sound.volume),
                            ),
                        )
                        .await
                        .map_err(|err| name_taken(err.into(), &sound.name))?;
                        let entry = Audit::new(pb::AuditAction::SoundUpdate, &sound.id)
                            .change("name", &before.name, &sound.name)
                            .change("emoji", &before.emoji, &sound.emoji)
                            .change("volume", before.volume, sound.volume);
                        store::audit(conn, &account.id, entry).await?;
                        changed(conn, &sdb.id, events).await?;
                        Ok(sound)
                    })
                    .await?;
                Ok(pb::UpdateSoundResponse { sound: Some(sound) })
            }
            .await,
        )
    }

    async fn delete_sound(
        &self,
        request: Request<pb::DeleteSoundRequest>,
    ) -> Result<Response<pb::DeleteSoundResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageSoundboard).await?.sdb;
                let sound = sdb
                    .write(&account.id, async |conn, events| {
                        let sound = load_sound(conn, &sdb.id, &req.sound_id).await?;
                        conn.execute("DELETE FROM sounds WHERE id = ?1", [sound.id.as_str()]).await?;
                        store::add_usage(
                            conn,
                            UsageChange { attachments: -1, attachment_bytes: -sound.size, ..Default::default() },
                        )
                        .await?;
                        let entry = Audit::new(pb::AuditAction::SoundDelete, &sound.id).change("name", &sound.name, "");
                        store::audit(conn, &account.id, entry).await?;
                        changed(conn, &sdb.id, events).await?;
                        Ok(sound)
                    })
                    .await?;
                self.drop_picture(&sound.url, "", PictureOwner::Server(&sdb.id)).await;
                Ok(pb::DeleteSoundResponse {})
            }
            .await,
        )
    }

    async fn play_sound(
        &self,
        request: Request<pb::PlaySoundRequest>,
    ) -> Result<Response<pb::PlaySoundResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.membership(&account, &req.server_id).await?;
                access.require_in(&req.channel_id, Permission::UseSoundboard)?;
                access.require_in(&req.channel_id, Permission::Speak)?;
                let place = self
                    .app
                    .voice
                    .get(&sdb.id, &account.id)
                    .filter(|place| place.state.channel_id == req.channel_id)
                    .ok_or_else(|| Error::FailedPrecondition("join this voice channel to play sounds in it".into()))?;
                may_play(&place.state)?;
                let sound = load_sound(&*sdb.read()?, &sdb.id, &req.sound_id).await?;
                pace(&account.id, now_ms(), self.app.settings().limits.sound_plays_per_minute)?;
                self.app.hub.publish([pb::Event {
                    id: new_id(),
                    server_id: sdb.id.clone(),
                    sequence: 0,
                    actor_id: account.id.clone(),
                    created_at: Some(timestamp(now_ms())),
                    payload: Some(Payload::SoundPlayed(pb::SoundPlayed {
                        channel_id: req.channel_id,
                        user_id: account.id,
                        sound: Some(sound),
                    })),
                }]);
                Ok(pb::PlaySoundResponse {})
            }
            .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_are_paced_per_minute() {
        let id = new_id();
        assert!(pace(&id, 0, None).is_ok());
        assert!(pace(&id, 0, Some(2)).is_ok());
        assert!(pace(&id, 1, Some(2)).is_ok());
        assert!(pace(&id, 2, Some(2)).is_err());
        assert!(pace(&id, 60_000, Some(2)).is_ok());
    }

    #[test]
    fn nobody_plays_muted_or_deafened() {
        let fine = pb::VoiceState::default();
        assert!(may_play(&fine).is_ok());
        for state in [
            pb::VoiceState { self_mute: true, ..Default::default() },
            pb::VoiceState { self_deaf: true, ..Default::default() },
            pb::VoiceState { server_mute: true, ..Default::default() },
            pb::VoiceState { server_deaf: true, ..Default::default() },
            pb::VoiceState { suppress: true, ..Default::default() },
        ] {
            assert!(may_play(&state).is_err());
        }
    }

    #[test]
    fn names_and_volumes_are_checked() {
        assert_eq!(checked_name("  Air horn ").unwrap(), "Air horn");
        assert!(checked_name("").is_err());
        assert!(checked_name(&"a".repeat(33)).is_err());
        assert_eq!(name_key("Air Horn"), name_key("air horn"));
        assert!(checked_volume(0.5).is_ok());
        assert!(checked_volume(1.5).is_err());
        assert!(checked_volume(f32::NAN).is_err());
    }
}
