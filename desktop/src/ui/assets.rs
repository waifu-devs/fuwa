//! What the window draws from: GPUI Kit's icons, plus fuwa's own mark.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

pub struct Assets;

const OWN: &[(&str, &[u8])] = &[
    ("fuwa/mark.svg", include_bytes!("../../assets/fuwa/mark.svg")),
    ("fuwa/face.svg", include_bytes!("../../assets/fuwa/face.svg")),
    // The marks of the providers people sign in with (Simple Icons, CC0), as the web's `ProviderMarks.tsx`.
    ("providers/google.svg", include_bytes!("../../assets/providers/google.svg")),
    ("providers/x.svg", include_bytes!("../../assets/providers/x.svg")),
    ("providers/twitch.svg", include_bytes!("../../assets/providers/twitch.svg")),
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = OWN.iter().find(|(p, _)| *p == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut list = gpui_kit::assets::AllAssets.list(path)?;
        list.extend(OWN.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)));
        Ok(list)
    }
}
