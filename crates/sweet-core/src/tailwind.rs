//! Tailwind utility resolver.
//!
//! Class strings map onto Taffy style fields and paint attributes. There is no
//! CSS parser. Responsive utilities follow base utilities in breakpoint order;
//! equally ranked utilities follow class order, and `!` wins last.

use std::cell::Cell;

use crate::nerd_glyphs::glyph;
use taffy::prelude::*;
use taffy::style::Overflow;

use crate::color::{self, Rgba};
use crate::text::TextAlign;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorStyle {
    Default,
    Pointer,
}

/// How an image fills its content box. These are the Tailwind `object-*` utilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectFit {
    Fill,
    Contain,
    Cover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverflowMode {
    Visible,
    Clip,
    Hidden,
    Auto,
    Scroll,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverflowAnchorEnd {
    None,
    Auto,
    Length(f32),
    Percentage(f32),
}

/// A length that is either pixels or a fraction of the element's own border box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shift {
    pub px: f32,
    pub pct: f32,
}

impl Shift {
    pub const ZERO: Self = Self { px: 0.0, pct: 0.0 };

    pub fn is_zero(self) -> bool {
        self.px == 0.0 && self.pct == 0.0
    }
}

/// Closed transform list. Percent translations resolve against the border box at paint time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xform {
    pub tx: Shift,
    pub ty: Shift,
    pub tz: f32,
    pub rotate_y: f32,
    pub rotate_z: f32,
    pub skew_x: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub origin_x: f32,
    pub origin_y: f32,
}

impl Xform {
    pub const IDENTITY: Self = Self {
        tx: Shift::ZERO,
        ty: Shift::ZERO,
        tz: 0.0,
        rotate_y: 0.0,
        rotate_z: 0.0,
        skew_x: 0.0,
        scale_x: 1.0,
        scale_y: 1.0,
        origin_x: 0.5,
        origin_y: 0.5,
    };

    pub fn is_identity(self) -> bool {
        self.tx.is_zero()
            && self.ty.is_zero()
            && self.tz == 0.0
            && self.rotate_y == 0.0
            && self.rotate_z == 0.0
            && self.skew_x == 0.0
            && self.scale_x == 1.0
            && self.scale_y == 1.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    None,
    Spin { seconds: f32 },
    Float { seconds: f32 },
    Glow { seconds: f32 },
    Sheen { seconds: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ease {
    Linear,
    Out,
    InOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GradDir {
    Right,
    Left,
    Down,
    Up,
    UpRight,
    UpLeft,
    DownRight,
    DownLeft,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinearGrad {
    pub dir: GradDir,
    pub from: Rgba,
    pub via: Option<Rgba>,
    pub to: Rgba,
}

#[derive(Clone, Debug)]
pub struct Resolved {
    pub style: Style,
    /// Flex/grid item ordering from Tailwind `order-*` utilities.
    pub order: i32,
    pub color: Rgba,
    pub background: Rgba,
    pub border_color: Option<Rgba>,
    pub radius: f32,
    pub font_size: f32,
    pub font_weight: u16,
    pub line_height: f32,
    pub text_align: TextAlign,
    pub opacity: f32,
    pub cursor: CursorStyle,
    pub clip: bool,
    pub nowrap: bool,
    pub z: i32,
    pub hit_test: bool,
    pub object_fit: ObjectFit,
    /// 0 shows the top of a covered image, 1 the bottom, 0.5 the center.
    pub object_y: f32,
    pub xform: Xform,
    pub perspective: Option<f32>,
    pub preserve_3d: bool,
    pub transition: bool,
    pub transition_ms: f32,
    pub ease: Ease,
    pub motion: Motion,
    /// Blur radius in pixels. Painted as a radial falloff, then cached with the layer.
    pub blur: f32,
    pub gradient: Option<LinearGrad>,
    pub ring: f32,
    pub ring_color: Option<Rgba>,
    pub overflow_x: OverflowMode,
    pub overflow_y: OverflowMode,
    /// Pins a scroll container to its block end while it remains within the
    /// configured threshold. This is exposed through fake Tailwind utilities.
    pub overflow_anchor_end: OverflowAnchorEnd,
    /// Zero keeps bars invisible. A width utility enables the painted bar.
    pub scrollbar_width: f32,
    pub scrollbar_gutter_stable: bool,
    pub scrollbar_thumb: Rgba,
    pub scrollbar_track: Rgba,
    pub scrollbar_radius: f32,
    pub nerd_icon: Option<char>,
}

pub struct ResolveCtx {
    pub viewport_w: f32,
    pub viewport_h: f32,
    pub hovered: bool,
    pub focused: bool,
    pub dark: bool,
    pub disabled: bool,
}

#[derive(Clone, Copy)]
enum Len {
    Px(f32),
    Pct(f32),
    Auto,
    Viewport,
    MinContent,
    MaxContent,
}

const PREFIXES: &[&str] = &[
    "scrollbar-width",
    "scrollbar-gutter",
    "scrollbar-radius",
    "scrollbar-thumb",
    "scrollbar-track",
    "scrollbar-color",
    "scrollbar-w",
    "pointer-events",
    "perspective",
    "translate-x",
    "translate-y",
    "translate-z",
    "justify-items",
    "justify-self",
    "place-items",
    "place-content",
    "grid-cols",
    "grid-rows",
    "col-span",
    "row-span",
    "col-start",
    "row-start",
    "whitespace",
    "overflow",
    "overflow-x",
    "overflow-y",
    "overflow-anchor-end",
    "scrollbar",
    "max-w",
    "max-h",
    "min-w",
    "min-h",
    "justify",
    "content",
    "aspect",
    "leading",
    "rounded",
    "border",
    "blur",
    "ring",
    "from",
    "via",
    "to",
    "cursor",
    "opacity",
    "duration",
    "basis",
    "items",
    "font",
    "text",
    "grow",
    "shrink",
    "self",
    "gap-x",
    "gap-y",
    "inset",
    "gap",
    "px",
    "py",
    "pt",
    "pr",
    "pb",
    "pl",
    "mx",
    "my",
    "mt",
    "mr",
    "mb",
    "ml",
    "top",
    "right",
    "bottom",
    "left",
    "bg",
    "z",
    "order",
    "w",
    "h",
    "p",
    "m",
];

pub fn root_resolved(dark: bool) -> Resolved {
    let mut resolved = empty_resolved();
    resolved.color = if dark {
        color::palette("zinc", 50).unwrap()
    } else {
        color::palette("zinc", 900).unwrap()
    };
    resolved.font_size = 16.0;
    resolved.font_weight = 400;
    resolved.line_height = 24.0;
    resolved
}

pub fn inherit(parent: &Resolved) -> Resolved {
    let mut resolved = empty_resolved();
    resolved.color = parent.color;
    resolved.font_size = parent.font_size;
    resolved.font_weight = parent.font_weight;
    resolved.line_height = parent.line_height;
    resolved.text_align = parent.text_align;
    resolved.nowrap = parent.nowrap;
    resolved
}

thread_local! {
    static RESOLVE_VIEW: Cell<(f32, f32)> = const { Cell::new((0.0, 0.0)) };
}

pub fn apply_classes(resolved: &mut Resolved, class_name: &str, ctx: &ResolveCtx) {
    RESOLVE_VIEW.with(|view| view.set((ctx.viewport_w, ctx.viewport_h)));
    let mut utilities: Vec<_> = class_name.split_whitespace().map(parse_token)
        .filter(|(_, _, variants)| variants_match(variants, ctx)).collect();
    // Mobile-first responsive rules follow base rules, and larger breakpoints
    // follow smaller ones. Class attribute order must not undo `lg:hidden`.
    utilities.sort_by_key(|(important, _, variants)| {
        (*important, variants.iter().filter_map(|v| breakpoint(v)).max().unwrap_or(0), variants.len())
    });
    for (_, utility, _) in utilities {
        apply_utility(resolved, &utility, ctx);
    }
}

fn breakpoint(variant: &str) -> Option<u32> {
    Some(match variant {
        "sm" => 640, "md" => 768, "lg" => 1024, "xl" => 1280, "2xl" => 1536,
        _ => return None,
    })
}

fn empty_resolved() -> Resolved {
    Resolved {
        style: Style {
            display: Display::Block,
            box_sizing: BoxSizing::BorderBox,
            ..Style::default()
        },
        order: 0,
        color: color::Rgba::rgb(0x18181b),
        background: Rgba::TRANSPARENT,
        border_color: None,
        radius: 0.0,
        font_size: 16.0,
        font_weight: 400,
        line_height: 24.0,
        text_align: TextAlign::Left,
        opacity: 1.0,
        cursor: CursorStyle::Default,
        clip: false,
        nowrap: false,
        z: 0,
        hit_test: true,
        object_fit: ObjectFit::Fill,
        object_y: 0.5,
        xform: Xform::IDENTITY,
        perspective: None,
        preserve_3d: false,
        transition: false,
        transition_ms: 0.0,
        ease: Ease::Linear,
        motion: Motion::None,
        blur: 0.0,
        gradient: None,
        ring: 0.0,
        ring_color: None,
        overflow_x: OverflowMode::Visible,
        overflow_y: OverflowMode::Visible,
        overflow_anchor_end: OverflowAnchorEnd::None,
        scrollbar_width: 0.0,
        scrollbar_gutter_stable: false,
        scrollbar_thumb: color::palette("zinc", 500).unwrap(),
        scrollbar_track: Rgba::TRANSPARENT,
        scrollbar_radius: 2.0,
        nerd_icon: None,
    }
}

fn parse_token(token: &str) -> (bool, &str, Vec<&str>) {
    // Colons inside `[...]` belong to arbitrary values such as `[transform:translateX(1px)]`.
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth: i32 = 0;
    for (index, ch) in token.char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => {
                parts.push(&token[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&token[start..]);
    let mut utility = parts.pop().unwrap_or(token);
    let important = if let Some(rest) = utility.strip_prefix('!') {
        utility = rest;
        true
    } else {
        false
    };
    (important, utility, parts)
}

fn variants_match(variants: &[&str], ctx: &ResolveCtx) -> bool {
    variants.iter().all(|variant| match *variant {
        "hover" => ctx.hovered,
        "focus" => ctx.focused,
        "dark" => ctx.dark,
        "disabled" => ctx.disabled,
        "sm" => ctx.viewport_w >= 640.0,
        "md" => ctx.viewport_w >= 768.0,
        "lg" => ctx.viewport_w >= 1024.0,
        "xl" => ctx.viewport_w >= 1280.0,
        "2xl" => ctx.viewport_w >= 1536.0,
        _ => false,
    })
}

fn apply_utility(resolved: &mut Resolved, raw: &str, ctx: &ResolveCtx) {
    if let Some(inner) = raw
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        if let Some((property, value)) = inner.split_once(':') {
            apply_arbitrary(resolved, property, &value.replace('_', " "));
            return;
        }
    }
    let (negative, utility) = match raw.strip_prefix('-') {
        Some(rest) if !rest.is_empty() => (true, rest),
        _ => (false, raw),
    };
    if let Some(icon) = glyph(utility) {
        resolved.nerd_icon = Some(icon);
        return;
    }
    if utility == "nf" {
        return;
    }
    if apply_keyword(resolved, utility) {
        return;
    }
    if let Some((property, value)) = split_prefix(utility) {
        apply_value(resolved, property, value, negative, ctx);
    }
}

fn apply_keyword(resolved: &mut Resolved, utility: &str) -> bool {
    let style = &mut resolved.style;
    match utility {
        "block" => style.display = Display::Block,
        "flex" | "inline-flex" => style.display = Display::Flex,
        "grid" | "inline-grid" => style.display = Display::Grid,
        "hidden" => style.display = Display::None,
        "flex-row" => style.flex_direction = FlexDirection::Row,
        "flex-row-reverse" => style.flex_direction = FlexDirection::RowReverse,
        "flex-col" => style.flex_direction = FlexDirection::Column,
        "flex-col-reverse" => style.flex_direction = FlexDirection::ColumnReverse,
        "flex-wrap" => style.flex_wrap = FlexWrap::Wrap,
        "flex-nowrap" => style.flex_wrap = FlexWrap::NoWrap,
        "flex-wrap-reverse" => style.flex_wrap = FlexWrap::WrapReverse,
        "flex-1" => {
            style.flex_grow = 1.0;
            style.flex_shrink = 1.0;
            style.flex_basis = length(0.0);
        }
        "flex-auto" => {
            style.flex_grow = 1.0;
            style.flex_shrink = 1.0;
            style.flex_basis = auto();
        }
        "flex-initial" => {
            style.flex_grow = 0.0;
            style.flex_shrink = 1.0;
            style.flex_basis = auto();
        }
        "flex-none" => {
            style.flex_grow = 0.0;
            style.flex_shrink = 0.0;
            style.flex_basis = auto();
        }
        "grow" => style.flex_grow = 1.0,
        "shrink" => style.flex_shrink = 1.0,
        "static" | "relative" => style.position = Position::Relative,
        "absolute" | "fixed" => style.position = Position::Absolute,
        "border" => set_border_widths(style, 1.0, 1.0, 1.0, 1.0),
        "rounded" => resolved.radius = 4.0,
        "truncate" => {
            resolved.nowrap = true;
            resolved.overflow_x = OverflowMode::Hidden;
            resolved.overflow_y = OverflowMode::Hidden;
            resolved.clip = true;
            style.overflow = taffy::geometry::Point {
                x: Overflow::Hidden,
                y: Overflow::Hidden,
            };
        }
        "object-fill" => resolved.object_fit = ObjectFit::Fill,
        "object-contain" => resolved.object_fit = ObjectFit::Contain,
        "object-cover" => resolved.object_fit = ObjectFit::Cover,
        "object-top" => resolved.object_y = 0.0,
        "object-center" => resolved.object_y = 0.5,
        "object-bottom" => resolved.object_y = 1.0,
        "origin-center" => {
            resolved.xform.origin_x = 0.5;
            resolved.xform.origin_y = 0.5;
        }
        "origin-top" => resolved.xform.origin_y = 0.0,
        "origin-bottom" => resolved.xform.origin_y = 1.0,
        "origin-left" => resolved.xform.origin_x = 0.0,
        "origin-right" => resolved.xform.origin_x = 1.0,
        "transition" | "transition-all" => resolved.transition = true,
        "ease-linear" => resolved.ease = Ease::Linear,
        "ease-out" => resolved.ease = Ease::Out,
        "ease-in-out" => resolved.ease = Ease::InOut,
        "animate-spin" => resolved.motion = Motion::Spin { seconds: 1.0 },
        "animate-spin-slow" => resolved.motion = Motion::Spin { seconds: 16.0 },
        "animate-float" => resolved.motion = Motion::Float { seconds: 5.5 },
        "animate-glow" => resolved.motion = Motion::Glow { seconds: 5.5 },
        "animate-sheen" => resolved.motion = Motion::Sheen { seconds: 5.5 },
        "ring" => resolved.ring = 3.0,
        "blur" => resolved.blur = 8.0,
        _ => return false,
    }
    true
}

fn split_prefix(utility: &str) -> Option<(&str, &str)> {
    let mut best: Option<(&str, &str)> = None;
    for prefix in PREFIXES {
        if let Some(rest) = utility.strip_prefix(prefix) {
            if let Some(value) = rest.strip_prefix('-') {
                if !value.is_empty()
                    && best
                        .map(|(current, _)| prefix.len() > current.len())
                        .unwrap_or(true)
                {
                    best = Some((*prefix, value));
                }
            }
        }
    }
    best
}

fn apply_value(
    resolved: &mut Resolved,
    property: &str,
    value: &str,
    negative: bool,
    ctx: &ResolveCtx,
) {
    match property {
        "w" => {
            if let Some(len) = parse_len(value) {
                resolved.style.size.width = dim(len, ctx.viewport_w);
            }
        }
        "h" => {
            if let Some(len) = parse_len(value) {
                resolved.style.size.height = dim(len, ctx.viewport_h);
            }
        }
        "min-w" => {
            if let Some(len) = parse_len(value) {
                resolved.style.min_size.width = lpa(len, ctx.viewport_w);
            }
        }
        "min-h" => {
            if let Some(len) = parse_len(value) {
                resolved.style.min_size.height = lpa(len, ctx.viewport_h);
            }
        }
        "max-w" => {
            if let Some(px) = max_width_name(value) {
                resolved.style.max_size.width = length(px);
            } else if let Some(len) = parse_len(value) {
                resolved.style.max_size.width = lpa(len, ctx.viewport_w);
            }
        }
        "max-h" => {
            if let Some(len) = parse_len(value) {
                resolved.style.max_size.height = lpa(len, ctx.viewport_h);
            }
        }
        "p" => set_rect_lp(
            &mut resolved.style.padding,
            lp_all(parse_len(value), ctx, false),
        ),
        "px" => set_x_lp(
            &mut resolved.style.padding,
            parse_len(value).map(|len| lp(len, ctx.viewport_w)),
        ),
        "py" => set_y_lp(
            &mut resolved.style.padding,
            parse_len(value).map(|len| lp(len, ctx.viewport_h)),
        ),
        "pt" => assign_lp(&mut resolved.style.padding.top, value, ctx.viewport_h),
        "pr" => assign_lp(&mut resolved.style.padding.right, value, ctx.viewport_w),
        "pb" => assign_lp(&mut resolved.style.padding.bottom, value, ctx.viewport_h),
        "pl" => assign_lp(&mut resolved.style.padding.left, value, ctx.viewport_w),
        "m" => set_rect_lpa(
            &mut resolved.style.margin,
            lpa_all(parse_len(value), ctx, negative),
        ),
        "mx" => set_x_lpa(
            &mut resolved.style.margin,
            signed_lpa(value, ctx.viewport_w, negative),
        ),
        "my" => set_y_lpa(
            &mut resolved.style.margin,
            signed_lpa(value, ctx.viewport_h, negative),
        ),
        "mt" => assign_signed_lpa(
            &mut resolved.style.margin.top,
            value,
            ctx.viewport_h,
            negative,
        ),
        "mr" => assign_signed_lpa(
            &mut resolved.style.margin.right,
            value,
            ctx.viewport_w,
            negative,
        ),
        "mb" => assign_signed_lpa(
            &mut resolved.style.margin.bottom,
            value,
            ctx.viewport_h,
            negative,
        ),
        "ml" => assign_signed_lpa(
            &mut resolved.style.margin.left,
            value,
            ctx.viewport_w,
            negative,
        ),
        "gap" => {
            if let Some(len) = parse_len(value) {
                let x = lp(len, ctx.viewport_w);
                let y = lp(len, ctx.viewport_h);
                resolved.style.gap.width = x;
                resolved.style.gap.height = y;
            }
        }
        "gap-x" => {
            if let Some(len) = parse_len(value) {
                resolved.style.gap.width = lp(len, ctx.viewport_w);
            }
        }
        "gap-y" => {
            if let Some(len) = parse_len(value) {
                resolved.style.gap.height = lp(len, ctx.viewport_h);
            }
        }
        "inset" => apply_inset(&mut resolved.style, value, negative, ctx),
        "top" => assign_signed_lpa(
            &mut resolved.style.inset.top,
            value,
            ctx.viewport_h,
            negative,
        ),
        "right" => assign_signed_lpa(
            &mut resolved.style.inset.right,
            value,
            ctx.viewport_w,
            negative,
        ),
        "bottom" => assign_signed_lpa(
            &mut resolved.style.inset.bottom,
            value,
            ctx.viewport_h,
            negative,
        ),
        "left" => assign_signed_lpa(
            &mut resolved.style.inset.left,
            value,
            ctx.viewport_w,
            negative,
        ),
        "basis" => {
            if let Some(len) = parse_len(value) {
                resolved.style.flex_basis = dim(len, ctx.viewport_w);
            }
        }
        "grow" => {
            resolved.style.flex_grow = if value == "0" {
                0.0
            } else {
                value.parse().unwrap_or(1.0)
            }
        }
        "shrink" => {
            resolved.style.flex_shrink = if value == "0" {
                0.0
            } else {
                value.parse().unwrap_or(1.0)
            }
        }
        "items" => resolved.style.align_items = align_items(value),
        "self" => resolved.style.align_self = align_items(value),
        "justify" => resolved.style.justify_content = align_content(value),
        "justify-items" => resolved.style.justify_items = align_items(value),
        "justify-self" => resolved.style.justify_self = align_items(value),
        "content" => resolved.style.align_content = align_content(value),
        "place-items" => {
            let align = align_items(value);
            resolved.style.align_items = align;
            resolved.style.justify_items = align;
        }
        "place-content" => {
            let align = align_content(value);
            resolved.style.align_content = align;
            resolved.style.justify_content = align;
        }
        "grid-cols" => {
            if let Ok(count) = value.parse::<u16>() {
                resolved.style.grid_template_columns = (0..count).map(|_| fr(1.0)).collect();
            } else if let Some(tracks) = grid_tracks(value, ctx) {
                resolved.style.grid_template_columns = tracks;
            }
        }
        "grid-rows" => {
            if let Ok(count) = value.parse::<u16>() {
                resolved.style.grid_template_rows = (0..count).map(|_| fr(1.0)).collect();
            } else if let Some(tracks) = grid_tracks(value, ctx) {
                resolved.style.grid_template_rows = tracks;
            }
        }
        "col-span" => {
            if value == "full" {
                resolved.style.grid_column.start = line(1);
                resolved.style.grid_column.end = line(-1);
            } else if let Ok(count) = value.parse::<u16>() {
                resolved.style.grid_column.end = span(count);
            }
        }
        "row-span" => {
            if value == "full" {
                resolved.style.grid_row.start = line(1);
                resolved.style.grid_row.end = line(-1);
            } else if let Ok(count) = value.parse::<u16>() {
                resolved.style.grid_row.end = span(count);
            }
        }
        "col-start" => {
            if let Ok(index) = value.parse::<i16>() {
                resolved.style.grid_column.start = line(index);
            }
        }
        "row-start" => {
            if let Ok(index) = value.parse::<i16>() {
                resolved.style.grid_row.start = line(index);
            }
        }
        "text" => apply_text(resolved, value),
        "font" => {
            if let Some(weight) = font_weight(value) {
                resolved.font_weight = weight;
            }
        }
        "leading" => {
            if let Some(px) = line_height_px(value, resolved.font_size) {
                resolved.line_height = px;
            }
        }
        "bg" => {
            if let Some(dir) = value.strip_prefix("gradient-to-") {
                if let Some(dir) = grad_dir(dir) {
                    gradient(resolved).dir = dir;
                }
            } else if let Some(color) = color::parse_color(value) {
                resolved.background = color;
            }
        }
        "from" => {
            if let Some(color) = color::parse_color(value) {
                gradient(resolved).from = color;
            }
        }
        "via" => {
            if let Some(color) = color::parse_color(value) {
                gradient(resolved).via = Some(color);
            }
        }
        "to" => {
            if let Some(color) = color::parse_color(value) {
                gradient(resolved).to = color;
            }
        }
        "blur" => {
            if let Some(radius) = blur_radius(value) {
                resolved.blur = radius;
            }
        }
        "ring" => {
            if let Some(color) = color::parse_color(value) {
                resolved.ring_color = Some(color);
            } else if let Some(width) = ring_width(value) {
                resolved.ring = width;
            }
        }
        "duration" => {
            if let Ok(ms) = value.parse::<f32>() {
                resolved.transition_ms = ms;
            }
        }
        "perspective" => {
            let value = color::arbitrary(value).unwrap_or(value);
            if let Some(px) = parse_px(value).or_else(|| value.parse().ok()) {
                resolved.perspective = Some(px);
            }
        }
        "translate-x" => resolved.xform.tx = parse_shift(value, negative),
        "translate-y" => resolved.xform.ty = parse_shift(value, negative),
        "translate-z" => resolved.xform.tz = parse_shift(value, negative).px,
        "border" => apply_border(resolved, value),
        "rounded" => {
            if let Some(radius) = radius(value) {
                resolved.radius = radius;
            }
        }
        "opacity" => {
            if let Ok(step) = value.parse::<f32>() {
                resolved.opacity = (step / 100.0).clamp(0.0, 1.0);
            }
        }
        "z" => {
            let value = color::arbitrary(value).unwrap_or(value);
            if value == "auto" {
                resolved.z = 0;
            } else if let Ok(z) = value.parse::<i32>() {
                resolved.z = if negative { -z } else { z };
            }
        }
        "order" => {
            if value == "none" {
                resolved.order = 0;
            } else if value == "first" {
                resolved.order = -9999;
            } else if value == "last" {
                resolved.order = 9999;
            } else if let Ok(order) = value.parse::<i32>() {
                resolved.order = order;
            }
        }
        "overflow" => {
            if let Some(mode) = parse_overflow(value) {
                set_overflow(resolved, None, mode);
            }
        }
        "overflow-x" | "overflow-y" => {
            if let Some(mode) = parse_overflow(value) {
                set_overflow(
                    resolved,
                    Some(if property == "overflow-x" {
                        true
                    } else {
                        false
                    }),
                    mode,
                );
            }
        }
        "overflow-anchor-end" => {
            resolved.overflow_anchor_end = match value {
                "none" => OverflowAnchorEnd::None,
                "auto" => OverflowAnchorEnd::Auto,
                _ => match parse_len(value) {
                    Some(Len::Px(length)) => OverflowAnchorEnd::Length(length.max(0.0)),
                    Some(Len::Pct(percentage)) => {
                        OverflowAnchorEnd::Percentage(percentage.max(0.0))
                    }
                    _ => resolved.overflow_anchor_end,
                },
            };
        }
        "scrollbar-width" | "scrollbar-w" => {
            if let Some(width) = scrollbar_width(value) {
                resolved.scrollbar_width = width;
                sync_overflow_style(resolved);
            }
        }
        "scrollbar-gutter" => {
            resolved.scrollbar_gutter_stable = value == "stable";
            sync_overflow_style(resolved);
        }
        "scrollbar-color" | "scrollbar-thumb" => {
            if let Some(color) = color::parse_color(value) {
                resolved.scrollbar_thumb = color;
            }
        }
        "scrollbar-track" => {
            if let Some(color) = color::parse_color(value) {
                resolved.scrollbar_track = color;
            }
        }
        "scrollbar-radius" => {
            if let Some(px) = color::arbitrary(value)
                .and_then(parse_px)
                .or_else(|| parse_px(value))
            {
                resolved.scrollbar_radius = px.max(0.0);
            }
        }
        "whitespace" => match value {
            "nowrap" => resolved.nowrap = true,
            "normal" | "pre-wrap" => resolved.nowrap = false,
            _ => {}
        },
        "cursor" => match value {
            "pointer" => resolved.cursor = CursorStyle::Pointer,
            "default" => resolved.cursor = CursorStyle::Default,
            _ => {}
        },
        "aspect" => match value {
            "square" => resolved.style.aspect_ratio = Some(1.0),
            "video" => resolved.style.aspect_ratio = Some(16.0 / 9.0),
            _ => {}
        },
        "pointer-events" => match value {
            "none" => resolved.hit_test = false,
            "auto" => resolved.hit_test = true,
            _ => {}
        },
        _ => {}
    }
}

fn apply_text(resolved: &mut Resolved, value: &str) {
    if let Some((size, line)) = font_size(value) {
        resolved.font_size = size;
        resolved.line_height = line;
        return;
    }
    match value {
        "left" => resolved.text_align = TextAlign::Left,
        "center" => resolved.text_align = TextAlign::Center,
        "right" => resolved.text_align = TextAlign::Right,
        _ => {
            if let Some(inner) = color::arbitrary(value) {
                if let Some(px) = inner.strip_suffix("px").and_then(|n| n.parse().ok()) {
                    resolved.font_size = px;
                    resolved.line_height = px * 1.5;
                    return;
                }
            }
            if let Some(color) = color::parse_color(value) {
                resolved.color = color;
            }
        }
    }
}

fn apply_border(resolved: &mut Resolved, value: &str) {
    if let Some(width) = border_width_token(value) {
        set_border_widths(&mut resolved.style, width, width, width, width);
        return;
    }
    if let Some((sides, width)) = border_side(value) {
        let style = &mut resolved.style;
        if sides.contains('t') {
            style.border.top = length(width);
        }
        if sides.contains('r') {
            style.border.right = length(width);
        }
        if sides.contains('b') {
            style.border.bottom = length(width);
        }
        if sides.contains('l') {
            style.border.left = length(width);
        }
        return;
    }
    if let Some(color) = color::parse_color(value) {
        resolved.border_color = Some(color);
    }
}

fn apply_inset(style: &mut Style, value: &str, negative: bool, ctx: &ResolveCtx) {
    if let Some(rest) = value.strip_prefix("x-") {
        if let Some(len) = signed_lpa(rest, ctx.viewport_w, negative) {
            style.inset.left = len;
            style.inset.right = len;
        }
        return;
    }
    if let Some(rest) = value.strip_prefix("y-") {
        if let Some(len) = signed_lpa(rest, ctx.viewport_h, negative) {
            style.inset.top = len;
            style.inset.bottom = len;
        }
        return;
    }
    if let Some(len) = parse_len(value) {
        let len = if negative { negate(len) } else { len };
        style.inset.top = lpa(len, ctx.viewport_h);
        style.inset.right = lpa(len, ctx.viewport_w);
        style.inset.bottom = lpa(len, ctx.viewport_h);
        style.inset.left = lpa(len, ctx.viewport_w);
    }
}

fn border_width_token(value: &str) -> Option<f32> {
    Some(match value {
        "0" => 0.0,
        "2" => 2.0,
        "4" => 4.0,
        "8" => 8.0,
        _ => return None,
    })
}

fn border_side(value: &str) -> Option<(&'static str, f32)> {
    let (side, width) = match value.split_once('-') {
        Some((side, width)) => (side, border_width_token(width).unwrap_or(1.0)),
        None => (value, 1.0),
    };
    let sides = match side {
        "t" => "t",
        "r" => "r",
        "b" => "b",
        "l" => "l",
        "x" => "lr",
        "y" => "tb",
        _ => return None,
    };
    Some((sides, width))
}

fn set_border_widths(style: &mut Style, top: f32, right: f32, bottom: f32, left: f32) {
    style.border.top = length(top);
    style.border.right = length(right);
    style.border.bottom = length(bottom);
    style.border.left = length(left);
}

fn parse_overflow(value: &str) -> Option<OverflowMode> {
    Some(match value {
        "visible" => OverflowMode::Visible,
        "clip" => OverflowMode::Clip,
        "hidden" => OverflowMode::Hidden,
        "auto" => OverflowMode::Auto,
        "scroll" => OverflowMode::Scroll,
        _ => return None,
    })
}

fn scrollbar_width(value: &str) -> Option<f32> {
    Some(match value {
        "none" => 0.0,
        "thin" => 5.0,
        "auto" => 12.0,
        "1" => 4.0,
        "2" => 8.0,
        "3" => 12.0,
        other => color::arbitrary(other)
            .and_then(parse_px)
            .or_else(|| parse_px(other))?,
    })
}

fn set_overflow(resolved: &mut Resolved, horizontal: Option<bool>, mode: OverflowMode) {
    if horizontal != Some(false) {
        resolved.overflow_x = mode;
    }
    if horizontal != Some(true) {
        resolved.overflow_y = mode;
    }
    sync_overflow_style(resolved);
}

fn sync_overflow_style(resolved: &mut Resolved) {
    let to_taffy = |mode| match mode {
        OverflowMode::Visible => Overflow::Visible,
        OverflowMode::Clip => Overflow::Clip,
        OverflowMode::Hidden => Overflow::Hidden,
        OverflowMode::Auto => Overflow::Hidden,
        OverflowMode::Scroll => Overflow::Scroll,
    };
    let stable = resolved.scrollbar_gutter_stable;
    let gutter_width = if stable {
        if resolved.scrollbar_width > 0.0 {
            resolved.scrollbar_width
        } else {
            12.0
        }
    } else {
        0.0
    };
    resolved.style.overflow = taffy::geometry::Point {
        x: to_taffy(resolved.overflow_x),
        y: to_taffy(resolved.overflow_y),
    };
    resolved.style.scrollbar_width = gutter_width;
    resolved.clip = matches!(
        resolved.overflow_x,
        OverflowMode::Clip | OverflowMode::Hidden | OverflowMode::Auto | OverflowMode::Scroll
    ) || matches!(
        resolved.overflow_y,
        OverflowMode::Clip | OverflowMode::Hidden | OverflowMode::Auto | OverflowMode::Scroll
    );
}

fn align_items(value: &str) -> Option<AlignItems> {
    Some(match value {
        "start" => AlignItems::START,
        "end" => AlignItems::END,
        "center" => AlignItems::CENTER,
        "stretch" => AlignItems::STRETCH,
        "baseline" => AlignItems::BASELINE,
        _ => return None,
    })
}

fn align_content(value: &str) -> Option<AlignContent> {
    Some(match value {
        "start" => AlignContent::FLEX_START,
        "end" => AlignContent::FLEX_END,
        "center" => AlignContent::CENTER,
        "between" => AlignContent::SPACE_BETWEEN,
        "around" => AlignContent::SPACE_AROUND,
        "evenly" => AlignContent::SPACE_EVENLY,
        "stretch" => AlignContent::STRETCH,
        _ => return None,
    })
}

fn font_weight(value: &str) -> Option<u16> {
    Some(match value {
        "thin" => 100,
        "extralight" => 200,
        "light" => 300,
        "normal" => 400,
        "medium" => 500,
        "semibold" => 600,
        "bold" => 700,
        "extrabold" => 800,
        "black" => 900,
        _ => return None,
    })
}

fn font_size(value: &str) -> Option<(f32, f32)> {
    Some(match value {
        "xs" => (12.0, 16.0),
        "sm" => (14.0, 20.0),
        "base" => (16.0, 24.0),
        "lg" => (18.0, 28.0),
        "xl" => (20.0, 28.0),
        "2xl" => (24.0, 32.0),
        "3xl" => (30.0, 36.0),
        "4xl" => (36.0, 40.0),
        "5xl" => (48.0, 48.0),
        "6xl" => (60.0, 60.0),
        "7xl" => (72.0, 72.0),
        "8xl" => (96.0, 96.0),
        "9xl" => (128.0, 128.0),
        _ => return None,
    })
}

fn line_height_px(value: &str, font_size: f32) -> Option<f32> {
    let named = match value {
        "none" => Some(1.0),
        "tight" => Some(1.25),
        "snug" => Some(1.375),
        "normal" => Some(1.5),
        "relaxed" => Some(1.625),
        "loose" => Some(2.0),
        _ => None,
    };
    if let Some(mult) = named {
        return Some(font_size * mult);
    }
    spacing(value).or_else(|| color::arbitrary(value).and_then(parse_px))
}

fn radius(value: &str) -> Option<f32> {
    if let Some(px) = color::arbitrary(value).and_then(parse_px) {
        return Some(px);
    }
    Some(match value {
        "none" => 0.0,
        "sm" => 2.0,
        "md" => 6.0,
        "lg" => 8.0,
        "xl" => 12.0,
        "2xl" => 16.0,
        "3xl" => 24.0,
        "full" => 9999.0,
        _ => return None,
    })
}

fn max_width_name(value: &str) -> Option<f32> {
    Some(match value {
        "xs" => 320.0,
        "sm" => 384.0,
        "md" => 448.0,
        "lg" => 512.0,
        "xl" => 576.0,
        "2xl" => 672.0,
        "3xl" => 768.0,
        "4xl" => 896.0,
        "5xl" => 1024.0,
        "6xl" => 1152.0,
        "7xl" => 1280.0,
        _ => return None,
    })
}

fn parse_len(value: &str) -> Option<Len> {
    let (vw, vh) = RESOLVE_VIEW.with(|view| view.get());
    parse_len_with(value, vw, vh)
}

fn grid_tracks(value: &str, ctx: &ResolveCtx) -> Option<Vec<GridTemplateComponent<String>>> {
    color::arbitrary(value)?.split('_').map(|track| {
        if let Some(fraction) = track.strip_suffix("fr") {
            return Some(fr(fraction.parse::<f32>().ok()?));
        }
        if track == "auto" {
            return Some(auto());
        }
        match parse_arb_len(track, ctx.viewport_w, ctx.viewport_h)? {
            Len::Px(px) => Some(length(px)),
            Len::Pct(value) => Some(percent(value)),
            _ => None,
        }
    }).collect()
}

fn parse_len_with(value: &str, vw: f32, vh: f32) -> Option<Len> {
    match value {
        "auto" => return Some(Len::Auto),
        "full" => return Some(Len::Pct(1.0)),
        "screen" => return Some(Len::Viewport),
        "min" => return Some(Len::MinContent),
        "max" | "fit" => return Some(Len::MaxContent),
        "none" => return Some(Len::Px(1.0e6)),
        _ => {}
    }
    if let Some(px) = spacing(value) {
        return Some(Len::Px(px));
    }
    if let Some((num, den)) = value.split_once('/') {
        let num: f32 = num.parse().ok()?;
        let den: f32 = den.parse().ok()?;
        if den == 0.0 {
            return None;
        }
        return Some(Len::Pct(num / den));
    }
    if let Some(inner) = color::arbitrary(value) {
        return parse_arb_len(inner, vw, vh);
    }
    None
}

fn parse_arb_len(value: &str, vw: f32, vh: f32) -> Option<Len> {
    let value = value.trim();
    if let Some(inner) = value
        .strip_prefix("min(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let (left, right) = inner.split_once(',')?;
        let left = len_px(parse_arb_len(left.trim(), vw, vh)?, vw)?;
        let right = len_px(parse_arb_len(right.trim(), vw, vh)?, vw)?;
        return Some(Len::Px(left.min(right)));
    }
    if let Some(px) = parse_px(value) {
        return Some(Len::Px(px));
    }
    if let Some(num) = value.strip_suffix("rem") {
        return Some(Len::Px(num.trim().parse::<f32>().ok()? * 16.0));
    }
    if let Some(num) = value.strip_suffix("vw") {
        return Some(Len::Px(num.trim().parse::<f32>().ok()? / 100.0 * vw));
    }
    if let Some(num) = value.strip_suffix("vh") {
        return Some(Len::Px(num.trim().parse::<f32>().ok()? / 100.0 * vh));
    }
    if let Some(num) = value.strip_suffix('%') {
        return Some(Len::Pct(num.trim().parse::<f32>().ok()? / 100.0));
    }
    None
}

fn len_px(len: Len, viewport: f32) -> Option<f32> {
    Some(match len {
        Len::Px(value) => value,
        Len::Pct(value) => value * viewport,
        Len::Viewport => viewport,
        Len::Auto | Len::MinContent | Len::MaxContent => return None,
    })
}

fn parse_px(value: &str) -> Option<f32> {
    value.strip_suffix("px")?.parse().ok()
}

fn spacing(value: &str) -> Option<f32> {
    Some(match value {
        "0" => 0.0,
        "px" => 1.0,
        "0.5" => 2.0,
        "1" => 4.0,
        "1.5" => 6.0,
        "2" => 8.0,
        "2.5" => 10.0,
        "3" => 12.0,
        "3.5" => 14.0,
        "4" => 16.0,
        "5" => 20.0,
        "6" => 24.0,
        "7" => 28.0,
        "8" => 32.0,
        "9" => 36.0,
        "10" => 40.0,
        "11" => 44.0,
        "12" => 48.0,
        "14" => 56.0,
        "16" => 64.0,
        "20" => 80.0,
        "24" => 96.0,
        "28" => 112.0,
        "32" => 128.0,
        "36" => 144.0,
        "40" => 160.0,
        "44" => 176.0,
        "48" => 192.0,
        "52" => 208.0,
        "56" => 224.0,
        "60" => 240.0,
        "64" => 256.0,
        "72" => 288.0,
        "80" => 320.0,
        "96" => 384.0,
        _ => return None,
    })
}

fn negate(len: Len) -> Len {
    match len {
        Len::Px(value) => Len::Px(-value),
        Len::Pct(value) => Len::Pct(-value),
        other => other,
    }
}

fn dim(len: Len, viewport: f32) -> Dimension {
    match len {
        Len::Px(value) => length(value),
        Len::Pct(value) => percent(value),
        Len::Auto => auto(),
        Len::Viewport => length(viewport),
        Len::MinContent => Dimension::min_content(),
        Len::MaxContent => Dimension::max_content(),
    }
}

fn lp(len: Len, viewport: f32) -> LengthPercentage {
    match len {
        Len::Px(value) => length(value),
        Len::Pct(value) => percent(value),
        Len::Viewport => length(viewport),
        Len::Auto | Len::MinContent | Len::MaxContent => length(0.0),
    }
}

fn lpa(len: Len, viewport: f32) -> LengthPercentageAuto {
    match len {
        Len::Px(value) => length(value),
        Len::Pct(value) => percent(value),
        Len::Auto | Len::MinContent | Len::MaxContent => auto(),
        Len::Viewport => length(viewport),
    }
}

fn lp_all(
    len: Option<Len>,
    ctx: &ResolveCtx,
    _negative: bool,
) -> Option<(LengthPercentage, LengthPercentage)> {
    let len = len?;
    Some((lp(len, ctx.viewport_h), lp(len, ctx.viewport_w)))
}

fn lpa_all(
    len: Option<Len>,
    ctx: &ResolveCtx,
    negative: bool,
) -> Option<(LengthPercentageAuto, LengthPercentageAuto)> {
    let mut len = len?;
    if negative {
        len = negate(len);
    }
    Some((lpa(len, ctx.viewport_h), lpa(len, ctx.viewport_w)))
}

fn set_rect_lp(
    rect: &mut Rect<LengthPercentage>,
    value: Option<(LengthPercentage, LengthPercentage)>,
) {
    if let Some((vertical, horizontal)) = value {
        rect.top = vertical;
        rect.bottom = vertical;
        rect.left = horizontal;
        rect.right = horizontal;
    }
}

fn set_rect_lpa(
    rect: &mut Rect<LengthPercentageAuto>,
    value: Option<(LengthPercentageAuto, LengthPercentageAuto)>,
) {
    if let Some((vertical, horizontal)) = value {
        rect.top = vertical;
        rect.bottom = vertical;
        rect.left = horizontal;
        rect.right = horizontal;
    }
}

fn set_x_lp(rect: &mut Rect<LengthPercentage>, value: Option<LengthPercentage>) {
    if let Some(value) = value {
        rect.left = value;
        rect.right = value;
    }
}

fn set_y_lp(rect: &mut Rect<LengthPercentage>, value: Option<LengthPercentage>) {
    if let Some(value) = value {
        rect.top = value;
        rect.bottom = value;
    }
}

fn set_x_lpa(rect: &mut Rect<LengthPercentageAuto>, value: Option<LengthPercentageAuto>) {
    if let Some(value) = value {
        rect.left = value;
        rect.right = value;
    }
}

fn set_y_lpa(rect: &mut Rect<LengthPercentageAuto>, value: Option<LengthPercentageAuto>) {
    if let Some(value) = value {
        rect.top = value;
        rect.bottom = value;
    }
}

fn assign_lp(slot: &mut LengthPercentage, value: &str, viewport: f32) {
    if let Some(len) = parse_len(value) {
        *slot = lp(len, viewport);
    }
}

fn signed_lpa(value: &str, viewport: f32, negative: bool) -> Option<LengthPercentageAuto> {
    let mut len = parse_len(value)?;
    if negative {
        len = negate(len);
    }
    Some(lpa(len, viewport))
}

fn assign_signed_lpa(slot: &mut LengthPercentageAuto, value: &str, viewport: f32, negative: bool) {
    if let Some(len) = signed_lpa(value, viewport, negative) {
        *slot = len;
    }
}

fn gradient(resolved: &mut Resolved) -> &mut LinearGrad {
    resolved.gradient.get_or_insert(LinearGrad {
        dir: GradDir::Right,
        from: Rgba::TRANSPARENT,
        via: None,
        to: Rgba::TRANSPARENT,
    })
}

fn grad_dir(value: &str) -> Option<GradDir> {
    Some(match value {
        "r" => GradDir::Right,
        "l" => GradDir::Left,
        "t" => GradDir::Up,
        "b" => GradDir::Down,
        "tr" => GradDir::UpRight,
        "tl" => GradDir::UpLeft,
        "br" => GradDir::DownRight,
        "bl" => GradDir::DownLeft,
        _ => return None,
    })
}

fn blur_radius(value: &str) -> Option<f32> {
    Some(match value {
        "none" => 0.0,
        "sm" => 4.0,
        "md" => 12.0,
        "lg" => 16.0,
        "xl" => 24.0,
        "2xl" => 40.0,
        "3xl" => 64.0,
        _ => return None,
    })
}

fn ring_width(value: &str) -> Option<f32> {
    Some(match value {
        "0" => 0.0,
        "1" => 1.0,
        "2" => 2.0,
        "4" => 4.0,
        "8" => 8.0,
        _ => value.parse().ok()?,
    })
}

fn parse_shift(value: &str, negative: bool) -> Shift {
    let sign = if negative { -1.0 } else { 1.0 };
    match parse_len(value) {
        Some(Len::Px(px)) => Shift {
            px: sign * px,
            pct: 0.0,
        },
        Some(Len::Pct(pct)) => Shift {
            px: 0.0,
            pct: sign * pct,
        },
        _ => Shift::ZERO,
    }
}

fn apply_arbitrary(resolved: &mut Resolved, property: &str, value: &str) {
    match property {
        "transform" => apply_transform_list(&mut resolved.xform, value),
        "transform-style" if value.trim() == "preserve-3d" => resolved.preserve_3d = true,
        _ => {}
    }
}

fn apply_transform_list(xform: &mut Xform, value: &str) {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_alphabetic() {
            index += 1;
        }
        if start == index || index >= bytes.len() || bytes[index] != b'(' {
            break;
        }
        let name = &value[start..index];
        index += 1;
        let arg_start = index;
        let mut depth = 1;
        while index < bytes.len() && depth > 0 {
            match bytes[index] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            if depth > 0 {
                index += 1;
            }
        }
        let args = value[arg_start..index].trim();
        if index < bytes.len() && bytes[index] == b')' {
            index += 1;
        }
        apply_transform_fn(xform, name, args);
    }
}

fn apply_transform_fn(xform: &mut Xform, name: &str, args: &str) {
    let parts: Vec<&str> = args
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    let Some(first) = parts.first().copied() else {
        return;
    };
    match name {
        "translateX" => xform.tx = measure_shift(first),
        "translateY" => xform.ty = measure_shift(first),
        "translateZ" => xform.tz = measure_px(first),
        "rotate" | "rotateZ" => xform.rotate_z = measure_deg(first),
        "rotateY" => xform.rotate_y = measure_deg(first),
        "skewX" => xform.skew_x = measure_deg(first),
        "scale" => {
            let x = first.parse::<f32>().unwrap_or(1.0);
            let y = parts.get(1).and_then(|part| part.parse().ok()).unwrap_or(x);
            xform.scale_x = x;
            xform.scale_y = y;
        }
        "scaleX" => xform.scale_x = first.parse().unwrap_or(1.0),
        "scaleY" => xform.scale_y = first.parse().unwrap_or(1.0),
        _ => {}
    }
}

fn measure_shift(token: &str) -> Shift {
    if let Some(num) = token.strip_suffix('%') {
        return Shift {
            px: 0.0,
            pct: num.trim().parse::<f32>().unwrap_or(0.0) / 100.0,
        };
    }
    Shift {
        px: measure_px(token),
        pct: 0.0,
    }
}

fn measure_px(token: &str) -> f32 {
    token
        .trim()
        .trim_end_matches("px")
        .trim()
        .parse()
        .unwrap_or(0.0)
}

fn measure_deg(token: &str) -> f32 {
    token
        .trim()
        .trim_end_matches("deg")
        .trim()
        .parse()
        .unwrap_or(0.0)
}

/// Class tokens that change the matrix, opacity, or stacking, not the layer's pixels.
pub(crate) fn is_pose_token(token: &str) -> bool {
    if token.starts_with("[transform:") || token.starts_with("[transform-style:") {
        return true;
    }
    let utility = match token.rsplit_once(':') {
        Some((_, utility)) if !token.starts_with('[') => utility,
        _ => token,
    };
    utility.starts_with("opacity-")
        || utility.starts_with("z-")
        || utility == "z-auto"
        || utility.starts_with("duration-")
        || utility.starts_with("ease-")
        || utility == "transition"
        || utility == "transition-all"
        || utility.starts_with("origin-")
        || utility.starts_with("animate-")
}

pub(crate) fn pose_stripped_class(class_name: &str) -> String {
    class_name
        .split_whitespace()
        .filter(|token| !is_pose_token(token))
        .collect::<Vec<_>>()
        .join(" ")
}
