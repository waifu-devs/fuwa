// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AtlasTextureId, AtlasTile, Background, Bounds, ContentMask, Corners, Edges, Hsla, Pixels,
    Point, Radians, ScaledPixels, Size, bounds_tree::BoundsTree, point,
};
use std::{
    fmt::Debug,
    iter::Peekable,
    ops::{Add, Range, Sub},
    slice,
};

#[allow(non_camel_case_types, unused)]
#[expect(missing_docs)]
pub type PathVertex_ScaledPixels = PathVertex<ScaledPixels>;

#[expect(missing_docs)]
pub type DrawOrder = u32;

/// A boolean stored as a `u32` so that GPU-facing structs contain no
/// compiler-inserted padding bytes, which would be undefined behavior to
/// reinterpret as `&[u8]` when writing instance buffers. Guaranteed to be
/// `0` or `1` by construction; shaders read it as a `u32`/`uint`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct PaddedBool32(u32);

impl From<bool> for PaddedBool32 {
    fn from(value: bool) -> Self {
        PaddedBool32(value as u32)
    }
}

#[derive(Default)]
#[expect(missing_docs)]
pub struct Scene {
    pub(crate) paint_operations: Vec<PaintOperation>,
    primitive_bounds: BoundsTree<ScaledPixels>,
    layer_stack: Vec<DrawOrder>,
    pub shadows: Vec<Shadow>,
    pub quads: Vec<Quad>,
    pub paths: Vec<Path<ScaledPixels>>,
    pub underlines: Vec<Underline>,
    pub monochrome_sprites: Vec<MonochromeSprite>,
    pub subpixel_sprites: Vec<SubpixelSprite>,
    pub polychrome_sprites: Vec<PolychromeSprite>,
    pub surfaces: Vec<PaintSurface>,
    pub backdrop_blurs: Vec<BackdropBlur>,
}

#[expect(missing_docs)]
impl Scene {
    pub fn clear(&mut self) {
        self.paint_operations.clear();
        self.primitive_bounds.clear();
        self.layer_stack.clear();
        self.paths.clear();
        self.shadows.clear();
        self.quads.clear();
        self.underlines.clear();
        self.monochrome_sprites.clear();
        self.subpixel_sprites.clear();
        self.polychrome_sprites.clear();
        self.surfaces.clear();
        self.backdrop_blurs.clear();
    }

    pub fn len(&self) -> usize {
        self.paint_operations.len()
    }

    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        let order = self.primitive_bounds.insert(bounds);
        self.layer_stack.push(order);
        self.paint_operations
            .push(PaintOperation::StartLayer(bounds));
    }

    pub fn pop_layer(&mut self) {
        self.layer_stack.pop();
        self.paint_operations.push(PaintOperation::EndLayer);
    }

    pub fn insert_primitive(&mut self, primitive: impl Into<Primitive>) {
        let mut primitive = primitive.into();
        let clipped_bounds = primitive
            .bounds()
            .intersect(&primitive.content_mask().bounds);

        if clipped_bounds.is_empty() {
            return;
        }

        // Draw order is worked out where the primitive lands on screen, after
        // its element transform. A backdrop blur reads around its bounds too.
        let mut drawn_bounds = primitive.element_transform().map_bounds(clipped_bounds);
        if let Primitive::BackdropBlur(blur) = &primitive {
            drawn_bounds = drawn_bounds.dilate(blur.blur_radius * 3.);
        }

        let order = self
            .layer_stack
            .last()
            .copied()
            .unwrap_or_else(|| self.primitive_bounds.insert(drawn_bounds));
        match &mut primitive {
            Primitive::Shadow(shadow) => {
                shadow.order = order;
                self.shadows.push(*shadow);
            }
            Primitive::Quad(quad) => {
                quad.order = order;
                self.quads.push(*quad);
            }
            Primitive::Path(path) => {
                path.order = order;
                path.id = PathId(self.paths.len());
                self.paths.push(path.clone());
            }
            Primitive::Underline(underline) => {
                underline.order = order;
                self.underlines.push(*underline);
            }
            Primitive::MonochromeSprite(sprite) => {
                sprite.order = order;
                self.monochrome_sprites.push(*sprite);
            }
            Primitive::SubpixelSprite(sprite) => {
                sprite.order = order;
                self.subpixel_sprites.push(*sprite);
            }
            Primitive::PolychromeSprite(sprite) => {
                sprite.order = order;
                self.polychrome_sprites.push(*sprite);
            }
            Primitive::Surface(surface) => {
                surface.order = order;
                self.surfaces.push(surface.clone());
            }
            Primitive::BackdropBlur(blur) => {
                blur.order = order;
                self.backdrop_blurs.push(*blur);
            }
        }
        self.paint_operations
            .push(PaintOperation::Primitive(primitive));
    }

    pub fn replay(&mut self, range: Range<usize>, prev_scene: &Scene) {
        for operation in &prev_scene.paint_operations[range] {
            match operation {
                PaintOperation::Primitive(primitive) => self.insert_primitive(primitive.clone()),
                PaintOperation::StartLayer(bounds) => self.push_layer(*bounds),
                PaintOperation::EndLayer => self.pop_layer(),
            }
        }
    }

    pub fn finish(&mut self) {
        self.shadows.sort_by_key(|shadow| shadow.order);
        self.quads.sort_by_key(|quad| quad.order);
        self.paths.sort_by_key(|path| path.order);
        self.underlines.sort_by_key(|underline| underline.order);
        self.monochrome_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.subpixel_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.polychrome_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.surfaces.sort_by_key(|surface| surface.order);
        self.backdrop_blurs.sort_by_key(|blur| blur.order);
    }

    #[cfg_attr(
        all(
            any(target_os = "linux", target_os = "freebsd"),
            not(any(feature = "x11", feature = "wayland"))
        ),
        allow(dead_code)
    )]
    pub fn batches(&self) -> impl Iterator<Item = PrimitiveBatch> + '_ {
        BatchIterator {
            shadows_start: 0,
            shadows_iter: self.shadows.iter().peekable(),
            quads_start: 0,
            quads_iter: self.quads.iter().peekable(),
            paths_start: 0,
            paths_iter: self.paths.iter().peekable(),
            underlines_start: 0,
            underlines_iter: self.underlines.iter().peekable(),
            monochrome_sprites_start: 0,
            monochrome_sprites_iter: self.monochrome_sprites.iter().peekable(),
            subpixel_sprites_start: 0,
            subpixel_sprites_iter: self.subpixel_sprites.iter().peekable(),
            polychrome_sprites_start: 0,
            polychrome_sprites_iter: self.polychrome_sprites.iter().peekable(),
            surfaces_start: 0,
            surfaces_iter: self.surfaces.iter().peekable(),
            backdrop_blurs_start: 0,
            backdrop_blurs_iter: self.backdrop_blurs.iter().peekable(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Default)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveKind {
    // First, so that at the same order a blur reads what's beneath it before
    // anything else of that order is drawn over it.
    BackdropBlur,
    Shadow,
    #[default]
    Quad,
    Path,
    Underline,
    MonochromeSprite,
    SubpixelSprite,
    PolychromeSprite,
    Surface,
}

pub(crate) enum PaintOperation {
    Primitive(Primitive),
    StartLayer(Bounds<ScaledPixels>),
    EndLayer,
}

#[derive(Clone)]
#[expect(missing_docs)]
pub enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Path(Path<ScaledPixels>),
    Underline(Underline),
    MonochromeSprite(MonochromeSprite),
    SubpixelSprite(SubpixelSprite),
    PolychromeSprite(PolychromeSprite),
    Surface(PaintSurface),
    BackdropBlur(BackdropBlur),
}

#[expect(missing_docs)]
impl Primitive {
    pub fn bounds(&self) -> &Bounds<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.bounds,
            Primitive::Quad(quad) => &quad.bounds,
            Primitive::Path(path) => &path.bounds,
            Primitive::Underline(underline) => &underline.bounds,
            Primitive::MonochromeSprite(sprite) => &sprite.bounds,
            Primitive::SubpixelSprite(sprite) => &sprite.bounds,
            Primitive::PolychromeSprite(sprite) => &sprite.bounds,
            Primitive::Surface(surface) => &surface.bounds,
            Primitive::BackdropBlur(blur) => &blur.bounds,
        }
    }

    pub fn content_mask(&self) -> &RoundedMask<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.content_mask,
            Primitive::Quad(quad) => &quad.content_mask,
            Primitive::Path(path) => &path.content_mask,
            Primitive::Underline(underline) => &underline.content_mask,
            Primitive::MonochromeSprite(sprite) => &sprite.content_mask,
            Primitive::SubpixelSprite(sprite) => &sprite.content_mask,
            Primitive::PolychromeSprite(sprite) => &sprite.content_mask,
            Primitive::Surface(surface) => &surface.content_mask,
            Primitive::BackdropBlur(blur) => &blur.content_mask,
        }
    }

    pub fn element_transform(&self) -> &TransformationMatrix {
        match self {
            Primitive::Shadow(shadow) => &shadow.element_transform,
            Primitive::Quad(quad) => &quad.element_transform,
            Primitive::Path(path) => &path.element_transform,
            Primitive::Underline(underline) => &underline.element_transform,
            Primitive::MonochromeSprite(sprite) => &sprite.element_transform,
            Primitive::SubpixelSprite(sprite) => &sprite.element_transform,
            Primitive::PolychromeSprite(sprite) => &sprite.element_transform,
            Primitive::Surface(surface) => &surface.element_transform,
            Primitive::BackdropBlur(blur) => &blur.element_transform,
        }
    }
}

#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
struct BatchIterator<'a> {
    shadows_start: usize,
    shadows_iter: Peekable<slice::Iter<'a, Shadow>>,
    quads_start: usize,
    quads_iter: Peekable<slice::Iter<'a, Quad>>,
    paths_start: usize,
    paths_iter: Peekable<slice::Iter<'a, Path<ScaledPixels>>>,
    underlines_start: usize,
    underlines_iter: Peekable<slice::Iter<'a, Underline>>,
    monochrome_sprites_start: usize,
    monochrome_sprites_iter: Peekable<slice::Iter<'a, MonochromeSprite>>,
    subpixel_sprites_start: usize,
    subpixel_sprites_iter: Peekable<slice::Iter<'a, SubpixelSprite>>,
    polychrome_sprites_start: usize,
    polychrome_sprites_iter: Peekable<slice::Iter<'a, PolychromeSprite>>,
    surfaces_start: usize,
    surfaces_iter: Peekable<slice::Iter<'a, PaintSurface>>,
    backdrop_blurs_start: usize,
    backdrop_blurs_iter: Peekable<slice::Iter<'a, BackdropBlur>>,
}

impl<'a> Iterator for BatchIterator<'a> {
    type Item = PrimitiveBatch;

    fn next(&mut self) -> Option<Self::Item> {
        let mut orders_and_kinds = [
            (
                self.shadows_iter.peek().map(|s| s.order),
                PrimitiveKind::Shadow,
            ),
            (self.quads_iter.peek().map(|q| q.order), PrimitiveKind::Quad),
            (self.paths_iter.peek().map(|q| q.order), PrimitiveKind::Path),
            (
                self.underlines_iter.peek().map(|u| u.order),
                PrimitiveKind::Underline,
            ),
            (
                self.monochrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::MonochromeSprite,
            ),
            (
                self.subpixel_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::SubpixelSprite,
            ),
            (
                self.polychrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::PolychromeSprite,
            ),
            (
                self.surfaces_iter.peek().map(|s| s.order),
                PrimitiveKind::Surface,
            ),
            (
                self.backdrop_blurs_iter.peek().map(|b| b.order),
                PrimitiveKind::BackdropBlur,
            ),
        ];
        orders_and_kinds.sort_by_key(|(order, kind)| (order.unwrap_or(u32::MAX), *kind));

        let first = orders_and_kinds[0];
        let second = orders_and_kinds[1];
        let (batch_kind, max_order_and_kind) = if first.0.is_some() {
            (first.1, (second.0.unwrap_or(u32::MAX), second.1))
        } else {
            return None;
        };

        match batch_kind {
            PrimitiveKind::Shadow => {
                let shadows_start = self.shadows_start;
                let mut shadows_end = shadows_start + 1;
                self.shadows_iter.next();
                while self
                    .shadows_iter
                    .next_if(|shadow| (shadow.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    shadows_end += 1;
                }
                self.shadows_start = shadows_end;
                Some(PrimitiveBatch::Shadows(shadows_start..shadows_end))
            }
            PrimitiveKind::Quad => {
                let quads_start = self.quads_start;
                let mut quads_end = quads_start + 1;
                self.quads_iter.next();
                while self
                    .quads_iter
                    .next_if(|quad| (quad.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    quads_end += 1;
                }
                self.quads_start = quads_end;
                Some(PrimitiveBatch::Quads(quads_start..quads_end))
            }
            PrimitiveKind::Path => {
                let paths_start = self.paths_start;
                let mut paths_end = paths_start + 1;
                self.paths_iter.next();
                while self
                    .paths_iter
                    .next_if(|path| (path.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    paths_end += 1;
                }
                self.paths_start = paths_end;
                Some(PrimitiveBatch::Paths(paths_start..paths_end))
            }
            PrimitiveKind::Underline => {
                let underlines_start = self.underlines_start;
                let mut underlines_end = underlines_start + 1;
                self.underlines_iter.next();
                while self
                    .underlines_iter
                    .next_if(|underline| (underline.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    underlines_end += 1;
                }
                self.underlines_start = underlines_end;
                Some(PrimitiveBatch::Underlines(underlines_start..underlines_end))
            }
            PrimitiveKind::MonochromeSprite => {
                let texture_id = self.monochrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.monochrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.monochrome_sprites_iter.next();
                while self
                    .monochrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.monochrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::SubpixelSprite => {
                let texture_id = self.subpixel_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.subpixel_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.subpixel_sprites_iter.next();
                while self
                    .subpixel_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.subpixel_sprites_start = sprites_end;
                Some(PrimitiveBatch::SubpixelSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::PolychromeSprite => {
                let texture_id = self.polychrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.polychrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.polychrome_sprites_iter.next();
                while self
                    .polychrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.polychrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::Surface => {
                let surfaces_start = self.surfaces_start;
                let mut surfaces_end = surfaces_start + 1;
                self.surfaces_iter.next();
                while self
                    .surfaces_iter
                    .next_if(|surface| (surface.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    surfaces_end += 1;
                }
                self.surfaces_start = surfaces_end;
                Some(PrimitiveBatch::Surfaces(surfaces_start..surfaces_end))
            }
            PrimitiveKind::BackdropBlur => {
                let blurs_start = self.backdrop_blurs_start;
                let mut blurs_end = blurs_start + 1;
                self.backdrop_blurs_iter.next();
                while self
                    .backdrop_blurs_iter
                    .next_if(|blur| (blur.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    blurs_end += 1;
                }
                self.backdrop_blurs_start = blurs_end;
                Some(PrimitiveBatch::BackdropBlurs(blurs_start..blurs_end))
            }
        }
    }
}

#[derive(Debug)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
#[allow(missing_docs)]
pub enum PrimitiveBatch {
    Shadows(Range<usize>),
    Quads(Range<usize>),
    Paths(Range<usize>),
    Underlines(Range<usize>),
    MonochromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    SubpixelSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    PolychromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    Surfaces(Range<usize>),
    /// Each one blurs what's been drawn so far beneath it, so renderers end
    /// their pass to read it back.
    BackdropBlurs(Range<usize>),
}

impl PrimitiveBatch {
    #[expect(missing_docs)]
    pub fn label(&self) -> String {
        match self {
            Self::Shadows(range) => format!("shadows ({})", range.len()),
            Self::Quads(range) => format!("quads ({})", range.len()),
            Self::Paths(range) => format!("paths ({})", range.len()),
            Self::Underlines(range) => format!("underlines ({})", range.len()),
            Self::MonochromeSprites { texture_id, range } => {
                format!(
                    "monochrome sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::SubpixelSprites { texture_id, range } => {
                format!(
                    "subpixel sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::PolychromeSprites { texture_id, range } => {
                format!(
                    "polychrome sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::Surfaces(range) => format!("surfaces ({})", range.len()),
            Self::BackdropBlurs(range) => format!("backdrop blurs ({})", range.len()),
        }
    }
}

#[derive(Default, Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Quad {
    pub order: DrawOrder,
    pub border_style: BorderStyle,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: RoundedMask<ScaledPixels>,
    pub background: Background,
    pub border_color: Hsla,
    pub corner_radii: Corners<ScaledPixels>,
    pub border_widths: Edges<ScaledPixels>,
    pub element_transform: TransformationMatrix,
}

impl From<Quad> for Primitive {
    fn from(quad: Quad) -> Self {
        Primitive::Quad(quad)
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Underline {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: RoundedMask<ScaledPixels>,
    pub color: Hsla,
    pub thickness: ScaledPixels,
    pub wavy: PaddedBool32,
    pub element_transform: TransformationMatrix,
}

impl From<Underline> for Primitive {
    fn from(underline: Underline) -> Self {
        Primitive::Underline(underline)
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Shadow {
    pub order: DrawOrder,
    pub blur_radius: ScaledPixels,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: RoundedMask<ScaledPixels>,
    pub color: Hsla,
    pub element_bounds: Bounds<ScaledPixels>,
    pub element_corner_radii: Corners<ScaledPixels>,
    /// 0 = drop shadow (rendered outside the element), 1 = inset shadow (rendered inside).
    pub inset: u32,
    pub pad: u32, // align to 8 bytes
    pub element_transform: TransformationMatrix,
}

impl From<Shadow> for Primitive {
    fn from(shadow: Shadow) -> Self {
        Primitive::Shadow(shadow)
    }
}

/// The style of a border.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub enum BorderStyle {
    /// A solid border.
    #[default]
    Solid = 0,
    /// A dashed border.
    Dashed = 1,
}

/// A data type representing a 2 dimensional transformation that can be applied to an element.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct TransformationMatrix {
    /// 2x2 matrix containing rotation and scale,
    /// stored row-major
    pub rotation_scale: [[f32; 2]; 2],
    /// translation vector
    pub translation: [f32; 2],
}

impl Eq for TransformationMatrix {}

impl TransformationMatrix {
    /// The unit matrix, has no effect.
    pub fn unit() -> Self {
        Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [0.0, 0.0],
        }
    }

    /// Move the origin by a given point
    pub fn translate(mut self, point: Point<ScaledPixels>) -> Self {
        self.compose(Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [point.x.0, point.y.0],
        })
    }

    /// Clockwise rotation in radians around the origin
    pub fn rotate(self, angle: Radians) -> Self {
        self.compose(Self {
            rotation_scale: [
                [angle.0.cos(), -angle.0.sin()],
                [angle.0.sin(), angle.0.cos()],
            ],
            translation: [0.0, 0.0],
        })
    }

    /// Scale around the origin
    pub fn scale(self, size: Size<f32>) -> Self {
        self.compose(Self {
            rotation_scale: [[size.width, 0.0], [0.0, size.height]],
            translation: [0.0, 0.0],
        })
    }

    /// Perform matrix multiplication with another transformation
    /// to produce a new transformation that is the result of
    /// applying both transformations: first, `other`, then `self`.
    #[inline]
    pub fn compose(self, other: TransformationMatrix) -> TransformationMatrix {
        if other == Self::unit() {
            return self;
        }
        // Perform matrix multiplication
        TransformationMatrix {
            rotation_scale: [
                [
                    self.rotation_scale[0][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][0],
                    self.rotation_scale[0][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][1],
                ],
                [
                    self.rotation_scale[1][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][0],
                    self.rotation_scale[1][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][1],
                ],
            ],
            translation: [
                self.translation[0]
                    + self.rotation_scale[0][0] * other.translation[0]
                    + self.rotation_scale[0][1] * other.translation[1],
                self.translation[1]
                    + self.rotation_scale[1][0] * other.translation[0]
                    + self.rotation_scale[1][1] * other.translation[1],
            ],
        }
    }

    /// Apply transformation to a point, mainly useful for debugging
    pub fn apply(&self, point: Point<Pixels>) -> Point<Pixels> {
        let input = [point.x.0, point.y.0];
        let mut output = self.translation;
        for (i, output_cell) in output.iter_mut().enumerate() {
            for (k, input_cell) in input.iter().enumerate() {
                *output_cell += self.rotation_scale[i][k] * *input_cell;
            }
        }
        Point::new(output[0].into(), output[1].into())
    }
}

impl Default for TransformationMatrix {
    fn default() -> Self {
        Self::unit()
    }
}

impl TransformationMatrix {
    /// Whether this is the unit matrix, which changes nothing.
    pub fn is_unit(&self) -> bool {
        *self == Self::unit()
    }

    /// Whether this only scales (by positive amounts) and moves, so a
    /// rectangle stays a rectangle.
    pub fn is_axis_aligned(&self) -> bool {
        self.rotation_scale[0][1] == 0.
            && self.rotation_scale[1][0] == 0.
            && self.rotation_scale[0][0] > 0.
            && self.rotation_scale[1][1] > 0.
    }

    /// The transform that undoes this one, unless it squashes everything
    /// flat (a scale of zero).
    pub fn inverse(&self) -> Option<Self> {
        let [[a, b], [c, d]] = self.rotation_scale;
        let determinant = a * d - b * c;
        if determinant.abs() < f32::EPSILON {
            return None;
        }
        let rotation_scale = [
            [d / determinant, -b / determinant],
            [-c / determinant, a / determinant],
        ];
        let [x, y] = self.translation;
        Some(Self {
            rotation_scale,
            translation: [
                -(rotation_scale[0][0] * x + rotation_scale[0][1] * y),
                -(rotation_scale[1][0] * x + rotation_scale[1][1] * y),
            ],
        })
    }

    /// The same transform with its translation multiplied by `factor`, to go
    /// from logical pixels to device pixels.
    pub fn scale_translation(self, factor: f32) -> Self {
        Self {
            rotation_scale: self.rotation_scale,
            translation: [self.translation[0] * factor, self.translation[1] * factor],
        }
    }

    fn apply_f32(&self, x: f32, y: f32) -> (f32, f32) {
        let m = &self.rotation_scale;
        (
            self.translation[0] + m[0][0] * x + m[0][1] * y,
            self.translation[1] + m[1][0] * x + m[1][1] * y,
        )
    }

    /// The smallest axis-aligned bounds holding `bounds` once transformed.
    pub fn map_bounds<P>(&self, bounds: Bounds<P>) -> Bounds<P>
    where
        P: Clone + Debug + Default + PartialEq + Copy + Into<f32> + From<f32>,
    {
        if self.is_unit() {
            return bounds;
        }
        let left: f32 = bounds.origin.x.into();
        let top: f32 = bounds.origin.y.into();
        let right = left + Into::<f32>::into(bounds.size.width);
        let bottom = top + Into::<f32>::into(bounds.size.height);
        let corners = [
            self.apply_f32(left, top),
            self.apply_f32(right, top),
            self.apply_f32(right, bottom),
            self.apply_f32(left, bottom),
        ];
        let (mut min_x, mut min_y) = corners[0];
        let (mut max_x, mut max_y) = corners[0];
        for (x, y) in &corners[1..] {
            min_x = min_x.min(*x);
            min_y = min_y.min(*y);
            max_x = max_x.max(*x);
            max_y = max_y.max(*y);
        }
        Bounds {
            origin: Point {
                x: P::from(min_x),
                y: P::from(min_y),
            },
            size: Size {
                width: P::from(max_x - min_x),
                height: P::from(max_y - min_y),
            },
        }
    }

    /// Transforms a point.
    pub fn apply_point<P>(&self, point: Point<P>) -> Point<P>
    where
        P: Clone + Debug + Default + PartialEq + Copy + Into<f32> + From<f32>,
    {
        let (x, y) = self.apply_f32(point.x.into(), point.y.into());
        Point {
            x: P::from(x),
            y: P::from(y),
        }
    }
}

/// A content mask as primitives carry it: a rectangle, rounded at its corners
/// when an element clips its children to its own rounded shape. Both are in
/// the primitive's own space, before its element transform.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
#[repr(C)]
pub struct RoundedMask<P: Clone + Debug + Default + PartialEq> {
    /// The rectangle.
    pub bounds: Bounds<P>,
    /// The radius of each corner, zero for square ones.
    pub corner_radii: Corners<P>,
}

impl<P> RoundedMask<P>
where
    P: Clone
        + Debug
        + Default
        + PartialEq
        + PartialOrd
        + Copy
        + Into<f32>
        + From<f32>
        + Add<P, Output = P>
        + Sub<P, Output = P>,
{
    /// A plain rectangle.
    pub fn rect(bounds: Bounds<P>) -> Self {
        Self {
            bounds,
            corner_radii: Corners::default(),
        }
    }

    /// The rectangle alone, as a [`ContentMask`].
    pub fn content_mask(&self) -> ContentMask<P> {
        ContentMask {
            bounds: self.bounds,
        }
    }

    /// Whether any corner is rounded.
    pub fn is_rounded(&self) -> bool {
        corner_values(&self.corner_radii)
            .iter()
            .any(|radius| *radius > 0.)
    }

    /// What both masks let through, as one rounded rectangle.
    ///
    /// The rectangle is the overlap. Each of its corners keeps the rounding of
    /// whichever mask comes nearest to cutting it: where a corner is one of a
    /// mask's own corners, that mask's radius; where it sits just inside a
    /// mask's rounded corner (a scroll area inset by a border inside a rounded
    /// card), the radius that keeps it inside, `radius - inset`; and nothing
    /// from a mask whose rounding it's clear of. That's exact for the usual
    /// nestings (an inner rectangle sharing the outer's corners, or inset the
    /// same amount on both sides) and close for the rest, which can only come
    /// out a little more rounded than the true overlap.
    pub fn intersect(&self, other: &Self) -> Self {
        let bounds = self.bounds.intersect(&other.bounds);
        if !(self.is_rounded() || other.is_rounded()) {
            return Self::rect(bounds);
        }
        let rect = Rect::of(&bounds);
        if rect.right <= rect.left || rect.bottom <= rect.top {
            return Self::rect(bounds);
        }
        let mut radii = [0f32; 4];
        for mask in [self, other] {
            let outer = Rect::of(&mask.bounds);
            let mask_radii = corner_values(&mask.corner_radii);
            // Each corner's distance from the mask's same corner, on each axis.
            let insets = [
                (rect.left - outer.left, rect.top - outer.top),
                (outer.right - rect.right, rect.top - outer.top),
                (outer.right - rect.right, outer.bottom - rect.bottom),
                (rect.left - outer.left, outer.bottom - rect.bottom),
            ];
            for (corner, (dx, dy)) in insets.into_iter().enumerate() {
                let radius = mask_radii[corner];
                if radius > 0. && dx < radius && dy < radius {
                    radii[corner] = radii[corner].max(radius - dx.min(dy).max(0.));
                }
            }
        }
        let largest = (rect.right - rect.left).min(rect.bottom - rect.top) / 2.;
        Self {
            bounds,
            corner_radii: Corners {
                top_left: P::from(radii[0].min(largest)),
                top_right: P::from(radii[1].min(largest)),
                bottom_right: P::from(radii[2].min(largest)),
                bottom_left: P::from(radii[3].min(largest)),
            },
        }
    }

    /// The mask seen through `transform`, as the smallest rounded rectangle
    /// holding it: exact when the transform only scales and moves, and the
    /// square bounding box of the turned shape when it rotates.
    pub fn transformed(&self, transform: &TransformationMatrix) -> Self {
        if transform.is_unit() {
            return *self;
        }
        let bounds = transform.map_bounds(self.bounds);
        if !transform.is_axis_aligned() {
            return Self::rect(bounds);
        }
        let factor = transform.rotation_scale[0][0].min(transform.rotation_scale[1][1]);
        let scale = |radius: P| P::from(Into::<f32>::into(radius) * factor);
        Self {
            bounds,
            corner_radii: Corners {
                top_left: scale(self.corner_radii.top_left),
                top_right: scale(self.corner_radii.top_right),
                bottom_right: scale(self.corner_radii.bottom_right),
                bottom_left: scale(self.corner_radii.bottom_left),
            },
        }
    }
}

impl RoundedMask<Pixels> {
    /// Scale the mask's pixel units by the given scaling factor.
    pub fn scale(&self, factor: f32) -> RoundedMask<ScaledPixels> {
        RoundedMask {
            bounds: self.bounds.scale(factor),
            corner_radii: self.corner_radii.scale(factor),
        }
    }
}

impl<P: Clone + Debug + Default + PartialEq> From<ContentMask<P>> for RoundedMask<P> {
    fn from(mask: ContentMask<P>) -> Self {
        Self {
            bounds: mask.bounds,
            corner_radii: Corners::default(),
        }
    }
}

struct Rect {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl Rect {
    fn of<P>(bounds: &Bounds<P>) -> Self
    where
        P: Clone + Debug + Default + PartialEq + Copy + Into<f32>,
    {
        let left: f32 = bounds.origin.x.into();
        let top: f32 = bounds.origin.y.into();
        Rect {
            left,
            top,
            right: left + Into::<f32>::into(bounds.size.width),
            bottom: top + Into::<f32>::into(bounds.size.height),
        }
    }
}

fn corner_values<P>(corners: &Corners<P>) -> [f32; 4]
where
    P: Clone + Debug + Default + PartialEq + Copy + Into<f32>,
{
    [
        corners.top_left.into(),
        corners.top_right.into(),
        corners.bottom_right.into(),
        corners.bottom_left.into(),
    ]
}

/// A blur of whatever was drawn beneath an element, like CSS's
/// `backdrop-filter: blur()`: renderers read back the pixels around
/// `bounds`, blur them and draw them back inside its rounded shape.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
#[expect(missing_docs)]
pub struct BackdropBlur {
    pub order: DrawOrder,
    /// The blur's standard deviation, as CSS takes it.
    pub blur_radius: ScaledPixels,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: RoundedMask<ScaledPixels>,
    pub element_transform: TransformationMatrix,
    pub opacity: f32,
    pub pad: u32, // align to 8 bytes
}

impl From<BackdropBlur> for Primitive {
    fn from(blur: BackdropBlur) -> Self {
        Primitive::BackdropBlur(blur)
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct MonochromeSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: RoundedMask<ScaledPixels>,
    pub color: Hsla,
    pub tile: AtlasTile,
    /// The sprite's own transform (a turned icon), within its element.
    pub transformation: TransformationMatrix,
    pub element_transform: TransformationMatrix,
}

impl From<MonochromeSprite> for Primitive {
    fn from(sprite: MonochromeSprite) -> Self {
        Primitive::MonochromeSprite(sprite)
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct SubpixelSprite {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: RoundedMask<ScaledPixels>,
    pub color: Hsla,
    pub tile: AtlasTile,
    /// The sprite's own transform (a turned icon), within its element.
    pub transformation: TransformationMatrix,
    pub element_transform: TransformationMatrix,
}

impl From<SubpixelSprite> for Primitive {
    fn from(sprite: SubpixelSprite) -> Self {
        Primitive::SubpixelSprite(sprite)
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PolychromeSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub grayscale: PaddedBool32,
    pub opacity: f32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: RoundedMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub tile: AtlasTile,
    pub element_transform: TransformationMatrix,
}

impl From<PolychromeSprite> for Primitive {
    fn from(sprite: PolychromeSprite) -> Self {
        Primitive::PolychromeSprite(sprite)
    }
}

#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub struct PaintSurface {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: RoundedMask<ScaledPixels>,
    pub element_transform: TransformationMatrix,
    #[cfg(target_os = "macos")]
    pub image_buffer: core_video::pixel_buffer::CVPixelBuffer,
}

impl From<PaintSurface> for Primitive {
    fn from(surface: PaintSurface) -> Self {
        Primitive::Surface(surface)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[expect(missing_docs)]
pub struct PathId(pub usize);

/// A line made up of a series of vertices and control points.
#[derive(Clone, Debug)]
#[expect(missing_docs)]
pub struct Path<P: Clone + Debug + Default + PartialEq> {
    pub id: PathId,
    pub order: DrawOrder,
    pub bounds: Bounds<P>,
    pub content_mask: RoundedMask<P>,
    /// Where the path's space lands in the window. Set when it's painted.
    pub element_transform: TransformationMatrix,
    pub vertices: Vec<PathVertex<P>>,
    pub color: Background,
    start: Point<P>,
    current: Point<P>,
    contour_count: usize,
}

impl Path<Pixels> {
    /// Create a new path with the given starting point.
    pub fn new(start: Point<Pixels>) -> Self {
        Self {
            id: PathId(0),
            order: DrawOrder::default(),
            vertices: Vec::new(),
            start,
            current: start,
            bounds: Bounds {
                origin: start,
                size: Default::default(),
            },
            content_mask: Default::default(),
            element_transform: TransformationMatrix::unit(),
            color: Default::default(),
            contour_count: 0,
        }
    }

    /// Scale this path by the given factor.
    pub fn scale(&self, factor: f32) -> Path<ScaledPixels> {
        Path {
            id: self.id,
            order: self.order,
            bounds: self.bounds.scale(factor),
            content_mask: self.content_mask.scale(factor),
            element_transform: self.element_transform.scale_translation(factor),
            vertices: self
                .vertices
                .iter()
                .map(|vertex| vertex.scale(factor))
                .collect(),
            start: self.start.map(|start| start.scale(factor)),
            current: self.current.scale(factor),
            contour_count: self.contour_count,
            color: self.color,
        }
    }

    /// Move the start, current point to the given point.
    pub fn move_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        self.start = to;
        self.current = to;
    }

    /// Draw a straight line from the current point to the given point.
    pub fn line_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }
        self.current = to;
    }

    /// Draw a curve from the current point to the given point, using the given control point.
    pub fn curve_to(&mut self, to: Point<Pixels>, ctrl: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }

        self.push_triangle(
            (self.current, ctrl, to),
            (point(0., 0.), point(0.5, 0.), point(1., 1.)),
        );
        self.current = to;
    }

    /// Push a triangle to the Path.
    pub fn push_triangle(
        &mut self,
        xy: (Point<Pixels>, Point<Pixels>, Point<Pixels>),
        st: (Point<f32>, Point<f32>, Point<f32>),
    ) {
        self.bounds = self
            .bounds
            .union(&Bounds {
                origin: xy.0,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.1,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.2,
                size: Default::default(),
            });

        self.vertices.push(PathVertex {
            xy_position: xy.0,
            st_position: st.0,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.1,
            st_position: st.1,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.2,
            st_position: st.2,
            content_mask: Default::default(),
        });
    }
}

impl<T> Path<T>
where
    T: Clone + Debug + Default + PartialEq + PartialOrd + Add<T, Output = T> + Sub<Output = T>,
{
    #[allow(unused)]
    #[expect(missing_docs)]
    pub fn clipped_bounds(&self) -> Bounds<T> {
        self.bounds.intersect(&self.content_mask.bounds)
    }
}

impl Path<ScaledPixels> {
    /// Where the clipped path lands in the window, after its element
    /// transform: what renderers composite it from.
    pub fn drawn_bounds(&self) -> Bounds<ScaledPixels> {
        if self.element_transform.is_unit() {
            return self.clipped_bounds();
        }
        // Whole pixels around anything turned or scaled, so its edges stay in.
        let bounds = self.element_transform.map_bounds(self.clipped_bounds());
        Bounds::from_corners(
            point(bounds.left().floor(), bounds.top().floor()),
            point(bounds.right().ceil(), bounds.bottom().ceil()),
        )
    }
}

impl From<Path<ScaledPixels>> for Primitive {
    fn from(path: Path<ScaledPixels>) -> Self {
        Primitive::Path(path)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PathVertex<P: Clone + Debug + Default + PartialEq> {
    pub xy_position: Point<P>,
    pub st_position: Point<f32>,
    pub content_mask: ContentMask<P>,
}

#[expect(missing_docs)]
impl PathVertex<Pixels> {
    pub fn scale(&self, factor: f32) -> PathVertex<ScaledPixels> {
        PathVertex {
            xy_position: self.xy_position.scale(factor),
            st_position: self.st_position,
            content_mask: self.content_mask.scale(factor),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{px, size};

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    #[test]
    fn transforms_invert_and_map_bounds() {
        let transform = TransformationMatrix::unit()
            .translate(point(ScaledPixels(10.), ScaledPixels(20.)))
            .rotate(Radians(0.3))
            .scale(Size {
                width: 2.,
                height: 0.5,
            });
        let inverse = transform.inverse().unwrap();
        let start = point(px(7.), px(-3.));
        let back = inverse.apply_point(transform.apply_point(start));
        assert!((back.x.0 - 7.).abs() < 1e-4 && (back.y.0 + 3.).abs() < 1e-4);

        let flat = TransformationMatrix::unit().scale(Size {
            width: 0.,
            height: 1.,
        });
        assert!(flat.inverse().is_none());

        let moved = TransformationMatrix::unit()
            .translate(point(ScaledPixels(5.), ScaledPixels(5.)))
            .scale(Size {
                width: 2.,
                height: 2.,
            });
        assert_eq!(
            moved.map_bounds(rect(0., 0., 10., 10.)),
            rect(5., 5., 20., 20.)
        );
        assert!(moved.is_axis_aligned());
        assert!(!transform.is_axis_aligned());
    }

    #[test]
    fn rounded_masks_intersect() {
        let card = RoundedMask {
            bounds: rect(0., 0., 100., 100.),
            corner_radii: Corners::all(px(12.)),
        };
        // Sharing the card's corners keeps its rounding.
        let same = card.intersect(&RoundedMask::rect(rect(-10., -10., 200., 200.)));
        assert_eq!(same, card);
        // Clear of the rounding, nothing changes.
        let inner = RoundedMask::rect(rect(20., 20., 60., 60.));
        assert_eq!(card.intersect(&inner), inner);
        // A strip across the top keeps the top corners only.
        let top = card.intersect(&RoundedMask::rect(rect(0., 0., 100., 20.)));
        assert_eq!(top.corner_radii.top_left, px(12.));
        assert_eq!(top.corner_radii.bottom_left, px(0.));
        // Radii never pass half the overlap.
        let thin = card.intersect(&RoundedMask::rect(rect(0., 0., 100., 8.)));
        assert_eq!(thin.corner_radii.top_right, px(4.));
        // Both rounded: the rounder one at each corner wins.
        let rounder = RoundedMask {
            bounds: rect(0., 0., 100., 100.),
            corner_radii: Corners {
                top_left: px(30.),
                ..Corners::default()
            },
        };
        let both = card.intersect(&rounder);
        assert_eq!(both.corner_radii.top_left, px(30.));
        assert_eq!(both.corner_radii.bottom_right, px(12.));
    }

    #[test]
    fn rounded_masks_follow_transforms() {
        let card = RoundedMask {
            bounds: rect(0., 0., 100., 100.),
            corner_radii: Corners::all(px(10.)),
        };
        let half = TransformationMatrix::unit().scale(Size {
            width: 0.5,
            height: 0.5,
        });
        let small = card.transformed(&half);
        assert_eq!(small.bounds, rect(0., 0., 50., 50.));
        assert_eq!(small.corner_radii, Corners::all(px(5.)));

        // Turned, it becomes the square box around the turned shape.
        let quarter = TransformationMatrix::unit().rotate(Radians(std::f32::consts::FRAC_PI_2));
        let turned = card.transformed(&quarter);
        assert!(!turned.is_rounded());
        assert!((turned.bounds.size.width.0 - 100.).abs() < 1e-3);
    }

    #[test]
    fn transformed_primitives_are_ordered_where_they_land() {
        let mut scene = Scene::default();
        let quad = |x: f32, element_transform| Quad {
            order: 0,
            border_style: BorderStyle::Solid,
            bounds: Bounds::new(
                point(ScaledPixels(x), ScaledPixels(0.)),
                size(ScaledPixels(10.), ScaledPixels(10.)),
            ),
            content_mask: RoundedMask::rect(Bounds::new(
                point(ScaledPixels(-1000.), ScaledPixels(-1000.)),
                size(ScaledPixels(3000.), ScaledPixels(3000.)),
            )),
            background: Hsla::default().into(),
            border_color: Hsla::default(),
            corner_radii: Corners::default(),
            border_widths: Edges::default(),
            element_transform,
        };
        scene.insert_primitive(quad(100., TransformationMatrix::unit()));
        // Laid out far away but moved onto the first one: it must draw after it.
        let moved =
            TransformationMatrix::unit().translate(point(ScaledPixels(100.), ScaledPixels(0.)));
        scene.insert_primitive(quad(0., moved));
        assert!(scene.quads[1].order > scene.quads[0].order);
    }
}
