//! Resolve Tailwind classes, lay the tree out with Taffy, and paint it.

use std::cell::RefCell;
use std::collections::HashSet;

use taffy::prelude::*;
use tiny_skia::{FillRule, Mask, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::color::Rgba;
use crate::dom::{Document, NodeId, Raster, SnapKind, Snapshot};
use crate::error::SweetError;
use crate::tailwind::{self, CursorStyle, ObjectFit, ResolveCtx, Resolved};
use crate::text::{Clip, FontBook};

#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub width: f32,
    pub height: f32,
    pub scale: f32,
    pub hover: Option<NodeId>,
    pub focus: Option<NodeId>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: 800.0,
            height: 600.0,
            scale: 1.0,
            hover: None,
            focus: None,
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
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub boxes: Vec<LayoutBox>,
    pub fields: Vec<TextField>,
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
        for b in self.boxes.iter().rev() {
            if !b.hit_test || b.width <= 0.0 || b.height <= 0.0 {
                continue;
            }
            let in_box = x >= b.x && y >= b.y && x < b.x + b.width && y < b.y + b.height;
            let in_clip = x >= b.clip_x
                && y >= b.clip_y
                && x < b.clip_x + b.clip_w
                && y < b.clip_y + b.clip_h;
            if in_box && in_clip {
                return Some(b.id);
            }
        }
        None
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
    let (pixmap, boxes, fields) = draw_scene(document, options)?;
    let frame = Frame {
        width: pixmap.width(),
        height: pixmap.height(),
        rgba: straight_rgba(&pixmap),
        boxes,
        fields,
    };
    recycle_pixmap(pixmap);
    Ok(frame)
}

/// Paint `0x00RRGGBB` pixels into `out`. `out` must hold [`physical_size`] pixels.
pub fn render_xrgb(
    document: &Document,
    options: &RenderOptions,
    out: &mut [u32],
) -> Result<(Vec<LayoutBox>, Vec<TextField>), SweetError> {
    let (pixmap, boxes, fields) = draw_scene(document, options)?;
    let count = pixmap.width() as usize * pixmap.height() as usize;
    if out.len() < count {
        recycle_pixmap(pixmap);
        return Err(SweetError::new("frame buffer is smaller than the viewport"));
    }
    copy_xrgb(&pixmap, &mut out[..count]);
    recycle_pixmap(pixmap);
    Ok((boxes, fields))
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

fn draw_scene(
    document: &Document,
    options: &RenderOptions,
) -> Result<(Pixmap, Vec<LayoutBox>, Vec<TextField>), SweetError> {
    let snapshot = document.snapshot();
    let (width, height, scale, phys_w, phys_h) = viewport(options);
    let hovered = hover_chain(&snapshot, options.hover);
    let resolved = resolve_tree(&snapshot, &hovered, options.focus, width, height);
    let (mut tree, taffy_ids) = build_tree(&snapshot, &resolved, document.fonts.as_ref())?;
    let root = taffy_ids[snapshot.body.0 as usize]
        .ok_or_else(|| SweetError::new("body produced no box"))?;
    tree_layout(&mut tree, root, width, height, document.fonts.as_ref())?;

    let mut pixmap = lease_pixmap(phys_w, phys_h)?;
    pixmap.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));

    let mut boxes = Vec::new();
    let mut fields = Vec::new();
    let root_clip = LogicalClip {
        x: 0.0,
        y: 0.0,
        w: width,
        h: height,
    };
    if taffy_ids[snapshot.body.0 as usize].is_some() {
        paint_node(
            &mut pixmap,
            &snapshot,
            &resolved,
            taffy_ids.as_slice(),
            &tree,
            document.fonts.as_ref(),
            snapshot.body,
            0.0,
            0.0,
            1.0,
            root_clip,
            scale,
            None,
            &mut boxes,
            &mut fields,
        );
    }
    Ok((pixmap, boxes, fields))
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

fn paint_node(
    pixmap: &mut Pixmap,
    snapshot: &Snapshot,
    resolved: &[Resolved],
    taffy_ids: &[Option<taffy::NodeId>],
    tree: &TaffyTree<Option<LeafMeasure>>,
    fonts: &FontBook,
    id: NodeId,
    parent_x: f32,
    parent_y: f32,
    parent_opacity: f32,
    clip: LogicalClip,
    scale: f32,
    mask: Option<&Mask>,
    boxes: &mut Vec<LayoutBox>,
    fields: &mut Vec<TextField>,
) {
    let Some(taffy_id) = taffy_ids[id.0 as usize] else {
        return;
    };
    let Ok(layout) = tree.layout(taffy_id) else {
        return;
    };
    let style = &resolved[id.0 as usize];
    let x = parent_x + layout.location.x;
    let y = parent_y + layout.location.y;
    let w = layout.size.width;
    let h = layout.size.height;
    let opacity = (parent_opacity * style.opacity).clamp(0.0, 1.0);

    boxes.push(LayoutBox {
        id,
        parent: snapshot.nodes[id.0 as usize].parent,
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
    });

    let border = &layout.border;
    let padding = &layout.padding;
    let color = style.color.with_alpha(opacity);
    let background = style.background.with_alpha(opacity);
    let border_color = style
        .border_color
        .unwrap_or(style.color)
        .with_alpha(opacity);
    fill_box(pixmap, x, y, w, h, style.radius, background, scale, mask);
    if let SnapKind::Element {
        control: Some(control),
        disabled,
        ..
    } = &snapshot.nodes[id.0 as usize].kind
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
        fields.push(TextField {
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
    stroke_border(
        pixmap,
        x,
        y,
        w,
        h,
        style.radius,
        border,
        border_color,
        scale,
        mask,
    );

    if let SnapKind::Element {
        image: Some(image), ..
    } = &snapshot.nodes[id.0 as usize].kind
    {
        let content_x = x + border.left + padding.left;
        let content_y = y + border.top + padding.top;
        let content_w = (w - border.left - border.right - padding.left - padding.right).max(0.0);
        let content_h = (h - border.top - border.bottom - padding.top - padding.bottom).max(0.0);
        paint_image(
            pixmap,
            image,
            content_x,
            content_y,
            content_w,
            content_h,
            style.object_fit,
            style.radius,
            opacity,
            scale,
            mask,
        );
    }

    let mut child_clip = clip;
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
    }

    if let SnapKind::Text { text } = &snapshot.nodes[id.0 as usize].kind {
        let content_x = x + border.left + padding.left;
        let content_y = y + border.top + padding.top;
        let content_w = (w - border.left - border.right - padding.left - padding.right).max(0.0);
        let content_h = (h - border.top - border.bottom - padding.top - padding.bottom).max(0.0);
        let max_width = if style.nowrap {
            f32::INFINITY
        } else {
            content_w
        };
        fonts.draw(
            pixmap,
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
        let _ = content_h;
    }

    let mut children: Vec<NodeId> = snapshot.nodes[id.0 as usize].children.clone();
    children.sort_by(|a, b| {
        let za = resolved[a.0 as usize].z;
        let zb = resolved[b.0 as usize].z;
        za.cmp(&zb).then(a.0.cmp(&b.0))
    });
    if child_clip != clip && (child_clip.w <= 0.0 || child_clip.h <= 0.0) {
        return;
    }
    if child_clip != clip {
        let Some(child_mask) = clip_mask(pixmap, child_clip, scale) else {
            return;
        };
        for child in children {
            paint_node(
                pixmap,
                snapshot,
                resolved,
                taffy_ids,
                tree,
                fonts,
                child,
                x,
                y,
                opacity,
                child_clip,
                scale,
                Some(&child_mask),
                boxes,
                fields,
            );
        }
    } else {
        for child in children {
            paint_node(
                pixmap, snapshot, resolved, taffy_ids, tree, fonts, child, x, y, opacity,
                child_clip, scale, mask, boxes, fields,
            );
        }
    }
}

fn paint_image(
    pixmap: &mut Pixmap,
    image: &Raster,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    fit: ObjectFit,
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
    let origin_y = y + (h - drawn_h) * 0.5;
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
    for (index, pixel) in pixmap.pixels().iter().enumerate() {
        let alpha = pixel.alpha();
        let (red, green, blue) = if alpha == 0 {
            (0, 0, 0)
        } else if alpha == 255 {
            (pixel.red(), pixel.green(), pixel.blue())
        } else {
            (
                ((pixel.red() as u16 * 255) / alpha as u16) as u8,
                ((pixel.green() as u16 * 255) / alpha as u16) as u8,
                ((pixel.blue() as u16 * 255) / alpha as u16) as u8,
            )
        };
        out[index] = ((red as u32) << 16) | ((green as u32) << 8) | blue as u32;
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
