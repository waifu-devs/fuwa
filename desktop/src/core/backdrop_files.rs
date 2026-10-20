//! A video or picture of yours behind the app (`Prefs::backdrop_file`, the
//! web's `lib/backdrop-files.ts`), standing in for the backdrop's picture:
//! copied into the app's `backdrops` folder, named by what's in it like
//! `core::fonts`' files, and played from there by `ui::backdrop`, so nothing
//! is fetched to draw it. Nothing about it leaves this computer: the setting
//! that names it isn't synced, and the file is never uploaded.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::config::Prefs;
use crate::core::i18n::{Arg, t, t_with};

/// The biggest file taken, as on the web: the app holds a video in memory while it plays.
pub const MOST_FILE_BYTES: u64 = 100 * 1024 * 1024;

/// What a kept file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Video,
    Image,
}

/// A video or picture kept in the backdrops folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackdropFile {
    /// Its name in the folder (`core::fonts::valid_file_name`).
    pub file: String,
    pub kind: Kind,
    /// The name it had where it was picked, to show.
    #[serde(default)]
    pub name: String,
}

impl BackdropFile {
    /// Where it is, once it's one this app made.
    pub fn path(&self) -> Option<PathBuf> {
        crate::core::fonts::valid_file_name(&self.file).then(|| dir().join(&self.file))
    }
}

/// The folder files are kept in, from the settings' paths.
static DIR: parking_lot::Mutex<PathBuf> = parking_lot::Mutex::new(PathBuf::new());

/// Remembers where files are kept (`Paths::backdrops`).
pub fn set_dir(dir: &Path) {
    let mut held = DIR.lock();
    if held.as_path() != dir {
        *held = dir.to_owned();
    }
}

pub fn dir() -> PathBuf {
    DIR.lock().clone()
}

/// What a file is, by its name: the videos `core::media` plays and the pictures GPUI draws.
pub fn kind_of(name: &str) -> Option<Kind> {
    let (_, ext) = name.rsplit_once('.')?;
    Some(match ext.to_ascii_lowercase().as_str() {
        "mp4" | "m4v" | "mov" | "webm" | "mkv" => Kind::Video,
        "gif" | "png" | "jpg" | "jpeg" | "webp" => Kind::Image,
        _ => return None,
    })
}

/// Copies the video or picture at `path` into `dir` once it's known to be one
/// the app can draw: what to save as the setting, or what went wrong to say.
pub fn import(dir: &Path, path: &Path) -> Result<BackdropFile, String> {
    use sha2::Digest as _;
    let name: String = path.file_name().map(|n| n.to_string_lossy().chars().take(80).collect()).unwrap_or_default();
    let not_media = || t("appsettings.backdrop.deviceNotMedia");
    let kind = kind_of(&name).ok_or_else(not_media)?;
    let meta = std::fs::metadata(path).map_err(|_| t("desktop.look.cantRead"))?;
    if meta.len() > MOST_FILE_BYTES {
        let size = format!("{} MB", MOST_FILE_BYTES / 1024 / 1024);
        return Err(t_with("appsettings.backdrop.deviceTooBig", &[("size", Arg::Str(&size))]));
    }
    let bytes = std::fs::read(path).map_err(|_| t("desktop.look.cantRead"))?;
    let draws = match kind {
        Kind::Video => crate::core::media::Player::open(bytes.as_slice().into(), "", &name)
            .is_ok_and(|player| player.has_picture()),
        Kind::Image => image::guess_format(&bytes).is_ok(),
    };
    if !draws {
        return Err(not_media());
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let hash = sha2::Sha256::digest(&bytes);
    let file = format!("{}.{ext}", hash.iter().take(12).map(|b| format!("{b:02x}")).collect::<String>());
    let cant_save = || t("appsettings.backdrop.deviceCantSave");
    std::fs::create_dir_all(dir).map_err(|_| cant_save())?;
    let to = dir.join(&file);
    if !to.exists() {
        // Written beside it and moved in, so a half-written file is never played.
        let part = dir.join(format!("{file}.part"));
        std::fs::write(&part, &bytes).and_then(|_| std::fs::rename(&part, &to)).map_err(|_| cant_save())?;
    }
    Ok(BackdropFile { file, kind, name })
}

/// Deletes the files the setting doesn't use any more (at startup, so a file
/// just copied in is never swept before the setting is saved).
pub fn sweep(dir: &Path, prefs: &Prefs) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let used = prefs.backdrop_file.as_ref().map(|f| f.file.as_str());
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if used != Some(name.as_str()) && crate::core::fonts::valid_file_name(&name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_videos_from_pictures_by_name() {
        assert_eq!(kind_of("loop.MP4"), Some(Kind::Video));
        assert_eq!(kind_of("a.b.webm"), Some(Kind::Video));
        assert_eq!(kind_of("cat.gif"), Some(Kind::Image));
        assert_eq!(kind_of("notes.txt"), None);
        assert_eq!(kind_of("gif"), None);
    }

    #[test]
    fn imports_pictures_and_sweeps_the_unused() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("backdrops");
        let src = home.path().join("Dance.GIF");
        // The smallest GIF: one transparent pixel.
        let gif: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";
        std::fs::write(&src, gif).unwrap();
        let kept = import(&dir, &src).unwrap();
        assert!(crate::core::fonts::valid_file_name(&kept.file) && kept.file.ends_with(".gif"), "{}", kept.file);
        assert_eq!((kept.kind, kept.name.as_str()), (Kind::Image, "Dance.GIF"));
        assert_eq!(std::fs::read(dir.join(&kept.file)).unwrap(), gif);
        // The same file again is the same copy.
        assert_eq!(import(&dir, &src).unwrap(), kept);
        // Anything else is turned away, and nothing is kept of it.
        let junk = home.path().join("junk.mp4");
        std::fs::write(&junk, b"definitely not a video").unwrap();
        assert!(import(&dir, &junk).is_err());
        assert!(import(&dir, &home.path().join("notes.txt")).is_err());
        assert!(import(&dir, &home.path().join("gone.gif")).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        // Sweeping keeps what the setting uses and deletes the rest, but only files this made.
        let stale = dir.join("0123456789abcdef.webm");
        let other = dir.join("notes.txt");
        std::fs::write(&stale, b"x").unwrap();
        std::fs::write(&other, b"x").unwrap();
        let prefs = Prefs { backdrop_file: Some(kept.clone()), ..Prefs::default() };
        sweep(&dir, &prefs);
        assert!(dir.join(&kept.file).exists() && other.exists() && !stale.exists());
        sweep(&dir, &Prefs::default());
        assert!(!dir.join(&kept.file).exists());
    }
}
