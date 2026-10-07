//! What the window draws from: GPUI Kit's icons, plus fuwa's own mark.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

pub struct Assets;

const OWN: &[(&str, &[u8])] = &[
    ("fuwa/mark.svg", include_bytes!("../../assets/fuwa/mark.svg")),
    ("fuwa/face.svg", include_bytes!("../../assets/fuwa/face.svg")),
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = OWN.iter().find(|(p, _)| *p == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        // Profile effects' shapes, drawn from their paths.
        if let Some(shape) = path
            .strip_prefix("fx/")
            .and_then(|n| n.strip_suffix(".svg"))
            .and_then(crate::core::profile_effects::Shape::by_name)
        {
            return Ok(Some(Cow::Owned(shape.svg().into_bytes())));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut list = gpui_kit::assets::AllAssets.list(path)?;
        list.extend(OWN.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)));
        Ok(list)
    }
}
