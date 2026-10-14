//! Pictures and files pasted into a composer (the web's `onPaste`): a
//! screenshot or a picture copied in a browser comes as its bytes, files
//! copied in Finder or Explorer as their paths. They join the files picked
//! with the paperclip, in the same tray with the same spoiler switch and
//! checks; a paste with no files in it is text, for the box as ever.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::{App, ClipboardEntry, ClipboardItem, Context, ImageFormat, Window};

use crate::core::attachments;
use crate::core::sealed_files::Outgoing;
use crate::ui::app::FuwaApp;

/// What a paste carries that goes with the message as a file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Pasted {
    /// A file copied in the system's file manager.
    File(PathBuf),
    /// A picture's bytes, as the clipboard holds them.
    Picture { format: ImageFormat, bytes: Vec<u8> },
}

/// The files in a paste, in order; none means it's text, for the box.
/// Pictures and copied files win over the text beside them (a copied file's
/// name, a picture's caption), as they do in browsers. An SVG is text here:
/// it's markup more often than a picture someone meant to send.
pub(crate) fn files_in(item: &ClipboardItem) -> Vec<Pasted> {
    let mut out = Vec::new();
    for entry in item.entries() {
        match entry {
            ClipboardEntry::Image(image) if image.format != ImageFormat::Svg => {
                out.push(Pasted::Picture { format: image.format, bytes: image.bytes.clone() })
            }
            ClipboardEntry::ExternalPaths(paths) => {
                out.extend(paths.paths().iter().filter(|p| p.is_file()).cloned().map(Pasted::File))
            }
            _ => {}
        }
    }
    if out.is_empty()
        && let Some(text) = item.text()
    {
        let copied = copied_files(&text);
        if copied.iter().all(|p| p.is_file()) {
            out.extend(copied.into_iter().map(Pasted::File));
        }
    }
    out
}

/// Files copied in a Linux file manager, which reach the app as text: their
/// `file://` addresses, a line each (after Nautilus' own two lines, when it
/// adds them). Anything else is text, even a path typed out.
pub(crate) fn copied_files(text: &str) -> Vec<PathBuf> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty()).peekable();
    if lines.peek() == Some(&"x-special/nautilus-clipboard") {
        lines.next();
        if matches!(lines.peek(), Some(&"copy") | Some(&"cut")) {
            lines.next();
        }
    }
    let mut paths = Vec::new();
    for line in lines {
        let path = url::Url::parse(line).ok().filter(|u| u.scheme() == "file").and_then(|u| u.to_file_path().ok());
        match path {
            Some(path) => paths.push(path),
            None => return Vec::new(),
        }
    }
    paths
}

/// A pasted picture as a file to send, named as browsers name one: PNG,
/// JPEG, GIF and WebP as they are, anything else (the bitmap Windows'
/// screenshots leave, a TIFF) made a PNG. None when it can't be read.
pub(crate) fn picture_file(format: ImageFormat, bytes: &[u8]) -> Option<(String, Vec<u8>)> {
    let ext = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Gif => "gif",
        ImageFormat::Webp => "webp",
        _ => {
            let picture = image::load_from_memory(bytes).ok()?;
            let mut png = Vec::new();
            picture.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
            return Some(("image.png".into(), png));
        }
    };
    Some((format!("image.{ext}"), bytes.to_vec()))
}

/// Keeps a pasted picture as a file of its own in the system's temporary
/// folder, so it uploads, shows and can be sent again like a picked one.
fn save_picture(format: ImageFormat, bytes: &[u8]) -> Option<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let (name, bytes) = picture_file(format, bytes)?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let dir = std::env::temp_dir().join("fuwa-pasted").join(format!("{now}-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(name);
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

/// A pasted file or picture as one to seal and send, read here and never
/// written to disk.
fn outgoing(pasted: Pasted) -> Option<Outgoing> {
    let (name, bytes) = match pasted {
        Pasted::File(path) => {
            let bytes = std::fs::read(&path).ok()?;
            (path.file_name()?.to_string_lossy().into_owned(), bytes)
        }
        Pasted::Picture { format, bytes } => picture_file(format, &bytes)?,
    };
    let content_type = attachments::content_type_of(&name).to_owned();
    Some(Outgoing { name, content_type, bytes: Arc::new(bytes), spoiler: false, gif: None })
}

impl FuwaApp {
    /// A composer's paste hook: takes the paste when it carries files, handing
    /// them to `take` once the paste is done, and leaves text to the box.
    fn paste_hook(
        &self,
        cx: &mut Context<Self>,
        take: impl Fn(&mut Self, Vec<Pasted>, &mut Context<Self>) + 'static,
    ) -> impl Fn(&ClipboardItem, &mut Window, &mut App) -> bool + 'static {
        let this = cx.entity().downgrade();
        let take = Rc::new(take);
        move |item, _, cx| {
            let files = files_in(item);
            if files.is_empty() {
                return false;
            }
            let (this, take) = (this.clone(), take.clone());
            cx.defer(move |cx| {
                let _ = this.update(cx, |app, cx| take(app, files, cx));
            });
            true
        }
    }

    /// Pasting into a channel's box (or a card's description): the files join `add_files`.
    pub(crate) fn paste_to_channel(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&ClipboardItem, &mut Window, &mut App) -> bool + 'static {
        self.paste_hook(cx, |this, files, cx| this.add_pasted(files, cx))
    }

    /// Pasting into an encrypted box: the files join the ones picked in `place`, to be sealed.
    pub(crate) fn paste_to_sealed(
        &self,
        place: String,
        cx: &mut Context<Self>,
    ) -> impl Fn(&ClipboardItem, &mut Window, &mut App) -> bool + 'static {
        self.paste_hook(cx, move |this, files, cx| this.add_pasted_sealed(place.clone(), files, cx))
    }

    fn add_pasted(&mut self, files: Vec<Pasted>, cx: &mut Context<Self>) {
        let saved = cx.background_executor().spawn(async move {
            files
                .into_iter()
                .filter_map(|f| match f {
                    Pasted::File(path) => Some(path),
                    Pasted::Picture { format, bytes } => save_picture(format, &bytes),
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let paths = saved.await;
            let _ = this.update(cx, |this, cx| this.add_files(paths, cx));
        })
        .detach();
    }

    fn add_pasted_sealed(&mut self, place: String, files: Vec<Pasted>, cx: &mut Context<Self>) {
        let read = cx.background_executor().spawn(async move { files.into_iter().filter_map(outgoing).collect() });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            let _ = this.update(cx, |this, cx| this.add_picked(place, read, cx));
        })
        .detach();
    }

    /// Files picked or dropped for an encrypted conversation or secure channel, read to be sealed in `place`.
    pub(crate) fn add_sealed_files(&mut self, place: String, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.add_pasted_sealed(place, paths.into_iter().filter(|p| p.is_file()).map(Pasted::File).collect(), cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{ClipboardString, ExternalPaths, Image};

    fn picture(format: ImageFormat, bytes: Vec<u8>) -> ClipboardEntry {
        ClipboardEntry::Image(Image { format, bytes, id: 0 })
    }

    fn png_bytes() -> Vec<u8> {
        let mut out = Vec::new();
        image::DynamicImage::new_rgba8(2, 2)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn text_alone_is_for_the_box() {
        assert!(files_in(&ClipboardItem::new_string("hello".into())).is_empty());
        assert!(files_in(&ClipboardItem::new_string("/home/me/cat.png".into())).is_empty());
    }

    #[test]
    fn a_picture_wins_over_its_caption_and_svg_stays_text() {
        let item = ClipboardItem {
            entries: vec![
                picture(ImageFormat::Png, vec![1, 2, 3]),
                ClipboardEntry::String(ClipboardString::new("a caption".into())),
            ],
        };
        assert_eq!(files_in(&item), vec![Pasted::Picture { format: ImageFormat::Png, bytes: vec![1, 2, 3] }]);
        let svg = ClipboardItem { entries: vec![picture(ImageFormat::Svg, b"<svg/>".to_vec())] };
        assert!(files_in(&svg).is_empty());
    }

    #[test]
    fn every_copied_file_comes_and_folders_dont() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.png"), dir.path().join("b.jpg"));
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let item = ClipboardItem {
            entries: vec![
                ClipboardEntry::ExternalPaths(ExternalPaths(vec![a.clone(), dir.path().to_owned(), b.clone()].into())),
                ClipboardEntry::String(ClipboardString::new("a.png\nb.jpg".into())),
            ],
        };
        assert_eq!(files_in(&item), vec![Pasted::File(a), Pasted::File(b)]);
    }

    #[test]
    fn files_copied_on_linux_arrive_as_their_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("my shot.png");
        std::fs::write(&shot, b"png").unwrap();
        let uri = url::Url::from_file_path(&shot).unwrap().to_string();
        assert!(uri.contains("my%20shot.png"));
        for text in [uri.clone(), format!("x-special/nautilus-clipboard\ncopy\n{uri}\n")] {
            assert_eq!(files_in(&ClipboardItem::new_string(text)), vec![Pasted::File(shot.clone())]);
        }
        // A file that isn't there, or a line that isn't an address, leaves it all as text.
        let gone = format!("{uri}\nfile:///nowhere/at/all.png");
        assert!(files_in(&ClipboardItem::new_string(gone)).is_empty());
        assert!(copied_files(&format!("{uri}\nsome words")).is_empty());
        assert!(copied_files("https://example.com/cat.png").is_empty());
    }

    #[test]
    fn pictures_keep_their_format_and_bitmaps_become_png() {
        let png = png_bytes();
        assert_eq!(picture_file(ImageFormat::Png, &png), Some(("image.png".into(), png.clone())));
        assert_eq!(picture_file(ImageFormat::Jpeg, b"jpeg"), Some(("image.jpg".into(), b"jpeg".to_vec())));
        assert_eq!(picture_file(ImageFormat::Gif, b"gif"), Some(("image.gif".into(), b"gif".to_vec())));
        assert_eq!(picture_file(ImageFormat::Webp, b"webp"), Some(("image.webp".into(), b"webp".to_vec())));

        let mut bmp = Vec::new();
        image::DynamicImage::new_rgb8(3, 2)
            .write_to(&mut std::io::Cursor::new(&mut bmp), image::ImageFormat::Bmp)
            .unwrap();
        let (name, bytes) = picture_file(ImageFormat::Bmp, &bmp).unwrap();
        assert_eq!(name, "image.png");
        let back = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).unwrap();
        assert_eq!((back.width(), back.height()), (3, 2));

        assert_eq!(picture_file(ImageFormat::Tiff, b"not a tiff"), None);
    }

    #[test]
    fn a_pasted_picture_is_saved_to_send_and_sealed_ones_are_read() {
        let png = png_bytes();
        let path = save_picture(ImageFormat::Png, &png).unwrap();
        assert_eq!(path.file_name().unwrap(), "image.png");
        assert_eq!(std::fs::read(&path).unwrap(), png);
        // Two pastes never share a file.
        assert_ne!(save_picture(ImageFormat::Png, &png).unwrap(), path);

        let sealed = outgoing(Pasted::Picture { format: ImageFormat::Png, bytes: png.clone() }).unwrap();
        assert_eq!((sealed.name.as_str(), sealed.content_type.as_str()), ("image.png", "image/png"));
        assert_eq!(*sealed.bytes, png);
        let from_file = outgoing(Pasted::File(path.clone())).unwrap();
        assert_eq!(from_file.name, "image.png");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
