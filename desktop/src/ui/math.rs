//! Formulas in Markdown (marked by `core::math`), drawn by RaTeX: a port of
//! KaTeX to Rust that lays TeX out with KaTeX's own fonts, built into the
//! app, and paints it on the CPU into a picture GPUI shows in the line, the
//! web's `components/Math.tsx`. Nothing is fetched, nothing a formula says
//! opens or runs anything, formulas that define commands aren't drawn
//! (`core::math::allowed`), RaTeX caps macro expansion, and anything bigger
//! than a screen isn't painted: those, and any RaTeX can't read, show their
//! TeX as written.
//!
//! [`InlinePlugin`] draws `$…$` (and `$$…$$` amid text, in display style)
//! where it sits in the line; [`BlockPlugin`] draws a `$$…$$` alone in its
//! paragraph, and ```math / ```latex blocks, centered on a line of their own.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::text::{InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, markdown_ast};
use gpui_kit::{App, IntoElement, ParentElement as _, RenderImage, Rgba, Styled as _, Window, div, img, px};
use ratex_layout::{LayoutOptions, layout, to_display_list};
use ratex_render::{RenderOptions, render_to_png};
use ratex_types::color::Color;
use ratex_types::math_style::MathStyle;

use crate::core::math::{allowed, formula, is_math_lang};

/// KaTeX draws a little bigger than the text around it; the web's `.markdown .katex` is 1.1em.
const SCALE: f32 = 1.1;
/// Room around a formula's ink, in logical pixels.
const PAD: f32 = 1.0;
/// The most a formula may measure, in ems, and in device pixels painted.
const MAX_WIDE: f64 = 80.0;
const MAX_TALL: f64 = 30.0;
const MAX_PIXELS: u64 = 4_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Mode {
    Inline,
    Wide,
    Block,
}

struct Formula {
    tex: String,
    mode: Mode,
}

/// A formula painted: the picture, its size and where its baseline is, in logical pixels.
struct Painted {
    image: Arc<RenderImage>,
    width: f32,
    height: f32,
    baseline: f32,
}

type Key = (String, Mode, u32, u32, u32);

thread_local! {
    /// Formulas painted, by TeX, mode, size, colour and scale: each is painted once.
    static PAINTED: RefCell<HashMap<Key, Option<Arc<Painted>>>> = RefCell::new(HashMap::new());
}

/// Lays out and paints `tex`, or `None` when it can't or mustn't be drawn.
fn paint(tex: &str, mode: Mode, size: f32, color: Rgba, scale: f32) -> Option<Painted> {
    if !allowed(tex) {
        return None;
    }
    let nodes = ratex_parser::parse(tex).ok()?;
    let options =
        LayoutOptions { color: Color { r: color.r, g: color.g, b: color.b, a: color.a }, ..LayoutOptions::default() }
            .with_style(if mode == Mode::Inline { MathStyle::Text } else { MathStyle::Display });
    let list = to_display_list(&layout(&nodes, &options));
    let tall = list.height + list.depth;
    let device = |em: f64| em * f64::from(size * scale) + f64::from(2.0 * PAD * scale);
    if !(list.width.is_finite() && tall.is_finite())
        || list.width > MAX_WIDE
        || tall > MAX_TALL
        || (device(list.width) * device(tall)) as u64 > MAX_PIXELS
    {
        return None;
    }
    let render = RenderOptions {
        font_size: size,
        padding: PAD,
        background_color: Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 },
        font_dir: String::new(),
        device_pixel_ratio: scale,
    };
    let png = render_to_png(&list, &render).ok()?;
    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = decoder.read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut pixels).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    pixels.truncate(info.buffer_size());
    // GPUI's pictures are BGRA.
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(info.width, info.height, pixels)?;
    Some(Painted {
        image: Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])),
        width: info.width as f32 / scale,
        height: info.height as f32 / scale,
        baseline: PAD + list.height as f32 * size,
    })
}

/// [`paint`], remembered.
fn painted(tex: &str, mode: Mode, size: f32, color: Rgba, scale: f32) -> Option<Arc<Painted>> {
    let key = (tex.to_owned(), mode, size.to_bits(), packed(color), scale.to_bits());
    if let Some(hit) = PAINTED.with(|p| p.borrow().get(&key).cloned()) {
        return hit;
    }
    let made = paint(tex, mode, size, color, scale).map(Arc::new);
    PAINTED.with(|p| {
        let mut p = p.borrow_mut();
        if p.len() >= 1000 {
            p.clear();
        }
        p.insert(key, made.clone());
    });
    made
}

/// A colour as one number, for the cache key.
fn packed(c: Rgba) -> u32 {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (byte(c.r) << 24) | (byte(c.g) << 16) | (byte(c.b) << 8) | byte(c.a)
}

/// How a formula reads where it isn't drawn: as it was written.
fn written(tex: &str, mode: Mode) -> String {
    match mode {
        Mode::Inline => format!("${tex}$"),
        Mode::Wide | Mode::Block => format!("$${tex}$$"),
    }
}

fn node(tex: &str, mode: Mode, name: &str) -> MarkdownNode {
    let shown = written(tex, mode);
    MarkdownNode::new(name.to_owned(), Formula { tex: tex.to_owned(), mode }).text(shown.clone()).markdown(shown)
}

/// Draws the marked code spans `core::math::mark` makes where they sit in the line.
pub struct InlinePlugin;

impl gpui_kit::component::text::MarkdownPlugin for InlinePlugin {
    fn name(&self) -> &str {
        "fuwa-math"
    }

    fn parse(&self, md: &markdown_ast::Node, _: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let markdown_ast::Node::InlineCode(code) = md else { return None };
        let (tex, display) = formula(&code.value)?;
        Some(node(tex, if display { Mode::Wide } else { Mode::Inline }, "fuwa-math"))
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        window: &mut Window,
        _: &mut App,
    ) -> Option<InlineElement> {
        let formula = node.data::<Formula>()?;
        let size = f32::from(context.font_size()) * SCALE;
        let color: Rgba = context.text_style().color.into();
        // Not drawn: `None` leaves the text, the TeX as written.
        let painted = painted(&formula.tex, formula.mode, size, color, window.scale_factor())?;
        Some(
            InlineElement::new(img(painted.image.clone()).flex_none().w(px(painted.width)).h(px(painted.height)))
                .with_baseline(px(painted.baseline)),
        )
    }
}

/// Draws a `$$…$$` alone in its paragraph, and ```math / ```latex blocks, centered on their own line.
pub struct BlockPlugin;

impl gpui_kit::component::text::MarkdownPlugin for BlockPlugin {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "fuwa-math-block"
    }

    fn parse(&self, md: &markdown_ast::Node, _: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        match md {
            markdown_ast::Node::Code(code) if code.lang.as_deref().is_some_and(is_math_lang) => {
                let tex = code.value.trim();
                (!tex.is_empty()).then(|| node(tex, Mode::Block, "fuwa-math-block"))
            }
            markdown_ast::Node::Paragraph(paragraph) => {
                let mut parts = paragraph.children.iter().filter(|c| match c {
                    markdown_ast::Node::Text(t) => !t.value.trim().is_empty(),
                    markdown_ast::Node::Break(_) => false,
                    _ => true,
                });
                let (Some(markdown_ast::Node::InlineCode(code)), None) = (parts.next(), parts.next()) else {
                    return None;
                };
                let (tex, true) = formula(&code.value)? else { return None };
                Some(node(tex, Mode::Block, "fuwa-math-block"))
            }
            _ => None,
        }
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = crate::ui::widgets::pal(cx);
        let formula = node.data::<Formula>();
        let size = f32::from(window.rem_size()) * SCALE;
        let painted = formula.and_then(|f| painted(&f.tex, f.mode, size, p.foreground, window.scale_factor()));
        let line = div().w_full().flex().justify_center().overflow_hidden().py(px(2.0));
        match painted {
            Some(painted) => {
                line.child(img(painted.image.clone()).flex_none().w(px(painted.width)).h(px(painted.height)))
            }
            None => line.text_color(p.muted_foreground).child(node.as_text().to_owned()),
        }
    }
}

/// Markdown with formulas, for text that has no other plugins (chat's own,
/// `ui::emoji::markdown_extensions`, include these).
pub fn extensions() -> gpui_kit::component::text::MarkdownExtensions {
    gpui_kit::component::text::MarkdownExtensions::default().plugin(InlinePlugin).plugin(BlockPlugin).parser_revision(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Rgba = Rgba { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };

    #[test]
    fn formulas_paint() {
        let p = paint(r"\frac{-b \pm \sqrt{b^2-4ac}}{2a}", Mode::Inline, 16.5, WHITE, 2.0).expect("painted");
        assert!(p.width > 20.0 && p.height > 16.0);
        assert!(p.baseline > 0.0 && p.baseline < p.height);
        assert_eq!(p.image.size(0).width.0 as f32, (p.width * 2.0).round());
    }

    #[test]
    fn some_formulas_arent_painted() {
        // Defines a command.
        assert!(paint(r"\def\x{y}\x", Mode::Inline, 16.0, WHITE, 1.0).is_none());
        // Bigger than any screen.
        assert!(paint(r"\rule{500em}{500em}", Mode::Inline, 16.0, WHITE, 1.0).is_none());
        // Not TeX RaTeX can read.
        assert!(paint(r"\frac{", Mode::Inline, 16.0, WHITE, 1.0).is_none());
    }
}
