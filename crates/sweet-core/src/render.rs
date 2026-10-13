//! Resolve Tailwind classes, lay the tree out with Taffy, and paint it.

use std::cell::RefCell;
use std::collections::HashSet;

use taffy::prelude::*;
use tiny_skia::{FillRule, Mask, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::color::Rgba;
use crate::dom::{Document, NodeId, Raster, SnapKind, Snapshot};
use crate::error::SweetError;
use crate::tailwind::{
    self, CursorStyle, GradDir, LinearGrad, Motion, ObjectFit, ResolveCtx, Resolved,
};
use crate::text::{Clip, FontBook};
use crate::transform::{self, BlitClip, Mat4, Persp};

#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub width: f32,
    pub height: f32,
    pub scale: f32,
    pub hover: Option<NodeId>,
    pub focus: Option<NodeId>,
    /// Seconds since the window opened. Loops and transitions read this clock.
    pub time: f32,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: 800.0,
            height: 600.0,
            scale: 1.0,
            hover: None,
            focus: None,
            time: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LayoutBox {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub clip_x: f32,
    pub clip_y: f32,
    pub clip_w: f32,
    pub clip_h: f32,
    pub cursor: CursorStyle,
    pub hit_test: bool,
    /// Screen-space border-box corners after the transform. Used when `shaped`.
    pub corners: [(f32, f32); 4],
    pub shaped: bool,
    pub scroll_left: f32,
    pub scroll_top: f32,
    pub max_scroll_left: f32,
    pub max_scroll_top: f32,
    pub scrollable_x: bool,
    pub scrollable_y: bool,
    pub scrollbar_width: f32,
    pub scrollbar_gutter: f32,
    pub scrollbar_thumb: Rgba,
    pub scrollbar_track: Rgba,
    pub scrollbar_radius: f32,
}

impl LayoutBox {
    pub fn hits(&self, x: f32, y: f32) -> bool {
        if !self.hit_test || self.width <= 0.0 || self.height <= 0.0 {
            return false;
        }
        let in_clip = x >= self.clip_x
            && y >= self.clip_y
            && x < self.clip_x + self.clip_w
            && y < self.clip_y + self.clip_h;
        if !in_clip {
            return false;
        }
        if self.shaped {
            return point_in_quad(self.corners, x, y);
        }
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub boxes: Vec<LayoutBox>,
    pub fields: Vec<TextField>,
    /// A motion or an in-flight transition still needs another frame.
    pub animating: bool,
    /// Layer pixmaps built on this frame. A cache hit stays at zero.
    pub rasterized_layers: u32,
}

/// Content box of an `input` or `textarea`. Egui draws the text; Tailwind drew the chrome.
#[derive(Clone)]
pub struct TextField {
    pub id: NodeId,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: Rgba,
    pub font_size: f32,
    /// Padding edge, the same rectangle `overflow-hidden` clips to.
    pub clip_x: f32,
    pub clip_y: f32,
    pub clip_w: f32,
    pub clip_h: f32,
    pub multiline: bool,
    pub password: bool,
    pub disabled: bool,
    pub value: String,
    pub placeholder: String,
}

impl Frame {
    /// Deepest painted box under a point, in CSS pixels.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<NodeId> {
        self.boxes.iter().rev().find(|b| b.hits(x, y)).map(|b| b.id)
    }
}

struct TextMeasure {
    text: String,
    font_size: f32,
    font_weight: u16,
    line_height: f32,
    nowrap: bool,
}

enum LeafMeasure {
    Text(TextMeasure),
    NerdIcon {
        character: char,
        font_size: f32,
        line_height: f32,
    },
    Image {
        width: f32,
        height: f32,
    },
    /// Intrinsic content height of an input or textarea, in line boxes.
    Field {
        line_height: f32,
        lines: f32,
    },
}

pub fn physical_size(options: &RenderOptions) -> (u32, u32) {
    let (_, _, _, width, height) = viewport(options);
    (width, height)
}

pub fn render(document: &Document, options: &RenderOptions) -> Result<Frame, SweetError> {
    let (pixmap, boxes, fields, animating, rasterized_layers) =
        draw_scene(document, options, false)?;
    let frame = Frame {
        width: pixmap.width(),
        height: pixmap.height(),
        rgba: straight_rgba(&pixmap),
        boxes,
        fields,
        animating,
        rasterized_layers,
    };
    recycle_pixmap(pixmap);
    Ok(frame)
}

/// Paint `0x00RRGGBB` pixels into `out`. `out` must hold [`physical_size`] pixels.
pub fn render_xrgb(
    document: &Document,
    options: &RenderOptions,
    out: &mut [u32],
) -> Result<(Vec<LayoutBox>, Vec<TextField>, bool), SweetError> {
    let (pixmap, boxes, fields, animating, _) = draw_scene(document, options, false)?;
    let count = pixmap.width() as usize * pixmap.height() as usize;
    if out.len() < count {
        recycle_pixmap(pixmap);
        return Err(SweetError::new("frame buffer is smaller than the viewport"));
    }
    copy_xrgb(&pixmap, &mut out[..count]);
    recycle_pixmap(pixmap);
    Ok((boxes, fields, animating))
}

/// Paint premultiplied `0xAARRGGBB` pixels for transparent native windows.
pub fn render_argb(
    document: &Document,
    options: &RenderOptions,
    opacity: f32,
    out: &mut [u32],
) -> Result<(Vec<LayoutBox>, Vec<TextField>, bool), SweetError> {
    let (pixmap, boxes, fields, animating, _) = draw_scene(document, options, true)?;
    let count = pixmap.width() as usize * pixmap.height() as usize;
    if out.len() < count {
        recycle_pixmap(pixmap);
        return Err(SweetError::new("frame buffer is smaller than the viewport"));
    }
    copy_argb(&pixmap, opacity, &mut out[..count]);
    recycle_pixmap(pixmap);
    Ok((boxes, fields, animating))
}

fn viewport(options: &RenderOptions) -> (f32, f32, f32, u32, u32) {
    let width = options.width.max(1.0);
    let height = options.height.max(1.0);
    let scale = options.scale.max(0.25);
    let phys_w = (width * scale).round().max(1.0) as u32;
    let phys_h = (height * scale).round().max(1.0) as u32;
    (width, height, scale, phys_w, phys_h)
}

thread_local! {
    static SCRATCH: RefCell<Option<Pixmap>> = const { RefCell::new(None) };
}

fn lease_pixmap(width: u32, height: u32) -> Result<Pixmap, SweetError> {
    let cached = SCRATCH.with(|slot| {
        let existing = slot.borrow_mut().take();
        match existing {
            Some(pixmap) if pixmap.width() == width && pixmap.height() == height => Some(pixmap),
            _ => None,
        }
    });
    if let Some(pixmap) = cached {
        return Ok(pixmap);
    }
    Pixmap::new(width, height).ok_or_else(|| SweetError::new("viewport is empty"))
}

fn recycle_pixmap(pixmap: Pixmap) {
    SCRATCH.with(|slot| {
        *slot.borrow_mut() = Some(pixmap);
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SceneKey {
    doc: u64,
    epoch: u64,
    width: u32,
    height: u32,
    scale_bits: u32,
    hover: Option<u32>,
    focus: Option<u32>,
    transparent: bool,
}

struct SceneStore {
    key: SceneKey,
    snapshot: Snapshot,
    resolved: Vec<Resolved>,
    tree: TaffyTree<Option<LeafMeasure>>,
    taffy_ids: Vec<Option<taffy::NodeId>>,
    descendant_motion: Vec<bool>,
    replayed_images: Vec<bool>,
    /// Static pixels painted before the first moving layer.
    base: Pixmap,
    /// Static pixels painted after that layer (text, controls), transparent elsewhere.
    overlay: Pixmap,
    /// Inclusive-exclusive physical bounds of the non-transparent overlay pixels.
    overlay_rect: (i32, i32, i32, i32),
}

thread_local! {
    static SCENE: RefCell<Option<SceneStore>> = const { RefCell::new(None) };
}

fn draw_scene(
    document: &Document,
    options: &RenderOptions,
    transparent: bool,
) -> Result<(Pixmap, Vec<LayoutBox>, Vec<TextField>, bool, u32), SweetError> {
    let (width, height, scale, phys_w, phys_h) = viewport(options);
    let key = SceneKey {
        doc: document.id,
        epoch: document.epoch(),
        width: phys_w,
        height: phys_h,
        scale_bits: scale.to_bits(),
        hover: options.hover.map(|id| id.0),
        focus: options.focus.map(|id| id.0),
        transparent,
    };
    if let Some(frame) = replay_scene(&key, document, options, width, height, scale) {
        return frame;
    }

    let snapshot = document.snapshot();
    let hovered = hover_chain(&snapshot, options.hover);
    let resolved = resolve_tree(&snapshot, &hovered, options.focus, width, height);
    let (mut tree, taffy_ids) = build_tree(&snapshot, &resolved, document.fonts.as_ref())?;
    let root = taffy_ids[snapshot.body.0 as usize]
        .ok_or_else(|| SweetError::new("body produced no box"))?;
    tree_layout(&mut tree, root, width, height, document.fonts.as_ref())?;
    let descendant_motion = descendant_motion(&snapshot, &resolved);
    let replayed_images = replayed_images(&snapshot, &resolved, &descendant_motion);

    let mut base =
        Pixmap::new(phys_w, phys_h).ok_or_else(|| SweetError::new("viewport is empty"))?;
    if transparent {
        base.fill(tiny_skia::Color::TRANSPARENT);
    } else {
        base.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    }
    let mut overlay =
        Pixmap::new(phys_w, phys_h).ok_or_else(|| SweetError::new("viewport is empty"))?;
    let mut split_rasterized = 0u32;
    {
        let env = PaintEnv {
            snapshot: &snapshot,
            document,
            resolved: &resolved,
            taffy_ids: taffy_ids.as_slice(),
            tree: &tree,
            fonts: document.fonts.as_ref(),
            scale,
            time: options.time,
            doc_id: document.id,
            descendant_motion: &descendant_motion,
        };
        let mut boxes = Vec::new();
        let mut fields = Vec::new();
        let mut animating = false;
        let mut out = PaintOut {
            boxes: &mut boxes,
            fields: &mut fields,
            animating: &mut animating,
            rasterized: &mut split_rasterized,
        };
        let mut seen = false;
        paint_node(
            &mut base,
            Some(&mut overlay),
            &env,
            &mut out,
            snapshot.body,
            Mat4::identity(),
            None,
            1.0,
            LogicalClip {
                x: 0.0,
                y: 0.0,
                w: width,
                h: height,
            },
            None,
            None,
            None,
            None,
            false,
            PaintMode::Split,
            &mut seen,
        );
    }

    let overlay_rect = pixmap_bounds(&overlay);
    let mut pixmap = lease_pixmap(phys_w, phys_h)?;
    copy_pixmap(&base, &mut pixmap);
    let (boxes, fields, animating, rasterized) = paint_sprites(
        &mut pixmap,
        &overlay,
        overlay_rect,
        &snapshot,
        &resolved,
        &taffy_ids,
        &tree,
        &descendant_motion,
        document,
        options,
        width,
        height,
        scale,
    )?;
    SCENE.with(|slot| {
        *slot.borrow_mut() = Some(SceneStore {
            key,
            snapshot,
            resolved,
            tree,
            taffy_ids,
            descendant_motion,
            replayed_images,
            base,
            overlay,
            overlay_rect,
        });
    });
    Ok((pixmap, boxes, fields, animating, rasterized + split_rasterized))
}

fn replay_scene(
    key: &SceneKey,
    document: &Document,
    options: &RenderOptions,
    width: f32,
    height: f32,
    scale: f32,
) -> Option<Result<(Pixmap, Vec<LayoutBox>, Vec<TextField>, bool, u32), SweetError>> {
    SCENE.with(|slot| {
        let mut guard = slot.borrow_mut();
        let scene = guard.as_mut()?;
        if scene.key != *key
            || !document.refresh_layer_images(
                &mut scene.snapshot, key.epoch, &scene.replayed_images,
            )
        {
            return None;
        }
        let mut pixmap = match lease_pixmap(key.width, key.height) {
            Ok(pixmap) => pixmap,
            Err(err) => return Some(Err(err)),
        };
        copy_pixmap(&scene.base, &mut pixmap);
        let painted = paint_sprites(
            &mut pixmap,
            &scene.overlay,
            scene.overlay_rect,
            &scene.snapshot,
            &scene.resolved,
            &scene.taffy_ids,
            &scene.tree,
            &scene.descendant_motion,
            document,
            options,
            width,
            height,
            scale,
        );
        Some(match painted {
            Ok((boxes, fields, animating, rasterized)) => {
                Ok((pixmap, boxes, fields, animating, rasterized))
            }
            Err(err) => Err(err),
        })
    })
}

fn paint_sprites(
    pixmap: &mut Pixmap,
    overlay: &Pixmap,
    overlay_rect: (i32, i32, i32, i32),
    snapshot: &Snapshot,
    resolved: &[Resolved],
    taffy_ids: &[Option<taffy::NodeId>],
    tree: &TaffyTree<Option<LeafMeasure>>,
    descendant_motion: &[bool],
    document: &Document,
    options: &RenderOptions,
    width: f32,
    height: f32,
    scale: f32,
) -> Result<(Vec<LayoutBox>, Vec<TextField>, bool, u32), SweetError> {
    let env = PaintEnv {
        snapshot,
        document,
        resolved,
        taffy_ids,
        tree,
        fonts: document.fonts.as_ref(),
        scale,
        time: options.time,
        doc_id: document.id,
        descendant_motion,
    };
    let mut boxes = Vec::new();
    let mut fields = Vec::new();
    let mut animating = false;
    let mut rasterized = 0u32;
    let mut out = PaintOut {
        boxes: &mut boxes,
        fields: &mut fields,
        animating: &mut animating,
        rasterized: &mut rasterized,
    };
    let mut seen = false;
    paint_node(
        pixmap,
        None,
        &env,
        &mut out,
        snapshot.body,
        Mat4::identity(),
        None,
        1.0,
        LogicalClip {
            x: 0.0,
            y: 0.0,
            w: width,
            h: height,
        },
        None,
        None,
        None,
        None,
        true,
        PaintMode::Sprites,
        &mut seen,
    );
    composite_overlay(overlay, pixmap, overlay_rect);
    Ok((boxes, fields, animating, rasterized))
}

fn copy_pixmap(src: &Pixmap, dest: &mut Pixmap) {
    dest.data_mut().copy_from_slice(src.data());
}

fn pixmap_bounds(src: &Pixmap) -> (i32, i32, i32, i32) {
    let w = src.width() as i32;
    let h = src.height() as i32;
    let data = src.data();
    let mut x0 = w;
    let mut y0 = h;
    let mut x1 = 0;
    let mut y1 = 0;
    for y in 0..h {
        let row = (y as usize) * (w as usize);
        let mut left = w;
        let mut right = 0;
        for x in 0..w {
            if data[(row + x as usize) * 4 + 3] != 0 {
                if x < left {
                    left = x;
                }
                right = x + 1;
            }
        }
        if right > left {
            if y < y0 {
                y0 = y;
            }
            y1 = y + 1;
            if left < x0 {
                x0 = left;
            }
            if right > x1 {
                x1 = right;
            }
        }
    }
    if y0 >= y1 {
        (0, 0, 0, 0)
    } else {
        (x0, y0, x1, y1)
    }
}

fn composite_overlay(src: &Pixmap, dest: &mut Pixmap, rect: (i32, i32, i32, i32)) {
    let (x0, y0, x1, y1) = rect;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let w = src.width() as i32;
    let data = src.data();
    let dst = dest.pixels_mut();
    for y in y0..y1 {
        let row = y * w;
        for x in x0..x1 {
            let index = (row + x) as usize;
            let byte = index * 4;
            let sample = [data[byte], data[byte + 1], data[byte + 2], data[byte + 3]];
            if sample[3] == 0 {
                continue;
            }
            if let Some(pixel) = dst.get_mut(index) {
                transform::over_sprite(pixel, sample);
            }
        }
    }
}

pub fn encode_png(frame: &Frame) -> Result<Vec<u8>, SweetError> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|err| SweetError::new(err.to_string()))?;
    writer
        .write_image_data(&frame.rgba)
        .map_err(|err| SweetError::new(err.to_string()))?;
    writer
        .finish()
        .map_err(|err| SweetError::new(err.to_string()))?;
    Ok(bytes)
}

fn tree_layout(
    tree: &mut TaffyTree<Option<LeafMeasure>>,
    root: taffy::NodeId,
    width: f32,
    height: f32,
    fonts: &FontBook,
) -> Result<(), SweetError> {
    tree.compute_layout_with_measure(
        root,
        Size {
            width: AvailableSpace::Definite(width),
            height: AvailableSpace::Definite(height),
        },
        |inputs, _node, context, style| {
            taffy::compute_leaf_layout(
                inputs,
                style,
                |_, _| 0.0,
                |known, available| measure_leaf(known, available, context.as_deref(), fonts),
            )
        },
    )
    .map_err(|err| SweetError::new(err.to_string()))
}

fn measure_leaf(
    known: Size<Option<f32>>,
    available: Size<AvailableSpace>,
    context: Option<&Option<LeafMeasure>>,
    fonts: &FontBook,
) -> Size<f32> {
    let Some(Some(info)) = context else {
        return Size {
            width: known.width.unwrap_or(0.0),
            height: known.height.unwrap_or(0.0),
        };
    };
    let info = match info {
        LeafMeasure::NerdIcon {
            character,
            font_size,
            line_height,
        } => {
            return Size {
                width: known
                    .width
                    .unwrap_or_else(|| fonts.measure_nerd_glyph(*character, *font_size)),
                height: known.height.unwrap_or(*line_height),
            };
        }
        LeafMeasure::Image { width, height } => {
            return measure_image(known, *width, *height);
        }
        LeafMeasure::Field { line_height, lines } => {
            return Size {
                width: known.width.unwrap_or(0.0),
                height: known.height.unwrap_or(line_height * lines),
            };
        }
        LeafMeasure::Text(info) => info,
    };
    let wrap_at = if info.nowrap {
        f32::INFINITY
    } else if let Some(width) = known.width {
        width
    } else {
        match available.width {
            AvailableSpace::Definite(width) => width.max(0.0),
            AvailableSpace::MinContent => longest_word(fonts, info),
            AvailableSpace::MaxContent => f32::INFINITY,
        }
    };
    let laid = fonts.layout(
        &info.text,
        info.font_size,
        info.font_weight,
        info.line_height,
        wrap_at,
        info.nowrap,
    );
    Size {
        width: known.width.unwrap_or(laid.width),
        height: known
            .height
            .unwrap_or(laid.height.max(if info.text.is_empty() {
                0.0
            } else {
                info.line_height
            })),
    }
}

fn measure_image(known: Size<Option<f32>>, width: f32, height: f32) -> Size<f32> {
    let width = width.max(0.0);
    let height = height.max(0.0);
    match (known.width, known.height) {
        (Some(known_w), Some(known_h)) => Size {
            width: known_w,
            height: known_h,
        },
        (Some(known_w), None) => Size {
            width: known_w,
            height: if width > 0.0 {
                known_w * height / width
            } else {
                0.0
            },
        },
        (None, Some(known_h)) => Size {
            width: if height > 0.0 {
                known_h * width / height
            } else {
                0.0
            },
            height: known_h,
        },
        (None, None) => Size { width, height },
    }
}

fn longest_word(fonts: &FontBook, info: &TextMeasure) -> f32 {
    info.text
        .split_whitespace()
        .map(|word| {
            fonts
                .layout(
                    word,
                    info.font_size,
                    info.font_weight,
                    info.line_height,
                    f32::INFINITY,
                    true,
                )
                .width
        })
        .fold(0.0, f32::max)
}

fn resolve_tree(
    snapshot: &Snapshot,
    hovered: &HashSet<u32>,
    focus: Option<NodeId>,
    width: f32,
    height: f32,
) -> Vec<Resolved> {
    let mut resolved = Vec::with_capacity(snapshot.nodes.len());
    for _ in &snapshot.nodes {
        resolved.push(tailwind::root_resolved(snapshot.dark));
    }
    walk_resolve(
        snapshot,
        snapshot.body,
        &tailwind::root_resolved(snapshot.dark),
        hovered,
        focus,
        width,
        height,
        &mut resolved,
    );
    if let Some(body) = resolved.get_mut(snapshot.body.0 as usize) {
        body.style.size.width = length(width);
        body.style.size.height = length(height);
    }
    resolved
}

fn walk_resolve(
    snapshot: &Snapshot,
    id: NodeId,
    parent: &Resolved,
    hovered: &HashSet<u32>,
    focus: Option<NodeId>,
    width: f32,
    height: f32,
    out: &mut [Resolved],
) {
    let node = &snapshot.nodes[id.0 as usize];
    let mut style = match &node.kind {
        SnapKind::Text { .. } | SnapKind::Comment { .. } => {
            let mut style = tailwind::inherit(parent);
            style.hit_test = false;
            style
        }
        SnapKind::Element {
            class_name,
            disabled,
            control,
            ..
        } => {
            let mut style = if id == snapshot.body {
                tailwind::root_resolved(snapshot.dark)
            } else {
                tailwind::inherit(parent)
            };
            let ctx = ResolveCtx {
                viewport_w: width,
                viewport_h: height,
                hovered: hovered.contains(&id.0),
                focused: focus == Some(id),
                dark: snapshot.dark,
                disabled: *disabled,
            };
            tailwind::apply_classes(&mut style, class_name, &ctx);
            // Text is painted by egui, outside the child walk, so the field
            // itself has to clip the way overflow-hidden clips descendants.
            if control.is_some() {
                style.clip = true;
                style.style.overflow = taffy::geometry::Point {
                    x: taffy::style::Overflow::Hidden,
                    y: taffy::style::Overflow::Hidden,
                };
            }
            style
        }
    };
    if id == snapshot.body {
        style.style.size.width = length(width);
        style.style.size.height = length(height);
    }
    let inherited = style.clone();
    out[id.0 as usize] = style;
    if matches!(out[id.0 as usize].style.display, Display::None) {
        return;
    }
    for child in &node.children {
        walk_resolve(
            snapshot, *child, &inherited, hovered, focus, width, height, out,
        );
    }
}

fn build_tree<'a>(
    snapshot: &'a Snapshot,
    resolved: &'a [Resolved],
    _fonts: &FontBook,
) -> Result<(TaffyTree<Option<LeafMeasure>>, Vec<Option<taffy::NodeId>>), SweetError> {
    let mut tree: TaffyTree<Option<LeafMeasure>> = TaffyTree::new();
    let mut ids = vec![None; snapshot.nodes.len()];
    build_node(snapshot, resolved, snapshot.body, &mut tree, &mut ids)?;
    Ok((tree, ids))
}

fn build_node(
    snapshot: &Snapshot,
    resolved: &[Resolved],
    id: NodeId,
    tree: &mut TaffyTree<Option<LeafMeasure>>,
    ids: &mut [Option<taffy::NodeId>],
) -> Result<Option<taffy::NodeId>, SweetError> {
    let node = &snapshot.nodes[id.0 as usize];
    if matches!(node.kind, SnapKind::Comment { .. }) {
        return Ok(None);
    }
    let style = &resolved[id.0 as usize];
    if matches!(style.style.display, Display::None) {
        return Ok(None);
    }
    let mut child_ids = Vec::new();
    for child in &node.children {
        if let Some(child_id) = build_node(snapshot, resolved, *child, tree, ids)? {
            child_ids.push(child_id);
        }
    }
    let taffy_id = if child_ids.is_empty() {
        let context = match &node.kind {
            SnapKind::Text { text } => Some(LeafMeasure::Text(TextMeasure {
                text: text.clone(),
                font_size: style.font_size,
                font_weight: style.font_weight,
                line_height: style.line_height,
                nowrap: style.nowrap,
            })),
            SnapKind::Element {
                control: Some(control),
                ..
            } => Some(LeafMeasure::Field {
                line_height: style.line_height.max(style.font_size),
                lines: if control.multiline { 2.0 } else { 1.0 },
            }),
            SnapKind::Element {
                image: Some(image), ..
            } => Some(LeafMeasure::Image {
                width: image.width as f32,
                height: image.height as f32,
            }),
            SnapKind::Element { .. } if style.nerd_icon.is_some() => Some(LeafMeasure::NerdIcon {
                character: style.nerd_icon.unwrap(),
                font_size: style.font_size,
                line_height: style.line_height,
            }),
            SnapKind::Element { .. } | SnapKind::Comment { .. } => None,
        };
        tree.new_leaf_with_context(style.style.clone(), context)
            .map_err(|err| SweetError::new(err.to_string()))?
    } else {
        tree.new_with_children(style.style.clone(), &child_ids)
            .map_err(|err| SweetError::new(err.to_string()))?
    };
    ids[id.0 as usize] = Some(taffy_id);
    Ok(Some(taffy_id))
}

#[derive(Clone, Copy, PartialEq)]
struct LogicalClip {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

struct PaintEnv<'a> {
    snapshot: &'a Snapshot,
    document: &'a Document,
    resolved: &'a [Resolved],
    taffy_ids: &'a [Option<taffy::NodeId>],
    tree: &'a TaffyTree<Option<LeafMeasure>>,
    fonts: &'a FontBook,
    scale: f32,
    time: f32,
    doc_id: u64,
    descendant_motion: &'a [bool],
}

struct PaintOut<'a> {
    boxes: &'a mut Vec<LayoutBox>,
    fields: &'a mut Vec<TextField>,
    animating: &'a mut bool,
    rasterized: &'a mut u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PaintMode {
    /// Paint static pixels and sprites into `pixmap`.
    Direct,
    /// Static pixels go to `pixmap` until the first sprite, then to `overlay`.
    /// Sprites themselves are not drawn; the replay pass draws them.
    Split,
    /// Copy of the static frame is already in `pixmap`. Draw sprites only.
    Sprites,
}

fn paint_node(
    pixmap: &mut Pixmap,
    mut overlay: Option<&mut Pixmap>,
    env: &PaintEnv,
    out: &mut PaintOut,
    id: NodeId,
    space: Mat4,
    mut persp: Option<Persp>,
    parent_opacity: f32,
    clip: LogicalClip,
    round: Option<BlitClip>,
    mask: Option<&Mask>,
    bake: Option<NodeId>,
    local_place: Option<(f32, f32)>,
    record: bool,
    mode: PaintMode,
    seen_sprite: &mut bool,
) {
    let Some(taffy_id) = env.taffy_ids[id.0 as usize] else {
        return;
    };
    let Ok(layout) = env.tree.layout(taffy_id) else {
        return;
    };
    let location_x = layout.location.x;
    let location_y = layout.location.y;
    let w = layout.size.width;
    let h = layout.size.height;
    let border = layout.border;
    let padding = layout.padding;
    let style = &env.resolved[id.0 as usize];
    let (scroll_left, scroll_top) = match &env.snapshot.nodes[id.0 as usize].kind {
        SnapKind::Element {
            scroll_left,
            scroll_top,
            ..
        } => (*scroll_left, *scroll_top),
        _ => (0.0, 0.0),
    };
    let max_scroll_left = layout.scroll_width().max(0.0);
    let max_scroll_top = layout.scroll_height().max(0.0);
    let client_width = (w - border.left - border.right - layout.scrollbar_size.width).max(0.0);
    let client_height = (h - border.top - border.bottom - layout.scrollbar_size.height).max(0.0);
    if record {
        env.document.update_scroll_metrics(
            id,
            client_width + max_scroll_left,
            client_height + max_scroll_top,
            client_width,
            client_height,
        );
    }
    let (x, y) = if let Some(place) = local_place {
        place
    } else {
        let (sx, sy, _) = space.transform_point(location_x, location_y, 0.0);
        (sx, sy)
    };
    let track = style.transition || style.motion != Motion::None || !style.xform.is_identity();
    let tracked = if track {
        Some(transform::visual_sample(
            style,
            env.time,
            (env.doc_id, id.0),
            out.animating,
        ))
    } else {
        None
    };
    // The class transform counts even when a transition is still sitting on the
    // first frame. Otherwise that frame is baked into the static picture and
    // later frames cannot move it.
    let use_layer = style.transition
        || style.motion != Motion::None
        || !style.xform.is_identity()
        || tracked.as_ref().is_some_and(|sample| sample.moved());
    let animated_layer = style.motion != Motion::None || style.transition;
    let descendants = env
        .descendant_motion
        .get(id.0 as usize)
        .copied()
        .unwrap_or(false);
    if use_layer && !descendants && bake != Some(id) {
        let sample = tracked.expect("transformed node has a sample");
        if animated_layer {
            *seen_sprite = true;
            if mode != PaintMode::Split {
                paint_transformed(
                    pixmap,
                    env,
                    out,
                    id,
                    sample,
                    x,
                    y,
                    w,
                    h,
                    space,
                    location_x,
                    location_y,
                    persp,
                    parent_opacity,
                    clip,
                    round,
                    true,
                    record,
                );
            }
        } else if mode == PaintMode::Sprites {
            paint_transformed(
                pixmap,
                env,
                out,
                id,
                sample,
                x,
                y,
                w,
                h,
                space,
                location_x,
                location_y,
                persp,
                parent_opacity,
                clip,
                round,
                false,
                record,
            );
        } else {
            let dest = static_target(pixmap, overlay.as_deref_mut(), mode, *seen_sprite);
            paint_transformed(
                dest,
                env,
                out,
                id,
                sample,
                x,
                y,
                w,
                h,
                space,
                location_x,
                location_y,
                persp,
                parent_opacity,
                clip,
                round,
                true,
                record,
            );
        }
        return;
    }
    if bake != Some(id) {
        if let Some(distance) = style.perspective {
            persp = Some(Persp {
                cx: x + w * 0.5,
                cy: y + h * 0.5,
                d: distance,
            });
        }
    }
    let sample_opacity = tracked.as_ref().map(|sample| sample.opacity);
    let parent_local = if use_layer && descendants {
        tracked.map(|sample| transform::local_matrix(&sample, w, h))
    } else {
        None
    };
    let child_space = (if bake == Some(id) {
        space
    } else if let Some(local) = parent_local {
        space
            .mul(Mat4::translate(location_x, location_y, 0.0))
            .mul(local)
    } else {
        space.mul(Mat4::translate(location_x, location_y, 0.0))
    })
    .mul(Mat4::translate(-scroll_left, -scroll_top, 0.0));
    let own_opacity = if bake == Some(id) {
        1.0
    } else {
        sample_opacity.unwrap_or(style.opacity)
    };
    let opacity = (parent_opacity * own_opacity).clamp(0.0, 1.0);
    let scale = env.scale;

    let draw_static = mode != PaintMode::Sprites;
    if record {
        out.boxes.push(LayoutBox {
            id,
            parent: env.snapshot.nodes[id.0 as usize].parent,
            x,
            y,
            width: w,
            height: h,
            clip_x: clip.x,
            clip_y: clip.y,
            clip_w: clip.w,
            clip_h: clip.h,
            cursor: style.cursor,
            hit_test: style.hit_test,
            corners: [(0.0, 0.0); 4],
            shaped: false,
            scroll_left,
            scroll_top,
            max_scroll_left,
            max_scroll_top,
            scrollable_x: matches!(
                style.overflow_x,
                tailwind::OverflowMode::Auto | tailwind::OverflowMode::Scroll
            ),
            scrollable_y: matches!(
                style.overflow_y,
                tailwind::OverflowMode::Auto | tailwind::OverflowMode::Scroll
            ),
            scrollbar_width: style.scrollbar_width,
            scrollbar_gutter: layout
                .scrollbar_size
                .width
                .max(layout.scrollbar_size.height),
            scrollbar_thumb: style.scrollbar_thumb,
            scrollbar_track: style.scrollbar_track,
            scrollbar_radius: style.scrollbar_radius,
        });
    }

    let color = style.color.with_alpha(opacity);
    let border_color = style
        .border_color
        .unwrap_or(style.color)
        .with_alpha(opacity);
    if draw_static {
        let dest = static_target(pixmap, overlay.as_deref_mut(), mode, *seen_sprite);
        paint_background(dest, x, y, w, h, style, opacity, scale, mask);
        if let Some(icon) = style.nerd_icon {
            env.fonts.draw_nerd_glyph(
                dest,
                icon,
                x + border.left + padding.left,
                y + border.top + padding.top,
                (w - border.left - border.right - padding.left - padding.right).max(0.0),
                style.font_size,
                style.line_height,
                style.text_align,
                color,
                scale,
                Clip {
                    x: clip.x,
                    y: clip.y,
                    w: clip.w,
                    h: clip.h,
                },
            );
        }
    }
    if record {
        if let SnapKind::Element {
            control: Some(control),
            disabled,
            ..
        } = &env.snapshot.nodes[id.0 as usize].kind
        {
            let pad = intersect(
                clip,
                LogicalClip {
                    x: x + border.left,
                    y: y + border.top,
                    w: (w - border.left - border.right).max(0.0),
                    h: (h - border.top - border.bottom).max(0.0),
                },
            );
            out.fields.push(TextField {
                id,
                x: x + border.left + padding.left,
                y: y + border.top + padding.top,
                width: (w - border.left - border.right - padding.left - padding.right).max(0.0),
                height: (h - border.top - border.bottom - padding.top - padding.bottom).max(0.0),
                color,
                font_size: style.font_size,
                clip_x: pad.x,
                clip_y: pad.y,
                clip_w: pad.w,
                clip_h: pad.h,
                multiline: control.multiline,
                password: control.password,
                disabled: *disabled,
                value: control.value.clone(),
                placeholder: control.placeholder.clone(),
            });
        }
    }
    if draw_static {
        let dest = static_target(pixmap, overlay.as_deref_mut(), mode, *seen_sprite);
        stroke_border(
            dest,
            x,
            y,
            w,
            h,
            style.radius,
            &border,
            border_color,
            scale,
            mask,
        );
        stroke_ring(dest, x, y, w, h, style, opacity, scale, mask);
    }

    if let SnapKind::Element {
        image: Some(image), ..
    } = &env.snapshot.nodes[id.0 as usize].kind
    {
        let content_x = x + border.left + padding.left;
        let content_y = y + border.top + padding.top;
        let content_w = (w - border.left - border.right - padding.left - padding.right).max(0.0);
        let content_h = (h - border.top - border.bottom - padding.top - padding.bottom).max(0.0);
        if draw_static {
            let dest = static_target(pixmap, overlay.as_deref_mut(), mode, *seen_sprite);
            paint_image(
                dest,
                image,
                content_x,
                content_y,
                content_w,
                content_h,
                style.object_fit,
                style.object_y,
                style.radius,
                opacity,
                scale,
                mask,
            );
        }
    }

    let mut child_clip = clip;
    let mut child_round = round;
    if style.clip {
        let px = x + border.left;
        let py = y + border.top;
        let pw = (w - border.left - border.right).max(0.0);
        let ph = (h - border.top - border.bottom).max(0.0);
        child_clip = intersect(
            clip,
            LogicalClip {
                x: px,
                y: py,
                w: pw,
                h: ph,
            },
        );
        if style.radius > 0.0 {
            child_round = Some(BlitClip {
                x: px,
                y: py,
                w: pw,
                h: ph,
                radius: style.radius,
            });
        }
    }

    if let SnapKind::Text { text } = &env.snapshot.nodes[id.0 as usize].kind {
        let content_x = x + border.left + padding.left;
        let content_y = y + border.top + padding.top;
        let content_w = (w - border.left - border.right - padding.left - padding.right).max(0.0);
        let content_h = (h - border.top - border.bottom - padding.top - padding.bottom).max(0.0);
        let max_width = if style.nowrap {
            f32::INFINITY
        } else {
            content_w
        };
        if draw_static {
            let dest = static_target(pixmap, overlay.as_deref_mut(), mode, *seen_sprite);
            env.fonts.draw(
                dest,
                text,
                content_x,
                content_y,
                content_w,
                style.font_size,
                style.font_weight,
                style.line_height,
                style.text_align,
                color,
                scale,
                max_width,
                style.nowrap,
                Clip {
                    x: child_clip.x * scale,
                    y: child_clip.y * scale,
                    w: child_clip.w * scale,
                    h: child_clip.h * scale,
                },
            );
        }
        let _ = content_h;
    }

    let mut children: Vec<NodeId> = env.snapshot.nodes[id.0 as usize].children.clone();
    children.sort_by(|a, b| {
        let za = env.resolved[a.0 as usize].z;
        let zb = env.resolved[b.0 as usize].z;
        za.cmp(&zb).then(a.0.cmp(&b.0))
    });
    if child_clip != clip && (child_clip.w <= 0.0 || child_clip.h <= 0.0) {
        return;
    }
    if mode == PaintMode::Sprites || child_clip == clip {
        for child in children {
            paint_node(
                pixmap,
                overlay.as_deref_mut(),
                env,
                out,
                child,
                child_space,
                persp,
                opacity,
                child_clip,
                child_round,
                mask,
                bake,
                None,
                record,
                mode,
                seen_sprite,
            );
        }
        if draw_static {
            let dest = static_target(pixmap, overlay.as_deref_mut(), mode, *seen_sprite);
            paint_scrollbars(
                dest,
                x,
                y,
                w,
                h,
                border,
                layout,
                style,
                scroll_left,
                scroll_top,
                scale,
                mask,
            );
        }
        return;
    }
    let Some(child_mask) = clip_mask(pixmap, child_clip, scale) else {
        return;
    };
    for child in children {
        paint_node(
            pixmap,
            overlay.as_deref_mut(),
            env,
            out,
            child,
            child_space,
            persp,
            opacity,
            child_clip,
            child_round,
            Some(&child_mask),
            bake,
            None,
            record,
            mode,
            seen_sprite,
        );
    }
    if draw_static {
        let dest = static_target(pixmap, overlay.as_deref_mut(), mode, *seen_sprite);
        paint_scrollbars(
            dest,
            x,
            y,
            w,
            h,
            border,
            layout,
            style,
            scroll_left,
            scroll_top,
            scale,
            mask,
        );
    }
}

fn paint_scrollbars(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    border: taffy::Rect<f32>,
    layout: &taffy::Layout,
    style: &Resolved,
    scroll_left: f32,
    scroll_top: f32,
    scale: f32,
    mask: Option<&Mask>,
) {
    let thickness = style.scrollbar_width;
    if thickness <= 0.0 {
        return;
    }
    let max_x = layout.scroll_width().max(0.0);
    let max_y = layout.scroll_height().max(0.0);
    let client_w = (w - border.left - border.right - layout.scrollbar_size.width).max(0.0);
    let client_h = (h - border.top - border.bottom - layout.scrollbar_size.height).max(0.0);
    let round = style.scrollbar_radius;
    let vertical = style.overflow_y == tailwind::OverflowMode::Auto
        || style.overflow_y == tailwind::OverflowMode::Scroll;
    let horizontal = style.overflow_x == tailwind::OverflowMode::Auto
        || style.overflow_x == tailwind::OverflowMode::Scroll;

    if vertical && max_y > 0.0 && client_h > 0.0 {
        let track_x = if layout.scrollbar_size.width > 0.0 {
            x + w - border.right - layout.scrollbar_size.width
                + (layout.scrollbar_size.width - thickness).max(0.0) * 0.5
        } else {
            x + w - border.right - thickness
        };
        let track_y = y + border.top;
        let track_h = (h - border.top - border.bottom - layout.scrollbar_size.height).max(0.0);
        fill_box(
            pixmap,
            track_x,
            track_y,
            thickness,
            track_h,
            round,
            style.scrollbar_track,
            scale,
            mask,
        );
        let thumb_h = (track_h * client_h / (client_h + max_y).max(1.0))
            .max((thickness * 2.0).min(track_h))
            .min(track_h);
        let thumb_y = track_y + (track_h - thumb_h) * (scroll_top / max_y).clamp(0.0, 1.0);
        fill_box(
            pixmap,
            track_x,
            thumb_y,
            thickness,
            thumb_h,
            round,
            style.scrollbar_thumb,
            scale,
            mask,
        );
    }
    if horizontal && max_x > 0.0 && client_w > 0.0 {
        let track_x = x + border.left;
        let track_y = if layout.scrollbar_size.height > 0.0 {
            y + h - border.bottom - layout.scrollbar_size.height
                + (layout.scrollbar_size.height - thickness).max(0.0) * 0.5
        } else {
            y + h - border.bottom - thickness
        };
        let track_w = (w - border.left - border.right - layout.scrollbar_size.width).max(0.0);
        fill_box(
            pixmap,
            track_x,
            track_y,
            track_w,
            thickness,
            round,
            style.scrollbar_track,
            scale,
            mask,
        );
        let thumb_w = (track_w * client_w / (client_w + max_x).max(1.0))
            .max((thickness * 2.0).min(track_w))
            .min(track_w);
        let thumb_x = track_x + (track_w - thumb_w) * (scroll_left / max_x).clamp(0.0, 1.0);
        fill_box(
            pixmap,
            thumb_x,
            track_y,
            thumb_w,
            thickness,
            round,
            style.scrollbar_thumb,
            scale,
            mask,
        );
    }
}

fn static_target<'a>(
    pixmap: &'a mut Pixmap,
    overlay: Option<&'a mut Pixmap>,
    mode: PaintMode,
    seen_sprite: bool,
) -> &'a mut Pixmap {
    if mode == PaintMode::Split && seen_sprite {
        if let Some(overlay) = overlay {
            return overlay;
        }
    }
    pixmap
}

// Only pixels inside a leaf animated layer are absent from the static base/overlay.
// Updating anything else must rebuild those surfaces to avoid stale imagery.
fn replayed_images(snapshot: &Snapshot, resolved: &[Resolved], descendants: &[bool]) -> Vec<bool> {
    fn walk(id: NodeId, snapshot: &Snapshot, resolved: &[Resolved], descendants: &[bool],
            inherited: bool, result: &mut [bool]) {
        let index = id.0 as usize;
        let style = &resolved[index];
        let replayed = inherited || (!descendants[index]
            && (style.transition || style.motion != Motion::None));
        result[index] = replayed;
        for child in &snapshot.nodes[index].children {
            walk(*child, snapshot, resolved, descendants, replayed, result);
        }
    }
    let mut result = vec![false; snapshot.nodes.len()];
    walk(snapshot.body, snapshot, resolved, descendants, false, &mut result);
    result
}

fn descendant_motion(snapshot: &Snapshot, resolved: &[Resolved]) -> Vec<bool> {
    let mut desc = vec![false; snapshot.nodes.len()];
    fn walk(id: NodeId, snapshot: &Snapshot, resolved: &[Resolved], desc: &mut [bool]) -> bool {
        let mut any = false;
        if let Some(node) = snapshot.nodes.get(id.0 as usize) {
            for child in &node.children {
                if walk(*child, snapshot, resolved, desc) {
                    any = true;
                }
            }
        }
        if let Some(slot) = desc.get_mut(id.0 as usize) {
            *slot = any;
        }
        let own = resolved
            .get(id.0 as usize)
            .is_some_and(|style| style.motion != Motion::None || style.transition);
        own || any
    }
    walk(snapshot.body, snapshot, resolved, &mut desc);
    desc
}

fn point_in_quad(corners: [(f32, f32); 4], x: f32, y: f32) -> bool {
    let mut sign = 0.0;
    for index in 0..4 {
        let (x1, y1) = corners[index];
        let (x2, y2) = corners[(index + 1) % 4];
        let cross = (x2 - x1) * (y - y1) - (y2 - y1) * (x - x1);
        if cross.abs() < 1.0e-4 {
            continue;
        }
        let next = cross.signum();
        if sign == 0.0 {
            sign = next;
        } else if next != sign {
            return false;
        }
    }
    true
}

fn paint_transformed(
    pixmap: &mut Pixmap,
    env: &PaintEnv,
    out: &mut PaintOut,
    id: NodeId,
    sample: transform::Sample,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    space: Mat4,
    location_x: f32,
    location_y: f32,
    persp: Option<Persp>,
    parent_opacity: f32,
    clip: LogicalClip,
    round: Option<BlitClip>,
    draw: bool,
    record: bool,
) {
    let world = space
        .mul(Mat4::translate(location_x, location_y, 0.0))
        .mul(transform::local_matrix(&sample, w, h));
    let fade = (parent_opacity * sample.opacity).clamp(0.0, 1.0);
    let (min_x, min_y, logical_w, logical_h) = layer_bounds(env, id, w, h);
    let scale = env.scale;
    let pixel_w = (logical_w * scale).ceil().max(1.0) as u32;
    let pixel_h = (logical_h * scale).ceil().max(1.0) as u32;
    if draw && pixel_w <= 4096 && pixel_h <= 4096 && logical_w > 0.0 && logical_h > 0.0 {
        let stamp = layer_stamp(env, id);
        let clip_blit = BlitClip {
            x: clip.x,
            y: clip.y,
            w: clip.w,
            h: clip.h,
            radius: 0.0,
        };
        let built = transform::with_layer(
            (env.doc_id, id.0),
            stamp,
            pixel_w,
            pixel_h,
            scale,
            || {
                let mut baked =
                    transform::blank_layer(pixel_w, pixel_h, min_x, min_y, logical_w, logical_h)?;
                let mut seen = false;
                paint_node(
                    &mut baked.pixmap,
                    None,
                    env,
                    out,
                    id,
                    Mat4::translate(-min_x, -min_y, 0.0),
                    None,
                    1.0,
                    LogicalClip {
                        x: 0.0,
                        y: 0.0,
                        w: logical_w,
                        h: logical_h,
                    },
                    None,
                    None,
                    Some(id),
                    Some((-min_x, -min_y)),
                    false,
                    PaintMode::Direct,
                    &mut seen,
                );
                Some(baked)
            },
            |layer| {
                transform::blit(
                    pixmap,
                    layer,
                    &world,
                    persp,
                    fade,
                    scale,
                    &clip_blit,
                    round.as_ref(),
                );
            },
        );
        if built {
            *out.rasterized += 1;
        }
    }
    if record {
        let corners = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]
            .map(|(px, py)| transform::project(&world, px, py, persp));
        let style = &env.resolved[id.0 as usize];
        out.boxes.push(LayoutBox {
            id,
            parent: env.snapshot.nodes[id.0 as usize].parent,
            x,
            y,
            width: w,
            height: h,
            clip_x: clip.x,
            clip_y: clip.y,
            clip_w: clip.w,
            clip_h: clip.h,
            cursor: style.cursor,
            hit_test: style.hit_test && fade > 0.01,
            corners,
            shaped: true,
            scroll_left: 0.0,
            scroll_top: 0.0,
            max_scroll_left: 0.0,
            max_scroll_top: 0.0,
            scrollable_x: false,
            scrollable_y: false,
            scrollbar_width: 0.0,
            scrollbar_gutter: 0.0,
            scrollbar_thumb: Rgba::TRANSPARENT,
            scrollbar_track: Rgba::TRANSPARENT,
            scrollbar_radius: 0.0,
        });
    }
}

fn layer_bounds(env: &PaintEnv, id: NodeId, width: f32, height: f32) -> (f32, f32, f32, f32) {
    let blur = env.resolved[id.0 as usize].blur;
    let mut min_x = -blur;
    let mut min_y = -blur;
    let mut max_x = width + blur;
    let mut max_y = height + blur;
    fn walk(
        env: &PaintEnv,
        id: NodeId,
        ox: f32,
        oy: f32,
        min_x: &mut f32,
        min_y: &mut f32,
        max_x: &mut f32,
        max_y: &mut f32,
    ) {
        let Some(node) = env.snapshot.nodes.get(id.0 as usize) else {
            return;
        };
        for child in &node.children {
            let Some(taffy_id) = env.taffy_ids[child.0 as usize] else {
                continue;
            };
            let Ok(layout) = env.tree.layout(taffy_id) else {
                continue;
            };
            let cx = ox + layout.location.x;
            let cy = oy + layout.location.y;
            let blur = env.resolved[child.0 as usize].blur;
            *min_x = min_x.min(cx - blur);
            *min_y = min_y.min(cy - blur);
            *max_x = max_x.max(cx + layout.size.width + blur);
            *max_y = max_y.max(cy + layout.size.height + blur);
            walk(env, *child, cx, cy, min_x, min_y, max_x, max_y);
        }
    }
    walk(
        env, id, 0.0, 0.0, &mut min_x, &mut min_y, &mut max_x, &mut max_y,
    );
    let min_x = min_x.floor();
    let min_y = min_y.floor();
    let max_x = max_x.ceil();
    let max_y = max_y.ceil();
    (
        min_x,
        min_y,
        (max_x - min_x).max(1.0),
        (max_y - min_y).max(1.0),
    )
}

fn layer_stamp(env: &PaintEnv, root: NodeId) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    fn mix(hash: &mut u64, value: u64) {
        *hash ^= value;
        *hash = hash.wrapping_mul(0x100000001b3);
    }
    fn walk(env: &PaintEnv, id: NodeId, root: NodeId, hash: &mut u64) {
        let style = &env.resolved[id.0 as usize];
        let kind = &env.snapshot.nodes[id.0 as usize].kind;
        if let SnapKind::Element {
            class_name, image, ..
        } = kind
        {
            let class = if id == root {
                tailwind::pose_stripped_class(class_name)
            } else {
                class_name.clone()
            };
            for byte in class.bytes() {
                mix(hash, byte as u64);
            }
            if let Some(image) = image {
                mix(hash, std::sync::Arc::as_ptr(image) as u64);
                mix(hash, image.rgba.len() as u64);
            }
        }
        if let SnapKind::Text { text } = kind {
            for byte in text.bytes() {
                mix(hash, byte as u64);
            }
        }
        if let Some(taffy_id) = env.taffy_ids[id.0 as usize] {
            if let Ok(layout) = env.tree.layout(taffy_id) {
                mix(hash, layout.size.width.to_bits() as u64);
                mix(hash, layout.size.height.to_bits() as u64);
            }
        }
        mix(hash, style.background.r as u64);
        mix(hash, style.background.g as u64);
        mix(hash, style.background.b as u64);
        mix(hash, style.background.a as u64);
        mix(hash, style.radius.to_bits() as u64);
        mix(hash, style.blur.to_bits() as u64);
        mix(hash, style.ring.to_bits() as u64);
        if let Some(color) = style.ring_color {
            mix(hash, color.a as u64 | ((color.r as u64) << 8));
        }
        if let Some(gradient) = &style.gradient {
            mix(hash, gradient.from.r as u64);
            mix(hash, gradient.to.b as u64);
            mix(hash, gradient.dir as u64);
        }
        for child in &env.snapshot.nodes[id.0 as usize].children {
            walk(env, *child, root, hash);
        }
    }
    walk(env, root, root, &mut hash);
    hash
}

fn paint_background(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    style: &Resolved,
    opacity: f32,
    scale: f32,
    mask: Option<&Mask>,
) {
    if style.blur > 0.0 && style.background.a > 0 {
        fill_radial(
            pixmap,
            x,
            y,
            w,
            h,
            style.background.with_alpha(opacity),
            style.blur,
            scale,
            mask,
        );
        return;
    }
    if let Some(gradient) = style.gradient {
        fill_linear(
            pixmap,
            x,
            y,
            w,
            h,
            style.radius,
            gradient,
            opacity,
            scale,
            mask,
        );
        return;
    }
    fill_box(
        pixmap,
        x,
        y,
        w,
        h,
        style.radius,
        style.background.with_alpha(opacity),
        scale,
        mask,
    );
}

fn fill_linear(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    gradient: LinearGrad,
    opacity: f32,
    scale: f32,
    mask: Option<&Mask>,
) {
    if w <= 0.0 || h <= 0.0 || opacity <= 0.0 {
        return;
    }
    let (dx, dy): (f32, f32) = match gradient.dir {
        GradDir::Right => (1.0, 0.0),
        GradDir::Left => (-1.0, 0.0),
        GradDir::Down => (0.0, 1.0),
        GradDir::Up => (0.0, -1.0),
        GradDir::UpRight => (1.0, -1.0),
        GradDir::UpLeft => (-1.0, -1.0),
        GradDir::DownRight => (1.0, 1.0),
        GradDir::DownLeft => (-1.0, 1.0),
    };
    let len = (dx * dx + dy * dy).sqrt().max(0.001_f32);
    let (dx, dy) = (dx / len, dy / len);
    let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)];
    let mut min_p = f32::MAX;
    let mut max_p = f32::MIN;
    for (cx, cy) in corners {
        let p = cx * dx + cy * dy;
        min_p = min_p.min(p);
        max_p = max_p.max(p);
    }
    let span = (max_p - min_p).max(0.001);
    let x0 = (x * scale).floor().max(0.0) as i32;
    let y0 = (y * scale).floor().max(0.0) as i32;
    let x1 = ((x + w) * scale).ceil().min(pixmap.width() as f32) as i32;
    let y1 = ((y + h) * scale).ceil().min(pixmap.height() as f32) as i32;
    let mask_data = mask.map(|mask| mask.data());
    let mask_width = mask.map(|mask| mask.width()).unwrap_or(0);
    let pixmap_width = pixmap.width() as i32;
    let pixels = pixmap.pixels_mut();
    for py in y0..y1 {
        for px in x0..x1 {
            let logical_x = (px as f32 + 0.5) / scale;
            let logical_y = (py as f32 + 0.5) / scale;
            if !inside_round(logical_x - x, logical_y - y, w, h, radius) {
                continue;
            }
            if let Some(data) = mask_data {
                let index = py as u32 * mask_width + px as u32;
                if data.get(index as usize).copied().unwrap_or(0) == 0 {
                    continue;
                }
            }
            let p = ((logical_x - x) * dx + (logical_y - y) * dy - min_p) / span;
            let color = gradient_color(gradient, p).with_alpha(opacity);
            if color.a == 0 {
                continue;
            }
            let index = (py as u32 * pixmap_width as u32 + px as u32) as usize;
            blend_straight(&mut pixels[index], color.r, color.g, color.b, color.a);
        }
    }
}

fn gradient_color(gradient: LinearGrad, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    if let Some(via) = gradient.via {
        if t < 0.5 {
            mix_rgba(gradient.from, via, t * 2.0)
        } else {
            mix_rgba(via, gradient.to, (t - 0.5) * 2.0)
        }
    } else {
        mix_rgba(gradient.from, gradient.to, t)
    }
}

fn mix_rgba(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let mix = |from: u8, to: u8| (from as f32 + (to as f32 - from as f32) * t).round() as u8;
    Rgba {
        r: mix(a.r, b.r),
        g: mix(a.g, b.g),
        b: mix(a.b, b.b),
        a: mix(a.a, b.a),
    }
}

fn fill_radial(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: Rgba,
    blur: f32,
    scale: f32,
    mask: Option<&Mask>,
) {
    if w <= 0.0 || h <= 0.0 || color.a == 0 {
        return;
    }
    let cx = x + w * 0.5;
    let cy = y + h * 0.5;
    let rx = (w * 0.5).max(1.0);
    let ry = (h * 0.5).max(1.0);
    let feather = blur / rx.min(ry);
    let outer = 1.0 + feather;
    let pad = blur;
    let x0 = ((x - pad) * scale).floor().max(0.0) as i32;
    let y0 = ((y - pad) * scale).floor().max(0.0) as i32;
    let x1 = ((x + w + pad) * scale).ceil().min(pixmap.width() as f32) as i32;
    let y1 = ((y + h + pad) * scale).ceil().min(pixmap.height() as f32) as i32;
    let mask_data = mask.map(|mask| mask.data());
    let mask_width = mask.map(|mask| mask.width()).unwrap_or(0);
    let pixmap_width = pixmap.width() as i32;
    let pixels = pixmap.pixels_mut();
    let sigma = 0.55 + 0.35 * (feather / (1.0 + feather));
    for py in y0..y1 {
        for px in x0..x1 {
            if let Some(data) = mask_data {
                let index = py as u32 * mask_width + px as u32;
                if data.get(index as usize).copied().unwrap_or(0) == 0 {
                    continue;
                }
            }
            let logical_x = (px as f32 + 0.5) / scale;
            let logical_y = (py as f32 + 0.5) / scale;
            let nx = (logical_x - cx) / rx;
            let ny = (logical_y - cy) / ry;
            let radius = (nx * nx + ny * ny).sqrt();
            if radius >= outer {
                continue;
            }
            let keep = (-(radius * radius) / (2.0 * sigma * sigma)).exp();
            let alpha = (color.a as f32 * keep).round() as u8;
            if alpha == 0 {
                continue;
            }
            let index = (py as u32 * pixmap_width as u32 + px as u32) as usize;
            blend_straight(&mut pixels[index], color.r, color.g, color.b, alpha);
        }
    }
}

fn blend_straight(dst: &mut tiny_skia::PremultipliedColorU8, r: u8, g: u8, b: u8, a: u8) {
    let inv = 255 - a as u16;
    let red = (r as u16 * a as u16) / 255 + (dst.red() as u16 * inv) / 255;
    let green = (g as u16 * a as u16) / 255 + (dst.green() as u16 * inv) / 255;
    let blue = (b as u16 * a as u16) / 255 + (dst.blue() as u16 * inv) / 255;
    let alpha = a as u16 + (dst.alpha() as u16 * inv) / 255;
    if let Some(next) = tiny_skia::PremultipliedColorU8::from_rgba(
        red.min(255) as u8,
        green.min(255) as u8,
        blue.min(255) as u8,
        alpha.min(255) as u8,
    ) {
        *dst = next;
    }
}

fn stroke_ring(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    style: &Resolved,
    opacity: f32,
    scale: f32,
    mask: Option<&Mask>,
) {
    if style.ring <= 0.0 || w <= 0.0 || h <= 0.0 {
        return;
    }
    let color = style
        .ring_color
        .unwrap_or(Rgba {
            r: 59,
            g: 130,
            b: 246,
            a: 128,
        })
        .with_alpha(opacity);
    if color.a == 0 {
        return;
    }
    let grow = style.ring * 0.5;
    let Some(path) = rounded_path(
        (x - grow) * scale,
        (y - grow) * scale,
        (w + style.ring) * scale,
        (h + style.ring) * scale,
        (style.radius + grow) * scale,
    ) else {
        return;
    };
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color(tiny_skia::Color::from_rgba8(
        color.r, color.g, color.b, color.a,
    ));
    let stroke = Stroke {
        width: style.ring * scale,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), mask);
}

fn paint_image(
    pixmap: &mut Pixmap,
    image: &Raster,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    fit: ObjectFit,
    object_y: f32,
    radius: f32,
    opacity: f32,
    scale: f32,
    mask: Option<&Mask>,
) {
    if w <= 0.0 || h <= 0.0 || opacity <= 0.0 || image.width == 0 || image.height == 0 {
        return;
    }
    let source_w = image.width as f32;
    let source_h = image.height as f32;
    let fit_scale = match fit {
        ObjectFit::Fill => (w / source_w, h / source_h),
        ObjectFit::Contain => {
            let uniform = (w / source_w).min(h / source_h);
            (uniform, uniform)
        }
        ObjectFit::Cover => {
            let uniform = (w / source_w).max(h / source_h);
            (uniform, uniform)
        }
    };
    let drawn_w = source_w * fit_scale.0;
    let drawn_h = source_h * fit_scale.1;
    let origin_x = x + (w - drawn_w) * 0.5;
    let origin_y = y + (h - drawn_h) * object_y.clamp(0.0, 1.0);
    let x0 = (x * scale).floor().max(0.0) as i32;
    let y0 = (y * scale).floor().max(0.0) as i32;
    let x1 = ((x + w) * scale).ceil().min(pixmap.width() as f32) as i32;
    let y1 = ((y + h) * scale).ceil().min(pixmap.height() as f32) as i32;
    let radius = radius.max(0.0);
    let pixmap_width = pixmap.width() as i32;
    let pixels = pixmap.pixels_mut();
    let mask_data = mask.map(|mask| mask.data());
    let mask_width = mask.map(|mask| mask.width()).unwrap_or(0);

    for py in y0..y1 {
        for px in x0..x1 {
            let logical_x = (px as f32 + 0.5) / scale;
            let logical_y = (py as f32 + 0.5) / scale;
            if !inside_round(logical_x - x, logical_y - y, w, h, radius) {
                continue;
            }
            if let Some(data) = mask_data {
                let index = py as u32 * mask_width + px as u32;
                if data.get(index as usize).copied().unwrap_or(0) == 0 {
                    continue;
                }
            }
            let sample_x = (logical_x - origin_x) / fit_scale.0;
            let sample_y = (logical_y - origin_y) / fit_scale.1;
            if sample_x < 0.0 || sample_y < 0.0 || sample_x >= source_w || sample_y >= source_h {
                continue;
            }
            let sx = (sample_x as u32).min(image.width - 1);
            let sy = (sample_y as u32).min(image.height - 1);
            let offset = ((sy * image.width + sx) * 4) as usize;
            let sample = &image.rgba[offset..offset + 4];
            let alpha = ((sample[3] as f32 * opacity).round() as u16).min(255) as u8;
            if alpha == 0 {
                continue;
            }
            let index = (py as u32 * pixmap_width as u32 + px as u32) as usize;
            let dst = &mut pixels[index];
            let inv = 255 - alpha as u16;
            let red = ((sample[0] as u16 * alpha as u16) / 255) + (dst.red() as u16 * inv) / 255;
            let green =
                ((sample[1] as u16 * alpha as u16) / 255) + (dst.green() as u16 * inv) / 255;
            let blue = ((sample[2] as u16 * alpha as u16) / 255) + (dst.blue() as u16 * inv) / 255;
            let out_a = alpha as u16 + (dst.alpha() as u16 * inv) / 255;
            if let Some(next) = tiny_skia::PremultipliedColorU8::from_rgba(
                red.min(255) as u8,
                green.min(255) as u8,
                blue.min(255) as u8,
                out_a.min(255) as u8,
            ) {
                *dst = next;
            }
        }
    }
}

fn inside_round(x: f32, y: f32, w: f32, h: f32, radius: f32) -> bool {
    if x < 0.0 || y < 0.0 || x > w || y > h {
        return false;
    }
    if radius < 0.5 {
        return true;
    }
    let radius = radius.min(w * 0.5).min(h * 0.5);
    let dx = if x < radius {
        radius - x
    } else if x > w - radius {
        x - (w - radius)
    } else {
        0.0
    };
    let dy = if y < radius {
        radius - y
    } else if y > h - radius {
        y - (h - radius)
    } else {
        0.0
    };
    dx * dx + dy * dy <= radius * radius
}

fn fill_box(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    color: Rgba,
    scale: f32,
    mask: Option<&Mask>,
) {
    if color.a == 0 || w <= 0.0 || h <= 0.0 {
        return;
    }
    let Some(path) = rounded_path(x * scale, y * scale, w * scale, h * scale, radius * scale)
    else {
        return;
    };
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color(tiny_skia::Color::from_rgba8(
        color.r, color.g, color.b, color.a,
    ));
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        mask,
    );
}

fn stroke_border(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    border: &taffy::Rect<f32>,
    color: Rgba,
    scale: f32,
    mask: Option<&Mask>,
) {
    if color.a == 0 {
        return;
    }
    let top = border.top;
    let right = border.right;
    let bottom = border.bottom;
    let left = border.left;
    if top <= 0.0 && right <= 0.0 && bottom <= 0.0 && left <= 0.0 {
        return;
    }
    if (top - right).abs() < 0.1
        && (top - bottom).abs() < 0.1
        && (top - left).abs() < 0.1
        && top > 0.0
    {
        let inset = top * 0.5;
        let Some(path) = rounded_path(
            (x + inset) * scale,
            (y + inset) * scale,
            (w - top).max(0.0) * scale,
            (h - top).max(0.0) * scale,
            (radius - inset).max(0.0) * scale,
        ) else {
            return;
        };
        let mut paint = Paint::default();
        paint.anti_alias = true;
        paint.set_color(tiny_skia::Color::from_rgba8(
            color.r, color.g, color.b, color.a,
        ));
        let stroke = Stroke {
            width: top * scale,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), mask);
        return;
    }
    fill_box(pixmap, x, y, w, top, 0.0, color, scale, mask);
    fill_box(
        pixmap,
        x,
        y + h - bottom,
        w,
        bottom,
        0.0,
        color,
        scale,
        mask,
    );
    fill_box(pixmap, x, y, left, h, 0.0, color, scale, mask);
    fill_box(pixmap, x + w - right, y, right, h, 0.0, color, scale, mask);
}

fn clip_mask(pixmap: &Pixmap, clip: LogicalClip, scale: f32) -> Option<Mask> {
    let mut mask = Mask::new(pixmap.width(), pixmap.height())?;
    let path = rounded_path(
        clip.x * scale,
        clip.y * scale,
        clip.w * scale,
        clip.h * scale,
        0.0,
    )?;
    mask.fill_path(&path, FillRule::Winding, false, Transform::identity());
    Some(mask)
}

fn rounded_path(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<tiny_skia::Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = radius.max(0.0).min(w * 0.5).min(h * 0.5);
    let mut path = PathBuilder::new();
    if r < 0.5 {
        path.move_to(x, y);
        path.line_to(x + w, y);
        path.line_to(x + w, y + h);
        path.line_to(x, y + h);
        path.close();
        return path.finish();
    }
    path.move_to(x + r, y);
    path.line_to(x + w - r, y);
    path.quad_to(x + w, y, x + w, y + r);
    path.line_to(x + w, y + h - r);
    path.quad_to(x + w, y + h, x + w - r, y + h);
    path.line_to(x + r, y + h);
    path.quad_to(x, y + h, x, y + h - r);
    path.line_to(x, y + r);
    path.quad_to(x, y, x + r, y);
    path.close();
    path.finish()
}

fn intersect(a: LogicalClip, b: LogicalClip) -> LogicalClip {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.w).min(b.x + b.w);
    let bottom = (a.y + a.h).min(b.y + b.h);
    LogicalClip {
        x,
        y,
        w: (right - x).max(0.0),
        h: (bottom - y).max(0.0),
    }
}

fn straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    let mut out = vec![0u8; (pixmap.width() * pixmap.height() * 4) as usize];
    for (index, pixel) in pixmap.pixels().iter().enumerate() {
        let alpha = pixel.alpha();
        if alpha == 0 {
            continue;
        }
        let offset = index * 4;
        if alpha == 255 {
            out[offset] = pixel.red();
            out[offset + 1] = pixel.green();
            out[offset + 2] = pixel.blue();
            out[offset + 3] = 255;
        } else {
            out[offset] = ((pixel.red() as u16 * 255) / alpha as u16) as u8;
            out[offset + 1] = ((pixel.green() as u16 * 255) / alpha as u16) as u8;
            out[offset + 2] = ((pixel.blue() as u16 * 255) / alpha as u16) as u8;
            out[offset + 3] = alpha;
        }
    }
    out
}

fn copy_xrgb(pixmap: &Pixmap, out: &mut [u32]) {
    let src = pixmap.data();
    let count = (pixmap.width() as usize)
        .saturating_mul(pixmap.height() as usize)
        .min(out.len());
    for index in 0..count {
        let pixel = index * 4;
        let alpha = src[pixel + 3];
        let (red, green, blue) = if alpha == 255 {
            (src[pixel], src[pixel + 1], src[pixel + 2])
        } else if alpha == 0 {
            (0, 0, 0)
        } else {
            (
                ((src[pixel] as u16 * 255) / alpha as u16) as u8,
                ((src[pixel + 1] as u16 * 255) / alpha as u16) as u8,
                ((src[pixel + 2] as u16 * 255) / alpha as u16) as u8,
            )
        };
        out[index] = ((red as u32) << 16) | ((green as u32) << 8) | blue as u32;
    }
}

fn copy_argb(pixmap: &Pixmap, opacity: f32, out: &mut [u32]) {
    let src = pixmap.data();
    let count = (pixmap.width() as usize)
        .saturating_mul(pixmap.height() as usize)
        .min(out.len());
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity == 1.0 {
        // The pixmap is already premultiplied. Only its channel order changes.
        for (pixel, dest) in src.chunks_exact(4).zip(&mut out[..count]) {
            *dest = u32::from_be_bytes([pixel[3], pixel[0], pixel[1], pixel[2]]);
        }
        return;
    }
    // Preserve the exact rounding for fractional opacity, but compute it once
    // per possible byte value instead of four times per screen pixel.
    let channels: [u32; 256] =
        std::array::from_fn(|value| (value as f32 * opacity).round() as u32);
    for (pixel, dest) in src.chunks_exact(4).zip(&mut out[..count]) {
        *dest = (channels[pixel[3] as usize] << 24)
            | (channels[pixel[0] as usize] << 16)
            | (channels[pixel[1] as usize] << 8)
            | channels[pixel[2] as usize];
    }
}

fn hover_chain(snapshot: &Snapshot, hover: Option<NodeId>) -> HashSet<u32> {
    let mut set = HashSet::new();
    let mut current = hover;
    while let Some(id) = current {
        if !set.insert(id.0) {
            break;
        }
        current = snapshot
            .nodes
            .get(id.0 as usize)
            .and_then(|node| node.parent);
    }
    set
}

#[cfg(test)]
mod output_tests {
    use super::*;

    #[test]
    fn transparent_output_preserves_premultiplication_and_opacity_rounding() {
        let mut pixmap = Pixmap::new(256, 1).unwrap();
        for (alpha, pixel) in pixmap.data_mut().chunks_exact_mut(4).enumerate() {
            pixel.copy_from_slice(&[alpha as u8, (alpha / 2) as u8, (alpha / 3) as u8, alpha as u8]);
        }
        for opacity in [0.0, 0.1, 0.333, 0.85, 0.999, 1.0] {
            let mut out = vec![0; 256];
            copy_argb(&pixmap, opacity, &mut out);
            for (pixel, actual) in pixmap.data().chunks_exact(4).zip(out) {
                let scale = |byte: u8| (byte as f32 * opacity).round() as u32;
                let expected = (scale(pixel[3]) << 24) | (scale(pixel[0]) << 16)
                    | (scale(pixel[1]) << 8) | scale(pixel[2]);
                assert_eq!(actual, expected, "opacity {opacity}, pixel {pixel:?}");
            }
        }
    }
}
