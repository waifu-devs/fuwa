//! What the window draws from: GPUI Kit's icons, plus fuwa's own mark.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

pub struct Assets;

const OWN: &[(&str, &[u8])] = &[
    ("fuwa/mark.svg", include_bytes!("../../assets/fuwa/mark.svg")),
    ("fuwa/face.svg", include_bytes!("../../assets/fuwa/face.svg")),
];

/// A sign-in provider's mark, by its id.
fn brand(id: &str) -> Option<&'static str> {
    Some(match id {
        "google" => {
            "M12.48 10.92v3.28h7.84c-.24 1.84-.853 3.187-1.787 4.133-1.147 1.147-2.933 2.4-6.053 2.4-4.827 0-8.6-3.893-8.6-8.72s3.773-8.72 8.6-8.72c2.6 0 4.507 1.027 5.907 2.347l2.307-2.307C18.747 1.44 16.133 0 12.48 0 5.867 0 .307 5.387.307 12s5.56 12 12.173 12c3.573 0 6.267-1.173 8.373-3.36 2.16-2.16 2.84-5.213 2.84-7.667 0-.76-.053-1.467-.173-2.053H12.48z"
        }
        "x" => {
            "M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-5.214-6.817L4.99 21.75H1.68l7.73-8.835L1.254 2.25H8.08l4.713 6.231zm-1.161 17.52h1.833L7.084 4.126H5.117z"
        }
        "twitch" => {
            "M11.571 4.714h1.715v5.143H11.57zm4.715 0H18v5.143h-1.714zM6 0L1.714 4.286v15.428h5.143V24l4.286-4.286h3.428L22.286 12V0zm14.571 11.143l-3.428 3.428h-3.429l-3 3v-3H6.857V1.714h13.714Z"
        }
        _ => return None,
    })
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = OWN.iter().find(|(p, _)| *p == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        // Sign-in providers' marks (Simple Icons, CC0), as the web draws them.
        if let Some(d) = path.strip_prefix("icons/brand-").and_then(|n| n.strip_suffix(".svg")).and_then(brand) {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="{d}"/></svg>"#
            );
            return Ok(Some(Cow::Owned(svg.into_bytes())));
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
