//! Files attached to messages, as the web app's `lib/attachments.ts`: what
//! each is shown as, uploading one for a server, and saving one to disk.

use std::path::Path;

use bytes::Bytes;
use http_body_util::{BodyExt as _, Empty};
use tokio::io::AsyncWriteExt as _;
use tonic::Code;

use crate::core::Core;
use crate::core::api::Problem;
use crate::pb;
use crate::rpc;

/// At most this many files go with one message, as on the server.
pub const MAX_FILES: usize = 10;
/// The most a file read from disk to upload may be; the instance's own cap is usually lower.
pub const MOST_READ: u64 = 512 * 1024 * 1024;

/// How a file is shown in a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Look {
    Picture,
    Video,
    Audio,
    File,
}

/// What the instance serves as pictures.
const PICTURES: [&str; 5] = ["image/png", "image/jpeg", "image/gif", "image/webp", "image/avif"];

/// How a sent file is shown, by the kind the instance found in its bytes.
pub fn look_of(content_type: &str) -> Look {
    if PICTURES.contains(&content_type) {
        Look::Picture
    } else if content_type.starts_with("video/") {
        Look::Video
    } else if content_type.starts_with("audio/") {
        Look::Audio
    } else {
        Look::File
    }
}

/// What kind of file a name says it is, for its icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    Archive,
    Code,
    Pdf,
    Document,
    Sheet,
    Slides,
    Text,
    Audio,
    Video,
    Picture,
    Other,
}

const FAMILIES: [(Family, &[&str]); 10] = [
    (Family::Archive, &["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "zst"]),
    (
        Family::Code,
        &[
            "js", "ts", "tsx", "jsx", "rs", "go", "py", "rb", "java", "kt", "c", "h", "cpp", "hpp", "cs", "swift",
            "php", "sh", "lua", "json", "toml", "yaml", "yml", "html", "css", "sql", "proto",
        ],
    ),
    (Family::Pdf, &["pdf"]),
    (Family::Document, &["doc", "docx", "odt", "rtf", "pages"]),
    (Family::Sheet, &["xls", "xlsx", "ods", "csv", "tsv", "numbers"]),
    (Family::Slides, &["ppt", "pptx", "odp", "key"]),
    (Family::Text, &["txt", "md", "log", "ini", "cfg"]),
    (Family::Audio, &["mp3", "wav", "flac", "ogg", "opus", "m4a", "aac"]),
    (Family::Video, &["mp4", "webm", "mov", "mkv", "avi"]),
    (Family::Picture, &["png", "jpg", "jpeg", "gif", "webp", "avif", "svg", "heic", "bmp", "tif", "tiff"]),
];

/// A name's extension, lowercased, without the dot; "" when it has none.
pub fn extension_of(name: &str) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot < name.len() - 1 => name[dot + 1..].to_lowercase(),
        _ => String::new(),
    }
}

pub fn family_of(name: &str) -> Family {
    let ext = extension_of(name);
    FAMILIES.iter().find(|(_, exts)| exts.contains(&ext.as_str())).map_or(Family::Other, |(f, _)| *f)
}

/// What a file about to be sent says it is, by its name: the instance decides from its bytes.
pub fn content_type_of(name: &str) -> &'static str {
    match extension_of(name).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "ogg" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        _ => "",
    }
}

/// A file's name without control characters or the marks that flip text's
/// direction (which can make "exe.txt" read as "txt.exe"), as the web and the
/// server clean it.
pub fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{061c}')
        })
        .collect()
}

/// A long name shortened in the middle so its end (the extension, often the
/// part that matters) still shows: "a-very-long-repo…-v2.zip".
pub fn short_name(name: &str, max: usize) -> String {
    let name = clean_name(name);
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max {
        return name;
    }
    let tail = (extension_of(&name).chars().count() + 6).min(max / 2);
    let head: String = chars[..max - tail - 1].iter().collect();
    let end: String = chars[chars.len() - tail..].iter().collect();
    format!("{head}…{end}")
}

/// The box a picture is shown in, at most `max` wide and tall, keeping its shape.
pub fn fit_box(width: i32, height: i32, max: (f32, f32)) -> (f32, f32) {
    if width <= 0 || height <= 0 {
        return (max.0, (max.0 * 0.5625).round());
    }
    let (w, h) = (width as f32, height as f32);
    let scale = (max.0 / w).min(max.1 / h).min(1.0);
    ((w * scale).round().max(1.0), (h * scale).round().max(1.0))
}

/// "2.4 MB".
pub fn format_bytes(n: i64) -> String {
    let n = n.max(0) as f64;
    if n < 1024.0 {
        format!("{n} B")
    } else if n < 1024.0 * 1024.0 {
        format!("{:.0} KB", n / 1024.0)
    } else if n < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", n / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", n / (1024.0 * 1024.0 * 1024.0))
    }
}

/// A picture's size in pixels from its first bytes (PNG, GIF, JPEG or WebP), so
/// others' apps keep its place before it loads.
pub fn picture_size(bytes: &[u8]) -> Option<(i32, i32)> {
    let be16 = |at: usize| Some(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as i32);
    let le16 = |at: usize| Some(u16::from_le_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as i32);
    let be32 = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as i32);
    let le24 = |at: usize| {
        Some(i32::from(*bytes.get(at)?) | i32::from(*bytes.get(at + 1)?) << 8 | i32::from(*bytes.get(at + 2)?) << 16)
    };
    let size = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        (be32(16)?, be32(20)?)
    } else if bytes.starts_with(b"GIF8") {
        (le16(6)?, le16(8)?)
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        let mut at = 2;
        loop {
            if *bytes.get(at)? != 0xff {
                return None;
            }
            let marker = *bytes.get(at + 1)?;
            let len = be16(at + 2)? as usize;
            // A start-of-frame marker (not DHT, JPG or DAC) carries the size.
            if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
                break (be16(at + 7)?, be16(at + 5)?);
            }
            at += 2 + len;
        }
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        match bytes.get(12..16)? {
            b"VP8 " => (le16(26)? & 0x3fff, le16(28)? & 0x3fff),
            b"VP8L" => {
                let b = u32::from_le_bytes(bytes.get(21..25)?.try_into().ok()?);
                ((b & 0x3fff) as i32 + 1, ((b >> 14) & 0x3fff) as i32 + 1)
            }
            b"VP8X" => (le24(24)? + 1, le24(27)? + 1),
            _ => return None,
        }
    } else {
        return None;
    };
    (size.0 > 0 && size.1 > 0).then_some(size)
}

/// Whether a file's link is on this instance's own address: only those load in the app.
pub fn on_instance(url: &str, instance: &str) -> bool {
    match (url::Url::parse(url), url::Url::parse(instance)) {
        (Ok(a), Ok(b)) => a.scheme() == b.scheme() && a.host_str() == b.host_str() && a.port() == b.port(),
        _ => false,
    }
}

impl Core {
    /// Uploads a file from disk to attach to a message in `server_id`.
    pub async fn upload_attachment(&self, key: &str, server_id: &str, path: &Path) -> Result<pb::Attachment, Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(Code::Unavailable, "That instance isn't here."))?;
        let unreadable = || Problem::new(Code::NotFound, "Couldn't read that file.");
        let filename: String =
            clean_name(&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                .chars()
                .take(255)
                .collect();
        let size = tokio::fs::metadata(path).await.map_err(|_| unreadable())?.len();
        if size > MOST_READ {
            return Err(Problem::new(Code::InvalidArgument, "That file is too big to send."));
        }
        let bytes = tokio::fs::read(path).await.map_err(|_| unreadable())?;
        let content_type = content_type_of(&filename);
        let res = rpc!(
            api.media(),
            create_upload(pb::CreateUploadRequest {
                purpose: pb::MediaPurpose::Attachment as i32,
                content_type: content_type.into(),
                size: bytes.len() as i64,
                server_id: server_id.into(),
            })
        )
        .await?;
        let (width, height) = picture_size(&bytes).unwrap_or((0, 0));
        // The bytes go to the instance's own address, whatever name it gave the link.
        let token = res.upload_url.rsplit('/').next().unwrap_or_default();
        let target = format!("{}/media/upload/{token}", api.url.trim_end_matches('/'));
        crate::core::account::send(http::Method::PUT, &target, "application/octet-stream", bytes).await?;
        crate::core::reports::used("upload.attachment");
        let media = res.media.unwrap_or_default();
        Ok(pb::Attachment {
            id: media.id,
            filename,
            content_type: media.content_type,
            size: size as i64,
            url: media.url,
            width,
            height,
            ..Default::default()
        })
    }

    /// Saves an attached file from its instance to `path`, as it streams in.
    /// `size` is the file's size as its message gives it (0 if unknown): a
    /// download that runs past it (or past `MOST_READ`), or stalls, stops.
    pub async fn save_attachment(&self, key: &str, url: &str, size: i64, path: &Path) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(Code::Unavailable, "That instance isn't here."))?;
        let unreachable = || Problem::new(Code::Unavailable, "Couldn't reach this instance right now.");
        let unwritable = || Problem::new(Code::PermissionDenied, "Couldn't save the file there.");
        let too_long = || Problem::new(Code::DeadlineExceeded, "That took too long.");
        // Fetched from the instance this app talks to, whatever address the link names, and only its files.
        let wanted = url::Url::parse(url).map_err(|_| unreachable())?;
        if !wanted.path().starts_with("/media/") {
            return Err(Problem::new(Code::InvalidArgument, "That isn't a file on this instance."));
        }
        let target = format!("{}{}", api.url.trim_end_matches('/'), wanted.path());
        let most = most_saved(size);
        let roots = match hyper_rustls::HttpsConnectorBuilder::new().with_native_roots() {
            Ok(roots) => roots,
            Err(_) => hyper_rustls::HttpsConnectorBuilder::new().with_webpki_roots(),
        };
        let connector = roots.https_or_http().enable_http1().build();
        let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build::<_, Empty<Bytes>>(connector);
        let request = http::Request::get(&target).body(Empty::new()).map_err(|_| unreachable())?;
        let response = tokio::time::timeout(STALL, client.request(request))
            .await
            .map_err(|_| too_long())?
            .map_err(|_| unreachable())?;
        match response.status().as_u16() {
            200 => {}
            404 | 410 => return Err(Problem::new(Code::NotFound, "That file isn't on the instance anymore.")),
            _ => return Err(unreachable()),
        }
        // Written beside the target first, so a cut-off download never looks finished,
        // and never over a file that's already there.
        let (partial, mut file) = open_partial(path).await.ok_or_else(unwritable)?;
        let mut body = response.into_body();
        let result = async {
            let mut written: u64 = 0;
            loop {
                let Some(frame) = tokio::time::timeout(STALL, body.frame()).await.map_err(|_| too_long())? else {
                    break;
                };
                let frame = frame.map_err(|_| unreachable())?;
                if let Some(data) = frame.data_ref() {
                    written += data.len() as u64;
                    if written > most {
                        return Err(Problem::new(Code::OutOfRange, "That file is bigger than its message said."));
                    }
                    file.write_all(data).await.map_err(|_| unwritable())?;
                }
            }
            file.flush().await.map_err(|_| unwritable())?;
            drop(file);
            tokio::fs::rename(&partial, path).await.map_err(|_| unwritable())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&partial).await;
        }
        result?;
        crate::core::reports::used("attachment.download");
        Ok(())
    }
}

/// How long a download may wait for the instance's next bytes before it gives up.
const STALL: std::time::Duration = std::time::Duration::from_secs(60);

/// The most a download may bring: the file's own size and a little more (the
/// instance has the last word on its size), never past `MOST_READ`.
fn most_saved(size: i64) -> u64 {
    match u64::try_from(size) {
        Ok(size) if size > 0 => size.saturating_add(64 * 1024).min(MOST_READ),
        _ => MOST_READ,
    }
}

/// A new file beside `path` for a download in progress: `name.part`, or
/// `name.1.part` and on if that's taken (an earlier download cut off).
async fn open_partial(path: &Path) -> Option<(std::path::PathBuf, tokio::fs::File)> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    for n in 0..10 {
        let part = if n == 0 { format!("{name}.part") } else { format!("{name}.{n}.part") };
        let partial = path.with_file_name(part);
        match tokio::fs::OpenOptions::new().write(true).create_new(true).open(&partial).await {
            Ok(file) => return Some((partial, file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_look_and_group_as_on_the_web() {
        assert_eq!(look_of("image/webp"), Look::Picture);
        assert_eq!(look_of("image/svg+xml"), Look::File);
        assert_eq!(look_of("video/mp4"), Look::Video);
        assert_eq!(look_of("audio/ogg"), Look::Audio);
        assert_eq!(look_of("application/octet-stream"), Look::File);
        assert_eq!(family_of("Report.PDF"), Family::Pdf);
        assert_eq!(family_of("build.tar.gz"), Family::Archive);
        assert_eq!(family_of(".bashrc"), Family::Other);
        assert_eq!(extension_of("a."), "");
        assert_eq!(short_name("a-very-long-repository-name-backup-v2.zip", 24), "a-very-long-re…up-v2.zip");
        assert_eq!(short_name("short.txt", 24), "short.txt");
        assert_eq!(clean_name("photo\u{202e}gnp.exe"), "photognp.exe");
        assert_eq!(clean_name("a\tb\u{2066}c\u{200f}.txt\0"), "abc.txt");
        assert_eq!(short_name("evil\u{202e}txt.exe", 24), "eviltxt.exe");
        assert_eq!(most_saved(100), 100 + 64 * 1024);
        assert_eq!(most_saved(0), MOST_READ);
        assert_eq!(most_saved(i64::MAX), MOST_READ);
        assert_eq!(fit_box(1600, 900, (420.0, 320.0)), (420.0, 236.0));
        assert_eq!(fit_box(0, 0, (420.0, 320.0)), (420.0, 236.0));
        assert_eq!(fit_box(100, 50, (420.0, 320.0)), (100.0, 50.0));
        assert_eq!(format_bytes(2_500_000), "2.4 MB");
        assert!(on_instance("https://fuwa.chat/media/x", "https://fuwa.chat"));
        assert!(!on_instance("https://evil.example/media/x", "https://fuwa.chat"));
    }

    #[test]
    fn picture_sizes_come_from_their_headers() {
        let png = [b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".as_slice(), &3u32.to_be_bytes(), &2u32.to_be_bytes()].concat();
        assert_eq!(picture_size(&png), Some((3, 2)));
        let gif = [b"GIF89a".as_slice(), &[5, 0, 7, 0, 0, 0, 0]].concat();
        assert_eq!(picture_size(&gif), Some((5, 7)));
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 11, 8, 0, 9, 0, 12, 3];
        assert_eq!(picture_size(&jpeg), Some((12, 9)));
        assert_eq!(picture_size(b"not a picture"), None);
    }
}
