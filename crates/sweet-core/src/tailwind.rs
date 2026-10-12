//! Tailwind utility resolver.
//!
//! Class strings map onto Taffy style fields and paint attributes. There is no
//! CSS parser and no stylesheet cascade: later utilities win, and `!` wins last.

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

#[derive(Clone, Debug)]
pub struct Resolved {
    pub style: Style,
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
    "pointer-events",
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
    "cursor",
    "opacity",
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

pub fn apply_classes(resolved: &mut Resolved, class_name: &str, ctx: &ResolveCtx) {
    let mut important = Vec::new();
    for token in class_name.split_whitespace() {
        let (is_important, utility, variants) = parse_token(token);
        if !variants_match(&variants, ctx) {
            continue;
        }
        if is_important {
            important.push(utility.to_string());
        } else {
            apply_utility(resolved, utility, ctx);
        }
    }
    for utility in important {
        apply_utility(resolved, &utility, ctx);
    }
}

fn empty_resolved() -> Resolved {
    Resolved {
        style: Style {
            display: Display::Block,
            box_sizing: BoxSizing::BorderBox,
            ..Style::default()
        },
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
    }
}

fn parse_token(token: &str) -> (bool, &str, Vec<&str>) {
    let mut parts: Vec<&str> = token.split(':').collect();
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
    let (negative, utility) = match raw.strip_prefix('-') {
        Some(rest) if !rest.is_empty() => (true, rest),
        _ => (false, raw),
    };
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
            resolved.clip = true;
            set_overflow_hidden(style);
        }
        "object-fill" => resolved.object_fit = ObjectFit::Fill,
        "object-contain" => resolved.object_fit = ObjectFit::Contain,
        "object-cover" => resolved.object_fit = ObjectFit::Cover,
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
            }
        }
        "grid-rows" => {
            if let Ok(count) = value.parse::<u16>() {
                resolved.style.grid_template_rows = (0..count).map(|_| fr(1.0)).collect();
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
            if let Some(color) = color::parse_color(value) {
                resolved.background = color;
            }
        }
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
            if value == "auto" {
                resolved.z = 0;
            } else if let Ok(z) = value.parse::<i32>() {
                resolved.z = if negative { -z } else { z };
            }
        }
        "overflow" => {
            if value == "hidden" {
                resolved.clip = true;
                set_overflow_hidden(&mut resolved.style);
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

fn set_overflow_hidden(style: &mut Style) {
    style.overflow = taffy::geometry::Point {
        x: Overflow::Hidden,
        y: Overflow::Hidden,
    };
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
        return parse_arb_len(inner);
    }
    None
}

fn parse_arb_len(value: &str) -> Option<Len> {
    if let Some(px) = parse_px(value) {
        return Some(Len::Px(px));
    }
    if let Some(num) = value.strip_suffix("rem") {
        return Some(Len::Px(num.parse::<f32>().ok()? * 16.0));
    }
    if let Some(num) = value.strip_suffix('%') {
        return Some(Len::Pct(num.parse::<f32>().ok()? / 100.0));
    }
    None
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
