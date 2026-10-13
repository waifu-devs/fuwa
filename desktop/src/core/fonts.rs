//! Font files of yours for the app's text (`FontPick::File`): copied into the
//! app's `fonts` folder, named by what's in it, like `core::sounds`' files,
//! and loaded into the window's text system by `ui::theme`. Which family a
//! file holds is read from its name table. Nothing about them leaves this
//! computer.

use std::path::{Path, PathBuf};

use crate::core::config::{FontPick, Prefs};
use crate::core::i18n::{Arg, t, t_with};

/// The biggest font file taken (fonts with Japanese or Chinese are big).
pub const MOST_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// The file types offered.
pub const FILE_TYPES: &str = "TTF, OTF, TTC";

/// The folder font files are kept in, from the settings' paths.
static DIR: parking_lot::Mutex<PathBuf> = parking_lot::Mutex::new(PathBuf::new());

/// Remembers where font files are kept (`Paths::fonts`).
pub fn set_dir(dir: &Path) {
    let mut held = DIR.lock();
    if held.as_path() != dir {
        *held = dir.to_owned();
    }
}

/// Where font files are kept.
pub fn dir() -> PathBuf {
    DIR.lock().clone()
}

/// The size cap, as the settings say it.
pub fn most_size() -> String {
    format!("{} MB", MOST_FILE_BYTES / 1024 / 1024)
}

/// A file name this module made: hex, a dot and a short extension, so a
/// hand-edited settings file can't point anywhere else.
pub fn valid_file_name(name: &str) -> bool {
    let Some((stem, ext)) = name.split_once('.') else { return false };
    (8..=64).contains(&stem.len())
        && stem.bytes().all(|b| b.is_ascii_hexdigit())
        && (1..=5).contains(&ext.len())
        && ext.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// The family a font file holds (the first face of a collection): its
/// typographic family (name 16), else its family (name 1), as fontdb and so
/// the text system name it, in English where there's a choice.
pub fn family_of(bytes: &[u8]) -> Option<String> {
    let face = ttf_parser::Face::parse(bytes, 0).ok()?;
    let names = face.names();
    let find = |id: u16| {
        let mut any = None;
        for name in names.into_iter().filter(|n| n.name_id == id && n.is_unicode()) {
            let Some(text) = name.to_string() else { continue };
            let text = text.trim().to_owned();
            if text.is_empty() {
                continue;
            }
            if name.language() == ttf_parser::Language::English_UnitedStates {
                return Some(text);
            }
            any.get_or_insert(text);
        }
        any
    };
    find(ttf_parser::name_id::TYPOGRAPHIC_FAMILY).or_else(|| find(ttf_parser::name_id::FAMILY))
}

/// Copies the font file at `path` into `dir` once it's known to be a font
/// with a family: what to save as the pick, or what went wrong to say.
pub fn import(dir: &Path, path: &Path) -> Result<FontPick, String> {
    use sha2::Digest as _;
    let meta = std::fs::metadata(path).map_err(|_| t("desktop.look.cantRead"))?;
    if meta.len() > MOST_FILE_BYTES {
        return Err(t_with("appsettings.appearance.fontTooBig", &[("size", Arg::Str(&most_size()))]));
    }
    let bytes = std::fs::read(path).map_err(|_| t("desktop.look.cantRead"))?;
    let family = family_of(&bytes)
        .ok_or_else(|| t_with("appsettings.appearance.fontNotFont", &[("types", Arg::Str(FILE_TYPES))]))?;
    let ext: String = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .filter(|e| (1..=5).contains(&e.len()) && e.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
        .unwrap_or_else(|| "font".into());
    let hash = sha2::Sha256::digest(&bytes);
    let file = format!("{}.{ext}", hash.iter().take(12).map(|b| format!("{b:02x}")).collect::<String>());
    let cant_save = || t("appsettings.appearance.fontCantSave");
    std::fs::create_dir_all(dir).map_err(|_| cant_save())?;
    let to = dir.join(&file);
    if !to.exists() {
        // Written beside it and moved in, so a half-written file is never loaded.
        let part = dir.join(format!("{file}.part"));
        std::fs::write(&part, &bytes).and_then(|_| std::fs::rename(&part, &to)).map_err(|_| cant_save())?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().chars().take(100).collect()).unwrap_or_default();
    let family = family.chars().take(100).collect();
    Ok(FontPick::File { file, name, family })
}

/// Deletes the font files neither pick uses any more (at startup, so a file
/// just copied in is never swept before its pick is saved).
pub fn sweep(dir: &Path, prefs: &Prefs) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let used = [prefs.font.file(), prefs.code_font.file()];
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !used.contains(&Some(name.as_str())) && valid_file_name(&name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLED: &[u8] = include_bytes!("../../assets/fonts/MPLUSRounded1c-Regular.ttf");

    #[test]
    fn reads_a_fonts_family() {
        assert_eq!(family_of(BUNDLED).as_deref(), Some("Rounded Mplus 1c"));
        assert_eq!(family_of(b"not a font at all"), None);
        assert_eq!(family_of(&[]), None);
    }

    #[test]
    fn imports_fonts_and_sweeps_the_unused() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("fonts");
        let src = home.path().join("My Font.TTF");
        std::fs::write(&src, BUNDLED).unwrap();
        let pick = import(&dir, &src).unwrap();
        let FontPick::File { file, name, family } = &pick else { panic!("{pick:?}") };
        assert!(valid_file_name(file) && file.ends_with(".ttf"), "{file}");
        assert_eq!((name.as_str(), family.as_str()), ("My Font.TTF", "Rounded Mplus 1c"));
        assert_eq!(std::fs::read(dir.join(file)).unwrap(), BUNDLED);
        // The same font again is the same file.
        assert_eq!(import(&dir, &src).unwrap(), pick);
        // Anything else is turned away, and nothing is kept of it.
        let junk = home.path().join("junk.ttf");
        std::fs::write(&junk, b"definitely not a font").unwrap();
        assert!(import(&dir, &junk).is_err());
        assert!(import(&dir, &home.path().join("gone.ttf")).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        // Sweeping keeps what a pick uses and deletes the rest, but only files this made.
        let stale = dir.join("0123456789abcdef.otf");
        let other = dir.join("notes.txt");
        std::fs::write(&stale, b"x").unwrap();
        std::fs::write(&other, b"x").unwrap();
        let prefs = Prefs { code_font: pick.clone(), ..Prefs::default() };
        sweep(&dir, &prefs);
        assert!(dir.join(file).exists() && other.exists() && !stale.exists());
        sweep(&dir, &Prefs::default());
        assert!(!dir.join(file).exists());
    }
}
