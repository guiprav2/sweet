//! 3D transforms, layer cache, and perspective blits.
//!
//! A transformed element whose descendants are still is rasterized once. Later
//! frames only multiply the matrix and composite that pixmap. An ancestor does
//! not bake a descendant that has its own motion, or the child freezes.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

use tiny_skia::{Pixmap, PremultipliedColorU8};

use crate::tailwind::{Ease, Motion, Resolved, Xform};

const TAU: f32 = std::f32::consts::TAU;

#[derive(Clone, Copy, Debug)]
pub struct Mat4 {
    /// Column-major.
    pub m: [f32; 16],
}

impl Mat4 {
    pub fn identity() -> Self {
        let mut m = [0.0; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        Self { m }
    }

    pub fn translate(x: f32, y: f32, z: f32) -> Self {
        let mut m = Self::identity();
        m.m[12] = x;
        m.m[13] = y;
        m.m[14] = z;
        m
    }

    pub fn scale(x: f32, y: f32) -> Self {
        let mut m = Self::identity();
        m.m[0] = x;
        m.m[5] = y;
        m
    }

    /// Clockwise on a screen whose Y axis points down.
    pub fn rotate_z(cos: f32, sin: f32) -> Self {
        let mut m = Self::identity();
        m.m[0] = cos;
        m.m[1] = sin;
        m.m[4] = -sin;
        m.m[5] = cos;
        m
    }

    /// CSS `rotateY`. Positive Z points toward the viewer.
    pub fn rotate_y(cos: f32, sin: f32) -> Self {
        let mut m = Self::identity();
        m.m[0] = cos;
        m.m[2] = -sin;
        m.m[8] = sin;
        m.m[10] = cos;
        m
    }

    pub fn skew_x(tan: f32) -> Self {
        let mut m = Self::identity();
        m.m[4] = tan;
        m
    }

    pub fn mul(self, rhs: Self) -> Self {
        let mut out = [0.0; 16];
        for col in 0..4 {
            for row in 0..4 {
                out[col * 4 + row] = self.m[row] * rhs.m[col * 4]
                    + self.m[4 + row] * rhs.m[col * 4 + 1]
                    + self.m[8 + row] * rhs.m[col * 4 + 2]
                    + self.m[12 + row] * rhs.m[col * 4 + 3];
            }
        }
        Self { m: out }
    }

    pub fn transform_point(self, x: f32, y: f32, z: f32) -> (f32, f32, f32) {
        (
            self.m[0] * x + self.m[4] * y + self.m[8] * z + self.m[12],
            self.m[1] * x + self.m[5] * y + self.m[9] * z + self.m[13],
            self.m[2] * x + self.m[6] * y + self.m[10] * z + self.m[14],
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub tx_px: f32,
    pub tx_pct: f32,
    pub ty_px: f32,
    pub ty_pct: f32,
    pub tz: f32,
    pub rotate_y: f32,
    pub rotate_z: f32,
    pub skew_x: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub origin_x: f32,
    pub origin_y: f32,
    pub opacity: f32,
}

impl Sample {
    fn from_xform(xform: &Xform, opacity: f32) -> Self {
        Self {
            tx_px: xform.tx.px,
            tx_pct: xform.tx.pct,
            ty_px: xform.ty.px,
            ty_pct: xform.ty.pct,
            tz: xform.tz,
            rotate_y: xform.rotate_y,
            rotate_z: xform.rotate_z,
            skew_x: xform.skew_x,
            scale_x: xform.scale_x,
            scale_y: xform.scale_y,
            origin_x: xform.origin_x,
            origin_y: xform.origin_y,
            opacity,
        }
    }

    /// True when the matrix is not the identity. Opacity is separate.
    pub fn moved(&self) -> bool {
        self.tx_px.abs() > 0.01
            || self.tx_pct.abs() > 0.0001
            || self.ty_px.abs() > 0.01
            || self.ty_pct.abs() > 0.0001
            || self.tz.abs() > 0.01
            || self.rotate_y.abs() > 0.01
            || self.rotate_z.abs() > 0.01
            || self.skew_x.abs() > 0.01
            || (self.scale_x - 1.0).abs() > 0.001
            || (self.scale_y - 1.0).abs() > 0.001
    }

    fn near(self, other: Self) -> bool {
        (self.tx_px - other.tx_px).abs() < 0.05
            && (self.tx_pct - other.tx_pct).abs() < 0.001
            && (self.ty_px - other.ty_px).abs() < 0.05
            && (self.ty_pct - other.ty_pct).abs() < 0.001
            && (self.tz - other.tz).abs() < 0.05
            && (self.rotate_y - other.rotate_y).abs() < 0.05
            && (self.rotate_z - other.rotate_z).abs() < 0.05
            && (self.skew_x - other.skew_x).abs() < 0.05
            && (self.scale_x - other.scale_x).abs() < 0.001
            && (self.scale_y - other.scale_y).abs() < 0.001
            && (self.opacity - other.opacity).abs() < 0.002
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_sample(a: Sample, b: Sample, t: f32) -> Sample {
    Sample {
        tx_px: lerp(a.tx_px, b.tx_px, t),
        tx_pct: lerp(a.tx_pct, b.tx_pct, t),
        ty_px: lerp(a.ty_px, b.ty_px, t),
        ty_pct: lerp(a.ty_pct, b.ty_pct, t),
        tz: lerp(a.tz, b.tz, t),
        rotate_y: lerp(a.rotate_y, b.rotate_y, t),
        rotate_z: lerp(a.rotate_z, b.rotate_z, t),
        skew_x: lerp(a.skew_x, b.skew_x, t),
        scale_x: lerp(a.scale_x, b.scale_x, t),
        scale_y: lerp(a.scale_y, b.scale_y, t),
        origin_x: lerp(a.origin_x, b.origin_x, t),
        origin_y: lerp(a.origin_y, b.origin_y, t),
        opacity: lerp(a.opacity, b.opacity, t),
    }
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn bezier(s: f32, c1: f32, c2: f32) -> f32 {
    let u = 1.0 - s;
    3.0 * u * u * s * c1 + 3.0 * u * s * s * c2 + s * s * s
}

fn bezier_deriv(s: f32, c1: f32, c2: f32) -> f32 {
    let u = 1.0 - s;
    3.0 * u * u * c1 + 6.0 * u * s * (c2 - c1) + 3.0 * s * s * (1.0 - c2)
}

fn solve_bezier_x(x: f32, x1: f32, x2: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let mut s = x;
    for _ in 0..8 {
        let delta = bezier(s, x1, x2) - x;
        let deriv = bezier_deriv(s, x1, x2);
        if deriv.abs() < 1.0e-5 {
            break;
        }
        s = (s - delta / deriv).clamp(0.0, 1.0);
    }
    s
}

/// Tailwind `ease-out`: `cubic-bezier(0, 0, 0.2, 1)`.
fn ease_out(t: f32) -> f32 {
    let s = solve_bezier_x(t, 0.0, 0.2);
    bezier(s, 0.0, 1.0)
}

fn ease_in_out(t: f32) -> f32 {
    smoothstep(t)
}

fn apply_ease(t: f32, ease: Ease) -> f32 {
    match ease {
        Ease::Linear => t,
        Ease::Out => ease_out(t),
        Ease::InOut => ease_in_out(t),
    }
}

struct Transition {
    from: Sample,
    to: Sample,
    start: f32,
    dur: f32,
    ease: Ease,
}

fn eval_transition(tr: &Transition, time: f32) -> Sample {
    if tr.dur <= 0.0 {
        return tr.to;
    }
    let u = ((time - tr.start) / tr.dur).clamp(0.0, 1.0);
    lerp_sample(tr.from, tr.to, apply_ease(u, tr.ease))
}

thread_local! {
    static TRANSITIONS: RefCell<HashMap<(u64, u32), Transition>> = RefCell::new(HashMap::new());
}

fn track_transition(
    key: (u64, u32),
    target: Sample,
    time: f32,
    dur: f32,
    ease: Ease,
    animating: &mut bool,
) -> Sample {
    TRANSITIONS.with(|slot| {
        let mut map = slot.borrow_mut();
        if let Some(tr) = map.get(&key) {
            if tr.to.near(target) && (tr.ease == ease) && (tr.dur - dur).abs() < 0.0001 {
                let sample = eval_transition(tr, time);
                let u = if tr.dur <= 0.0 {
                    1.0
                } else {
                    ((time - tr.start) / tr.dur).clamp(0.0, 1.0)
                };
                if u < 1.0 && !tr.from.near(tr.to) {
                    *animating = true;
                }
                return sample;
            }
            let current = eval_transition(tr, time);
            let started = Transition {
                from: current,
                to: target,
                start: time,
                dur,
                ease,
            };
            let sample = eval_transition(&started, time);
            if !current.near(target) {
                *animating = true;
            }
            map.insert(key, started);
            return sample;
        }
        map.insert(
            key,
            Transition {
                from: target,
                to: target,
                start: time,
                dur,
                ease,
            },
        );
        target
    })
}

fn apply_motion(sample: &mut Sample, motion: Motion, time: f32, animating: &mut bool) {
    let seconds = match motion {
        Motion::None => return,
        Motion::Spin { seconds }
        | Motion::Float { seconds }
        | Motion::Glow { seconds }
        | Motion::Sheen { seconds } => seconds,
    };
    *animating = true;
    let period = seconds.max(0.001);
    match motion {
        Motion::Spin { .. } => {
            sample.rotate_z += (time.rem_euclid(period) / period) * 360.0;
        }
        Motion::Float { .. } => {
            let wave = 0.5 * (1.0 - (TAU * time / period).cos());
            sample.ty_px += -12.0 * wave;
        }
        Motion::Glow { .. } => {
            let wave = 0.5 * (1.0 - (TAU * time / period).cos());
            let scale = 0.92 + 0.16 * wave;
            sample.scale_x *= scale;
            sample.scale_y *= scale;
            sample.opacity = 0.4 + 0.55 * wave;
        }
        Motion::Sheen { .. } => {
            let p = time.rem_euclid(period) / period;
            let tx = if p >= 0.42 {
                2.4
            } else {
                -1.4 + 3.8 * smoothstep(p / 0.42)
            };
            sample.tx_pct += tx;
            sample.skew_x += -18.0;
        }
        Motion::None => {}
    }
}

/// Static transform, plus a running transition, plus a looping motion.
pub fn visual_sample(style: &Resolved, time: f32, key: (u64, u32), animating: &mut bool) -> Sample {
    let mut sample = Sample::from_xform(&style.xform, style.opacity);
    if style.transition {
        let dur = if style.transition_ms > 0.0 {
            style.transition_ms
        } else {
            150.0
        } / 1000.0;
        sample = track_transition(key, sample, time, dur, style.ease, animating);
    }
    apply_motion(&mut sample, style.motion, time, animating);
    sample
}

/// CSS function order: scale, skew, rotateZ, rotateY, then translation.
/// The origin is the border-box point the functions pivot around.
pub fn local_matrix(sample: &Sample, w: f32, h: f32) -> Mat4 {
    let ox = w * sample.origin_x;
    let oy = h * sample.origin_y;
    let tx = sample.tx_px + sample.tx_pct * w;
    let ty = sample.ty_px + sample.ty_pct * h;
    let rz = sample.rotate_z.to_radians();
    let ry = sample.rotate_y.to_radians();
    Mat4::translate(ox + tx, oy + ty, sample.tz)
        .mul(Mat4::rotate_y(ry.cos(), ry.sin()))
        .mul(Mat4::rotate_z(rz.cos(), rz.sin()))
        .mul(Mat4::skew_x(sample.skew_x.to_radians().tan()))
        .mul(Mat4::scale(sample.scale_x, sample.scale_y))
        .mul(Mat4::translate(-ox, -oy, 0.0))
}

#[derive(Clone, Copy, Debug)]
pub struct Persp {
    pub cx: f32,
    pub cy: f32,
    pub d: f32,
}

pub fn project(world: &Mat4, x: f32, y: f32, persp: Option<Persp>) -> (f32, f32) {
    let (x, y, z) = world.transform_point(x, y, 0.0);
    let Some(persp) = persp else {
        return (x, y);
    };
    let denom = persp.d - z;
    if denom.abs() < 0.5 {
        return (x, y);
    }
    let scale = persp.d / denom;
    (
        persp.cx + (x - persp.cx) * scale,
        persp.cy + (y - persp.cy) * scale,
    )
}

pub struct BakedLayer {
    stamp: u64,
    width: u32,
    height: u32,
    scale_bits: u32,
    pub min_x: f32,
    pub min_y: f32,
    pub logical_w: f32,
    pub logical_h: f32,
    pub pixmap: Pixmap,
}

thread_local! {
    static LAYERS: RefCell<HashMap<(u64, u32), BakedLayer>> = RefCell::new(HashMap::new());
}

fn layer_matches(layer: &BakedLayer, stamp: u64, width: u32, height: u32, scale: f32) -> bool {
    layer.stamp == stamp
        && layer.width == width
        && layer.height == height
        && layer.scale_bits == scale.to_bits()
}

/// Rasterize on a miss, then hand the cached pixmap to `draw`.
/// Returns whether this call rasterized.
pub fn with_layer(
    key: (u64, u32),
    stamp: u64,
    width: u32,
    height: u32,
    scale: f32,
    rasterize: impl FnOnce() -> Option<BakedLayer>,
    draw: impl FnOnce(&BakedLayer),
) -> bool {
    let hit = LAYERS.with(|slot| {
        slot.borrow()
            .get(&key)
            .is_some_and(|layer| layer_matches(layer, stamp, width, height, scale))
    });
    if !hit {
        let Some(mut baked) = rasterize() else {
            return false;
        };
        baked.stamp = stamp;
        baked.width = width;
        baked.height = height;
        baked.scale_bits = scale.to_bits();
        LAYERS.with(|slot| {
            let mut map = slot.borrow_mut();
            if map.len() > 64 {
                map.clear();
            }
            map.insert(key, baked);
        });
    }
    LAYERS.with(|slot| {
        let map = slot.borrow();
        if let Some(layer) = map.get(&key) {
            draw(layer);
        }
    });
    !hit
}

pub fn blank_layer(
    width: u32,
    height: u32,
    min_x: f32,
    min_y: f32,
    logical_w: f32,
    logical_h: f32,
) -> Option<BakedLayer> {
    let pixmap = Pixmap::new(width, height)?;
    Some(BakedLayer {
        stamp: 0,
        width,
        height,
        scale_bits: 0,
        min_x,
        min_y,
        logical_w,
        logical_h,
        pixmap,
    })
}

struct Homography {
    nx0: f32,
    nx1: f32,
    nx2: f32,
    ny0: f32,
    ny1: f32,
    ny2: f32,
    d0: f32,
    d1: f32,
    d2: f32,
}

fn homography(world: &Mat4, persp: Persp) -> Homography {
    let a = world.m[0];
    let b = world.m[4];
    let c = world.m[12];
    let d = world.m[1];
    let e = world.m[5];
    let f = world.m[13];
    let g = world.m[2];
    let h = world.m[6];
    let i = world.m[14];
    let dist = persp.d;
    Homography {
        nx0: dist * a - persp.cx * g,
        nx1: dist * b - persp.cx * h,
        nx2: dist * c - persp.cx * i,
        ny0: dist * d - persp.cy * g,
        ny1: dist * e - persp.cy * h,
        ny2: dist * f - persp.cy * i,
        d0: -g,
        d1: -h,
        d2: dist - i,
    }
}

#[inline(always)]
fn lerp8(a: u8, b: u8, t: u32) -> u8 {
    ((a as u32 * (256 - t) + b as u32 * t) >> 8) as u8
}

#[inline(always)]
fn lerp_px(a: [u8; 4], b: [u8; 4], t: u32) -> [u8; 4] {
    [
        lerp8(a[0], b[0], t),
        lerp8(a[1], b[1], t),
        lerp8(a[2], b[2], t),
        lerp8(a[3], b[3], t),
    ]
}

#[inline(always)]
unsafe fn sample_block(
    src: *const u8,
    stride: isize,
    x: isize,
    y: isize,
    tx: u32,
    ty: u32,
) -> [u8; 4] {
    let row = src.offset(y * stride + x * 4);
    let below = src.offset((y + 1) * stride + x * 4);
    let a = *row.cast::<[u8; 4]>();
    let b = *row.offset(4).cast::<[u8; 4]>();
    let c = *below.cast::<[u8; 4]>();
    let d = *below.offset(4).cast::<[u8; 4]>();
    lerp_px(lerp_px(a, b, tx), lerp_px(c, d, tx), ty)
}

#[inline(always)]
fn div255(x: u32) -> u32 {
    (x + 1 + (x >> 8)) >> 8
}

#[inline(always)]
fn over_into(dst: &mut tiny_skia::PremultipliedColorU8, src: [u8; 4], fade: u32) {
    let (sr, sg, sb, sa) = if fade >= 256 {
        (src[0] as u32, src[1] as u32, src[2] as u32, src[3] as u32)
    } else {
        (
            (src[0] as u32 * fade) >> 8,
            (src[1] as u32 * fade) >> 8,
            (src[2] as u32 * fade) >> 8,
            (src[3] as u32 * fade) >> 8,
        )
    };
    if sa == 0 {
        return;
    }
    if sa >= 255 {
        *dst = unsafe {
            std::mem::transmute::<[u8; 4], PremultipliedColorU8>([
                sr as u8, sg as u8, sb as u8, 255,
            ])
        };
        return;
    }
    let inv = 255 - sa;
    let red = sr + div255(dst.red() as u32 * inv);
    let green = sg + div255(dst.green() as u32 * inv);
    let blue = sb + div255(dst.blue() as u32 * inv);
    let alpha = sa + div255(dst.alpha() as u32 * inv);
    *dst = unsafe {
        std::mem::transmute::<[u8; 4], PremultipliedColorU8>([
            red.min(255) as u8,
            green.min(255) as u8,
            blue.min(255) as u8,
            alpha.min(255) as u8,
        ])
    };
}

pub(crate) fn over_sprite(dst: &mut PremultipliedColorU8, sample: [u8; 4]) {
    write_sample(dst, sample, 256);
}

#[inline(always)]
fn write_sample(dst: &mut PremultipliedColorU8, sample: [u8; 4], fade: u32) {
    if sample[3] == 0 || fade == 0 {
        return;
    }
    if fade >= 256 && sample[3] == 255 {
        *dst = unsafe { std::mem::transmute::<[u8; 4], PremultipliedColorU8>(sample) };
        return;
    }
    over_into(dst, sample, fade);
}

/// Fixed-point trunc toward zero, matching `f32 as i32` for the pixel index.
#[inline(always)]
fn nearest_i(u_fp: i32) -> i32 {
    if u_fp >= 0 {
        u_fp >> 16
    } else {
        -(((u_fp.wrapping_neg() as u32) >> 16) as i32)
    }
}

#[inline(always)]
fn load_px(src: *const u8, stride: isize, x: i32, y: i32) -> [u8; 4] {
    unsafe {
        *src.offset(y as isize * stride + x as isize * 4)
            .cast::<[u8; 4]>()
    }
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn blend4_sse(dst: *mut u8, src: &[u8; 16], fade: u32) {
    use std::arch::x86_64::*;
    let zero = _mm_setzero_si128();
    let packed = _mm_loadu_si128(src.as_ptr().cast());
    let faded = if fade >= 256 {
        packed
    } else {
        let factor = _mm_set1_epi16(fade as i16);
        let lo = _mm_mullo_epi16(_mm_unpacklo_epi8(packed, zero), factor);
        let hi = _mm_mullo_epi16(_mm_unpackhi_epi8(packed, zero), factor);
        _mm_packus_epi16(_mm_srli_epi16(lo, 8), _mm_srli_epi16(hi, 8))
    };
    let dest = _mm_loadu_si128(dst.cast());
    let s_lo = _mm_unpacklo_epi8(faded, zero);
    let s_hi = _mm_unpackhi_epi8(faded, zero);
    let d_lo = _mm_unpacklo_epi8(dest, zero);
    let d_hi = _mm_unpackhi_epi8(dest, zero);
    // Alpha of each pixel, broadcast across its four channels.
    let a_lo = _mm_shufflehi_epi16(_mm_shufflelo_epi16(s_lo, 0xFF), 0xFF);
    let a_hi = _mm_shufflehi_epi16(_mm_shufflelo_epi16(s_hi, 0xFF), 0xFF);
    let inv_lo = _mm_sub_epi16(_mm_set1_epi16(255), a_lo);
    let inv_hi = _mm_sub_epi16(_mm_set1_epi16(255), a_hi);
    let mul_lo = _mm_mullo_epi16(d_lo, inv_lo);
    let mul_hi = _mm_mullo_epi16(d_hi, inv_hi);
    let one = _mm_set1_epi16(1);
    let div = |prod: __m128i| {
        _mm_srli_epi16(
            _mm_add_epi16(prod, _mm_add_epi16(one, _mm_srli_epi16(prod, 8))),
            8,
        )
    };
    let out = _mm_packus_epi16(
        _mm_add_epi16(s_lo, div(mul_lo)),
        _mm_add_epi16(s_hi, div(mul_hi)),
    );
    _mm_storeu_si128(dst.cast(), out);
}

#[derive(Clone, Copy)]
pub struct BlitClip {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub radius: f32,
}

/// Inclusive-exclusive horizontal span of a rounded rectangle at logical `y`.
fn rounded_span(round: &BlitClip, y: f32) -> Option<(f32, f32)> {
    if y < round.y || y >= round.y + round.h {
        return None;
    }
    let radius = round.radius.min(round.w * 0.5).min(round.h * 0.5);
    if radius < 0.5 {
        return Some((round.x, round.x + round.w));
    }
    let top = round.y + radius;
    let bottom = round.y + round.h - radius;
    if y >= top && y <= bottom {
        return Some((round.x, round.x + round.w));
    }
    let dy = if y < top { top - y } else { y - bottom };
    let reach = (radius * radius - dy * dy).max(0.0).sqrt();
    Some((round.x + radius - reach, round.x + round.w - radius + reach))
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

#[derive(Clone, Copy)]
enum BlitPath {
    Nearest,
    Bilinear,
    Perspective,
}

#[derive(Clone, Copy)]
struct BlitProf {
    nearest_us: u128,
    nearest_px: u64,
    bilinear_us: u128,
    bilinear_px: u64,
    persp_us: u128,
    persp_px: u64,
}

thread_local! {
    static PROF_ON: bool = std::env::var_os("SWEET_PROF").is_some();
    static PROF: Cell<BlitProf> = const {
        Cell::new(BlitProf {
            nearest_us: 0,
            nearest_px: 0,
            bilinear_us: 0,
            bilinear_px: 0,
            persp_us: 0,
            persp_px: 0,
        })
    };
}

#[allow(dead_code)]
pub(crate) fn take_blit_prof() -> Option<(u128, u64, u128, u64, u128, u64)> {
    if !PROF_ON.with(|on| *on) {
        return None;
    }
    let prof = PROF.with(|cell| {
        cell.replace(BlitProf {
            nearest_us: 0,
            nearest_px: 0,
            bilinear_us: 0,
            bilinear_px: 0,
            persp_us: 0,
            persp_px: 0,
        })
    });
    Some((
        prof.nearest_us,
        prof.nearest_px,
        prof.bilinear_us,
        prof.bilinear_px,
        prof.persp_us,
        prof.persp_px,
    ))
}

fn note_blit(path: BlitPath, started: Option<Instant>, pixels: u64) {
    let Some(started) = started else {
        return;
    };
    let us = started.elapsed().as_micros();
    PROF.with(|cell| {
        let mut prof = cell.get();
        match path {
            BlitPath::Nearest => {
                prof.nearest_us += us;
                prof.nearest_px += pixels;
            }
            BlitPath::Bilinear => {
                prof.bilinear_us += us;
                prof.bilinear_px += pixels;
            }
            BlitPath::Perspective => {
                prof.persp_us += us;
                prof.persp_px += pixels;
            }
        }
        cell.set(prof);
    });
}

/// A pure translateZ is a uniform scale about the perspective center.
fn flatten_perspective(world: &Mat4, persp: Persp) -> Option<Mat4> {
    if world.m[2].abs() > 1.0e-4 || world.m[6].abs() > 1.0e-4 {
        return None;
    }
    let z = world.m[14];
    let denom = persp.d - z;
    if denom.abs() < 0.5 {
        return None;
    }
    let p = persp.d / denom;
    let mut flat = Mat4::identity();
    flat.m[0] = world.m[0] * p;
    flat.m[4] = world.m[4] * p;
    flat.m[12] = persp.cx + (world.m[12] - persp.cx) * p;
    flat.m[1] = world.m[1] * p;
    flat.m[5] = world.m[5] * p;
    flat.m[13] = persp.cy + (world.m[13] - persp.cy) * p;
    Some(flat)
}

fn clip_dest(
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    scale: f32,
    dest_w: i32,
    dest_h: i32,
    clip: &BlitClip,
    round: Option<&BlitClip>,
) -> (i32, i32, i32, i32) {
    let mut x0 = ((min_x * scale).floor() as i32).max(0);
    let mut y0 = ((min_y * scale).floor() as i32).max(0);
    let mut x1 = ((max_x * scale).ceil() as i32).min(dest_w);
    let mut y1 = ((max_y * scale).ceil() as i32).min(dest_h);
    let mut tighten = |region: &BlitClip| {
        if region.w <= 0.0 || region.h <= 0.0 {
            return;
        }
        x0 = x0.max((region.x * scale).floor() as i32);
        y0 = y0.max((region.y * scale).floor() as i32);
        x1 = x1.min(((region.x + region.w) * scale).ceil() as i32);
        y1 = y1.min(((region.y + region.h) * scale).ceil() as i32);
    };
    tighten(clip);
    if let Some(round) = round {
        tighten(round);
    }
    (x0, y0, x1, y1)
}

/// Composite a cached layer through `world` and an optional perspective.
pub fn blit(
    dest: &mut Pixmap,
    layer: &BakedLayer,
    world: &Mat4,
    persp: Option<Persp>,
    fade: f32,
    scale: f32,
    clip: &BlitClip,
    round: Option<&BlitClip>,
) {
    if fade <= 0.001
        || layer.pixmap.width() == 0
        || layer.logical_w <= 0.0
        || layer.logical_h <= 0.0
    {
        return;
    }
    let corners = [
        (layer.min_x, layer.min_y),
        (layer.min_x + layer.logical_w, layer.min_y),
        (layer.min_x + layer.logical_w, layer.min_y + layer.logical_h),
        (layer.min_x, layer.min_y + layer.logical_h),
    ];
    let projected = corners.map(|(x, y)| project(world, x, y, persp));
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for (x, y) in projected {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let scale = scale.max(0.25);
    let (x0, y0, x1, y1) = clip_dest(
        min_x,
        min_y,
        max_x,
        max_y,
        scale,
        dest.width() as i32,
        dest.height() as i32,
        clip,
        round,
    );
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    // A rounded window clip should not disable the SIMD compositing paths
    // for layers that never touch its corners. The rounded rectangle is
    // convex, so containment of all four bounds corners is sufficient.
    let round = round.filter(|round| {
        ![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
            .iter()
            .all(|&(x, y)| {
                inside_round(
                    x as f32 / scale - round.x,
                    y as f32 / scale - round.y,
                    round.w,
                    round.h,
                    round.radius,
                )
            })
    });
    if let Some(persp) = persp {
        if let Some(flat) = flatten_perspective(world, persp) {
            blit_affine(dest, layer, &flat, fade, scale, round, x0, y0, x1, y1);
        } else {
            blit_perspective(
                dest, layer, world, persp, fade, scale, round, x0, y0, x1, y1,
            );
        }
    } else {
        blit_affine(dest, layer, world, fade, scale, round, x0, y0, x1, y1);
    }
}

fn blit_affine(
    dest: &mut Pixmap,
    layer: &BakedLayer,
    world: &Mat4,
    fade: f32,
    scale: f32,
    round: Option<&BlitClip>,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
) {
    let a = world.m[0];
    let b = world.m[4];
    let c = world.m[12];
    let d = world.m[1];
    let e = world.m[5];
    let f = world.m[13];
    let det = a * e - b * d;
    if det.abs() < 1.0e-6 {
        return;
    }
    let inv = 1.0 / det;
    let inv00 = e * inv;
    let inv01 = -b * inv;
    let inv10 = -d * inv;
    let inv11 = a * inv;
    let fade_u = (fade.clamp(0.0, 1.0) * 256.0) as u32;
    if round.is_none() && inv01.abs() < 0.001 && inv10.abs() < 0.001 {
        blit_nearest(
            dest, layer, fade_u, scale, x0, y0, x1, y1, inv00, inv01, inv10, inv11, c, f,
        );
    } else {
        blit_bilinear(
            dest, layer, fade_u, scale, round, x0, y0, x1, y1, inv00, inv01, inv10, inv11, c, f,
        );
    }
}

fn blit_nearest(
    dest: &mut Pixmap,
    layer: &BakedLayer,
    fade_u: u32,
    scale: f32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    inv00: f32,
    inv01: f32,
    inv10: f32,
    inv11: f32,
    origin_x: f32,
    origin_y: f32,
) {
    let started = PROF_ON.with(|on| (*on).then(Instant::now));
    let inv_scale = 1.0 / scale;
    let src_w = layer.pixmap.width() as i32;
    let src_h = layer.pixmap.height() as i32;
    let src_ptr = layer.pixmap.data().as_ptr();
    let stride = (src_w * 4) as isize;
    let dest_w = dest.width() as u32;
    let pixels = dest.pixels_mut().as_mut_ptr();
    let du_fp = (inv00 * 65536.0) as i32;
    let mut ly = inv10 * ((x0 as f32 + 0.5) * inv_scale - origin_x)
        + inv11 * ((y0 as f32 + 0.5) * inv_scale - origin_y);
    let dly = inv11 * inv_scale;
    for py in y0..y1 {
        let v = (ly - layer.min_y) * scale - 0.5;
        let sy = v as i32;
        if (0..src_h).contains(&sy) {
            let screen_x = (x0 as f32 + 0.5) * inv_scale;
            let screen_y = (py as f32 + 0.5) * inv_scale;
            let lx = inv00 * (screen_x - origin_x) + inv01 * (screen_y - origin_y);
            let mut u_fp = ((lx - layer.min_x) * scale - 0.5).mul_add(65536.0, 0.0) as i32;
            let row = unsafe { pixels.add((py as u32 * dest_w) as usize) };
            let mut column = x0;
            while column + 4 <= x1 {
                unsafe {
                    blend4(
                        row.add(column as usize),
                        src_ptr,
                        stride,
                        sy,
                        src_w,
                        u_fp,
                        du_fp,
                        fade_u,
                    );
                }
                u_fp = u_fp.wrapping_add(du_fp.wrapping_mul(4));
                column += 4;
            }
            while column < x1 {
                let sx = nearest_i(u_fp);
                if (0..src_w).contains(&sx) {
                    let sample = load_px(src_ptr, stride, sx, sy);
                    write_sample(unsafe { &mut *row.add(column as usize) }, sample, fade_u);
                }
                u_fp = u_fp.wrapping_add(du_fp);
                column += 1;
            }
        }
        ly += dly;
    }
    note_blit(BlitPath::Nearest, started, ((x1 - x0) * (y1 - y0)) as u64);
}

#[inline(always)]
unsafe fn blend4(
    dst: *mut PremultipliedColorU8,
    src: *const u8,
    stride: isize,
    sy: i32,
    src_w: i32,
    u_fp: i32,
    du_fp: i32,
    fade: u32,
) {
    let mut bytes = [0u8; 16];
    let mut cursor = u_fp;
    for slot in 0..4 {
        let sx = nearest_i(cursor);
        if (0..src_w).contains(&sx) {
            let px = load_px(src, stride, sx, sy);
            bytes[slot * 4] = px[0];
            bytes[slot * 4 + 1] = px[1];
            bytes[slot * 4 + 2] = px[2];
            bytes[slot * 4 + 3] = px[3];
        }
        cursor = cursor.wrapping_add(du_fp);
    }
    if bytes[3] | bytes[7] | bytes[11] | bytes[15] == 0 {
        return;
    }
    #[cfg(target_arch = "x86_64")]
    blend4_sse(dst.cast(), &bytes, fade);
    #[cfg(not(target_arch = "x86_64"))]
    {
        for slot in 0..4 {
            let sample = [
                bytes[slot * 4],
                bytes[slot * 4 + 1],
                bytes[slot * 4 + 2],
                bytes[slot * 4 + 3],
            ];
            write_sample(&mut *dst.add(slot), sample, fade);
        }
    }
}

fn blit_bilinear(
    dest: &mut Pixmap,
    layer: &BakedLayer,
    fade_u: u32,
    scale: f32,
    round: Option<&BlitClip>,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    inv00: f32,
    inv01: f32,
    inv10: f32,
    inv11: f32,
    origin_x: f32,
    origin_y: f32,
) {
    let started = PROF_ON.with(|on| (*on).then(Instant::now));
    let inv_scale = 1.0 / scale;
    let src_w = layer.pixmap.width() as i32;
    let src_h = layer.pixmap.height() as i32;
    let src_ptr = layer.pixmap.data().as_ptr();
    let stride = (src_w * 4) as isize;
    let dest_w = dest.width() as u32;
    let pixels = dest.pixels_mut().as_mut_ptr();
    let dlx = inv00 * inv_scale;
    let dly = inv10 * inv_scale;
    for py in y0..y1 {
        let sy = (py as f32 + 0.5) * inv_scale;
        if let Some(round) = round {
            if sy < round.y || sy >= round.y + round.h {
                continue;
            }
        }
        let (mut col0, mut col1) = (x0, x1);
        if let Some(round) = round {
            let Some((left, right)) = rounded_span(round, sy) else {
                continue;
            };
            col0 = col0.max((left * scale - 0.5).ceil() as i32);
            col1 = col1.min((right * scale - 0.5).ceil() as i32);
            if col0 >= col1 {
                continue;
            }
        }
        let sx0 = (col0 as f32 + 0.5) * inv_scale;
        let mut lx = inv00 * (sx0 - origin_x) + inv01 * (sy - origin_y);
        let mut ly = inv10 * (sx0 - origin_x) + inv11 * (sy - origin_y);
        let row = unsafe { pixels.add((py as u32 * dest_w) as usize) };
        for column in col0..col1 {
            let u = (lx - layer.min_x) * scale - 0.5;
            let v = (ly - layer.min_y) * scale - 0.5;
            let sx = u as i32;
            let sy_src = v as i32;
            if sx >= 0 && sy_src >= 0 && sx + 1 < src_w && sy_src + 1 < src_h {
                let tx = ((u - sx as f32) * 256.0) as u32;
                let ty = ((v - sy_src as f32) * 256.0) as u32;
                let sample =
                    unsafe { sample_block(src_ptr, stride, sx as isize, sy_src as isize, tx, ty) };
                write_sample(unsafe { &mut *row.add(column as usize) }, sample, fade_u);
            }
            lx += dlx;
            ly += dly;
        }
    }
    note_blit(BlitPath::Bilinear, started, ((x1 - x0) * (y1 - y0)) as u64);
}

fn blit_perspective(
    dest: &mut Pixmap,
    layer: &BakedLayer,
    world: &Mat4,
    persp: Persp,
    fade: f32,
    scale: f32,
    round: Option<&BlitClip>,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
) {
    let started = PROF_ON.with(|on| (*on).then(Instant::now));
    let h = homography(world, persp);
    let inv_scale = 1.0 / scale;
    let src_w = layer.pixmap.width() as i32;
    let src_h = layer.pixmap.height() as i32;
    let src_ptr = layer.pixmap.data().as_ptr();
    let stride = (src_w * 4) as isize;
    let dest_w = dest.width() as u32;
    let fade_u = (fade.clamp(0.0, 1.0) * 256.0) as u32;
    let pixels = dest.pixels_mut().as_mut_ptr();
    let corners = [
        (layer.min_x, layer.min_y),
        (layer.min_x + layer.logical_w, layer.min_y),
        (layer.min_x + layer.logical_w, layer.min_y + layer.logical_h),
        (layer.min_x, layer.min_y + layer.logical_h),
    ]
    .map(|(x, y)| project(world, x, y, Some(persp)));
    for py in y0..y1 {
        let sy = (py as f32 + 0.5) * inv_scale;
        if let Some(round) = round {
            if sy < round.y || sy >= round.y + round.h {
                continue;
            }
        }
        let Some((left, right)) = quad_span_x(&corners, sy) else {
            continue;
        };
        let mut col0 = ((left * scale).floor() as i32).max(x0);
        let col1 = ((right * scale).ceil() as i32).min(x1);
        if let Some(round) = round {
            let screen_left = round.x * scale;
            let screen_right = (round.x + round.w) * scale;
            col0 = col0.max(screen_left.floor() as i32);
            let capped = (screen_right.ceil() as i32).min(col1);
            if col0 >= capped {
                continue;
            }
            paint_persp_span(
                pixels,
                dest_w,
                py,
                src_ptr,
                stride,
                src_w,
                src_h,
                layer,
                &h,
                fade_u,
                scale,
                inv_scale,
                sy,
                col0,
                capped,
                Some(round),
            );
        } else if col0 < col1 {
            paint_persp_span(
                pixels, dest_w, py, src_ptr, stride, src_w, src_h, layer, &h, fade_u, scale,
                inv_scale, sy, col0, col1, None,
            );
        }
    }
    note_blit(
        BlitPath::Perspective,
        started,
        ((x1 - x0) * (y1 - y0)) as u64,
    );
}

fn quad_span_x(corners: &[(f32, f32); 4], y: f32) -> Option<(f32, f32)> {
    let mut min_x = f32::MAX;
    let mut max_x = f32::MIN;
    for index in 0..4 {
        let (x0, y0) = corners[index];
        let (x1, y1) = corners[(index + 1) % 4];
        let (lo, hi) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
        if y < lo || y >= hi || (y1 - y0).abs() < 1.0e-6 {
            continue;
        }
        let t = (y - y0) / (y1 - y0);
        let x = x0 + (x1 - x0) * t;
        min_x = min_x.min(x);
        max_x = max_x.max(x);
    }
    (min_x < max_x).then_some((min_x, max_x))
}

fn paint_persp_span(
    pixels: *mut PremultipliedColorU8,
    dest_w: u32,
    py: i32,
    src: *const u8,
    stride: isize,
    src_w: i32,
    src_h: i32,
    layer: &BakedLayer,
    h: &Homography,
    fade_u: u32,
    scale: f32,
    inv_scale: f32,
    sy: f32,
    col0: i32,
    col1: i32,
    round: Option<&BlitClip>,
) {
    let a10 = h.ny0 - sy * h.d0;
    let a11 = h.ny1 - sy * h.d1;
    let rhs1 = sy * h.d2 - h.ny2;
    let ds = inv_scale;
    let ddet = ds * (h.d1 * a10 - h.d0 * a11);
    let dn = ds * (h.d2 * a11 + h.d1 * rhs1);
    let dm = ds * (-h.d0 * rhs1 - h.d2 * a10);
    let mut sx = (col0 as f32 + 0.5) * inv_scale;
    let a00 = h.nx0 - sx * h.d0;
    let a01 = h.nx1 - sx * h.d1;
    let rhs0 = sx * h.d2 - h.nx2;
    let mut det = a00 * a11 - a01 * a10;
    let mut n = rhs0 * a11 - a01 * rhs1;
    let mut m = a00 * rhs1 - rhs0 * a10;
    let row = unsafe { pixels.add((py as u32 * dest_w) as usize) };
    let mut column = col0;
    if round.is_none() {
        while column + 4 <= col1 {
            if (column - col0) & 15 == 0 {
                sx = (column as f32 + 0.5) * inv_scale;
                let a00 = h.nx0 - sx * h.d0;
                let a01 = h.nx1 - sx * h.d1;
                let rhs0 = sx * h.d2 - h.nx2;
                det = a00 * a11 - a01 * a10;
                n = rhs0 * a11 - a01 * rhs1;
                m = a00 * rhs1 - rhs0 * a10;
            }
            let mut ns = [0.0f32; 4];
            let mut ms = [0.0f32; 4];
            let mut dets = [0.0f32; 4];
            for slot in 0..4 {
                ns[slot] = n;
                ms[slot] = m;
                dets[slot] = det;
                n += dn;
                m += dm;
                det += ddet;
            }
            unsafe {
                persp4(
                    row.add(column as usize),
                    src,
                    stride,
                    src_w,
                    src_h,
                    &ns,
                    &ms,
                    &dets,
                    layer.min_x,
                    layer.min_y,
                    scale,
                    fade_u,
                );
            }
            column += 4;
        }
    }
    while column < col1 {
        let screen_x = (column as f32 + 0.5) * inv_scale;
        let inside = match round {
            Some(round) => inside_round(
                screen_x - round.x,
                sy - round.y,
                round.w,
                round.h,
                round.radius,
            ),
            None => true,
        };
        if inside && det.abs() > 1.0e-6 {
            let inv_det = 1.0 / det;
            let u = (n * inv_det - layer.min_x) * scale - 0.5;
            let v = (m * inv_det - layer.min_y) * scale - 0.5;
            let sx_i = u as i32;
            let sy_i = v as i32;
            if (sx_i as u32) < src_w as u32 && (sy_i as u32) < src_h as u32 {
                let sample = load_px(src, stride, sx_i, sy_i);
                write_sample(unsafe { &mut *row.add(column as usize) }, sample, fade_u);
            }
        }
        n += dn;
        m += dm;
        det += ddet;
        column += 1;
    }
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn persp4(
    dst: *mut PremultipliedColorU8,
    src: *const u8,
    stride: isize,
    src_w: i32,
    src_h: i32,
    n: &[f32; 4],
    m: &[f32; 4],
    det: &[f32; 4],
    min_x: f32,
    min_y: f32,
    scale: f32,
    fade: u32,
) {
    use std::arch::x86_64::*;
    let dv = _mm_loadu_ps(det.as_ptr());
    let rcp = _mm_rcp_ps(dv);
    let two = _mm_set1_ps(2.0);
    let inv = _mm_mul_ps(rcp, _mm_sub_ps(two, _mm_mul_ps(dv, rcp)));
    let lx = _mm_mul_ps(_mm_loadu_ps(n.as_ptr()), inv);
    let ly = _mm_mul_ps(_mm_loadu_ps(m.as_ptr()), inv);
    let step = _mm_set1_ps(scale);
    let half = _mm_set1_ps(0.5);
    let u = _mm_sub_ps(_mm_mul_ps(_mm_sub_ps(lx, _mm_set1_ps(min_x)), step), half);
    let v = _mm_sub_ps(_mm_mul_ps(_mm_sub_ps(ly, _mm_set1_ps(min_y)), step), half);
    let mut xs = [0i32; 4];
    let mut ys = [0i32; 4];
    _mm_storeu_si128(xs.as_mut_ptr().cast(), _mm_cvttps_epi32(u));
    _mm_storeu_si128(ys.as_mut_ptr().cast(), _mm_cvttps_epi32(v));
    let mut bytes = [0u8; 16];
    for slot in 0..4 {
        if det[slot].abs() <= 1.0e-4 {
            continue;
        }
        let sx = xs[slot];
        let sy = ys[slot];
        if (sx as u32) < src_w as u32 && (sy as u32) < src_h as u32 {
            let px = load_px(src, stride, sx, sy);
            bytes[slot * 4] = px[0];
            bytes[slot * 4 + 1] = px[1];
            bytes[slot * 4 + 2] = px[2];
            bytes[slot * 4 + 3] = px[3];
        }
    }
    if bytes[3] | bytes[7] | bytes[11] | bytes[15] != 0 {
        blend4_sse(dst.cast(), &bytes, fade);
    }
}

#[cfg(not(target_arch = "x86_64"))]
#[inline(always)]
unsafe fn persp4(
    dst: *mut PremultipliedColorU8,
    src: *const u8,
    stride: isize,
    src_w: i32,
    src_h: i32,
    n: &[f32; 4],
    m: &[f32; 4],
    det: &[f32; 4],
    min_x: f32,
    min_y: f32,
    scale: f32,
    fade: u32,
) {
    for slot in 0..4 {
        if det[slot].abs() <= 1.0e-4 {
            continue;
        }
        let inv_det = 1.0 / det[slot];
        let u = (n[slot] * inv_det - min_x) * scale - 0.5;
        let v = (m[slot] * inv_det - min_y) * scale - 0.5;
        let sx = u as i32;
        let sy = v as i32;
        if (sx as u32) < src_w as u32 && (sy as u32) < src_h as u32 {
            let sample = load_px(src, stride, sx, sy);
            write_sample(&mut *dst.add(slot), sample, fade);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tailwind::Shift;

    fn flat(tx: f32, ty: f32, tz: f32) -> Sample {
        Sample {
            tx_px: tx,
            tx_pct: 0.0,
            ty_px: ty,
            ty_pct: 0.0,
            tz,
            rotate_y: 0.0,
            rotate_z: 0.0,
            skew_x: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            origin_x: 0.5,
            origin_y: 0.5,
            opacity: 1.0,
        }
    }

    #[test]
    fn ease_out_starts_fast() {
        assert!(ease_out(0.0).abs() < 0.001);
        assert!((ease_out(1.0) - 1.0).abs() < 0.001);
        assert!(ease_out(0.5) > 0.8, "{}", ease_out(0.5));
    }

    #[test]
    fn rounded_clip_preserves_interior_layers_and_clips_corners() {
        let mut layer = blank_layer(16, 16, 0.0, 0.0, 16.0, 16.0).unwrap();
        layer.pixmap.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let clip = BlitClip { x: 0.0, y: 0.0, w: 64.0, h: 64.0, radius: 16.0 };
        let mut interior = Pixmap::new(64, 64).unwrap();
        let mut unclipped = Pixmap::new(64, 64).unwrap();
        let world = Mat4::translate(24.0, 24.0, 0.0);
        blit(&mut interior, &layer, &world, None, 1.0, 1.0, &clip, Some(&clip));
        blit(&mut unclipped, &layer, &world, None, 1.0, 1.0, &clip, None);
        assert_eq!(interior.data(), unclipped.data());
        let mut corner = Pixmap::new(64, 64).unwrap();
        blit(&mut corner, &layer, &Mat4::identity(), None, 1.0, 1.0, &clip, Some(&clip));
        assert_eq!(corner.pixel(0, 0).unwrap().alpha(), 0);
        assert_eq!(corner.pixel(12, 12).unwrap().alpha(), 255);
    }

    #[test]
    fn translation_ignores_the_origin() {
        let matrix = local_matrix(&flat(40.0, 0.0, 0.0), 30.0, 10.0);
        let (x, y, z) = matrix.transform_point(0.0, 0.0, 0.0);
        assert!((x - 40.0).abs() < 0.01, "{x}");
        assert!(y.abs() < 0.01, "{y}");
        assert!(z.abs() < 0.01, "{z}");
    }

    #[test]
    fn positive_z_scales_up_about_the_perspective_center() {
        let local = local_matrix(&flat(0.0, 0.0, 50.0), 100.0, 100.0);
        let world = Mat4::translate(100.0, 100.0, 0.0).mul(local);
        let persp = Persp {
            cx: 150.0,
            cy: 150.0,
            d: 200.0,
        };
        let (x, y) = project(&world, 0.0, 0.0, Some(persp));
        assert!((x - 83.333).abs() < 0.2, "{x}");
        assert!((y - 83.333).abs() < 0.2, "{y}");
        let (x, y) = project(&world, 50.0, 50.0, Some(persp));
        assert!((x - 150.0).abs() < 0.2, "{x}");
        assert!((y - 150.0).abs() < 0.2, "{y}");
    }

    #[test]
    fn percent_translation_uses_the_border_box() {
        let mut sample = flat(0.0, 0.0, 0.0);
        sample.tx_pct = -0.5;
        sample.ty_pct = -0.5;
        let matrix = local_matrix(&sample, 80.0, 40.0);
        let (x, y, _) = matrix.transform_point(0.0, 0.0, 0.0);
        assert!((x + 40.0).abs() < 0.01, "{x}");
        assert!((y + 20.0).abs() < 0.01, "{y}");
        let _ = Shift::ZERO;
    }
}
