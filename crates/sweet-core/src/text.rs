//! System font loading, measurement, and rasterization.
//!
//! Layout uses CSS pixels. Glyphs are rasterized at `font_size * scale` so a
//! HiDPI window stays sharp without a second layout pass.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use fontdue::Font;

/// Enough for a desktop UI's weights, sizes, and characters. A full cache
/// starts over rather than growing without a bound.
const GLYPH_CACHE_LIMIT: usize = 4096;

use crate::color::Rgba;
use crate::error::SweetError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug)]
pub struct Clip {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub struct TextLayout {
    pub width: f32,
    pub height: f32,
    pub line_box: f32,
    lines: Vec<Line>,
}

struct Line {
    start: usize,
    end: usize,
    width: f32,
}

struct CachedGlyph {
    metrics: fontdue::Metrics,
    bitmap: Vec<u8>,
}

pub struct FontBook {
    faces: Vec<(u16, Font)>,
    nerd: Font,
    /// Keyed by face, pixel size, and character. Rasterizing every glyph on
    /// every frame held the window thread inside pointer handlers.
    cache: Mutex<HashMap<(u16, u32, char), CachedGlyph>>,
}

impl FontBook {
    pub fn load() -> Result<Self, SweetError> {
        let mut faces = Vec::new();
        if let Ok(path) = std::env::var("SWEET_FONT") {
            if let Some(font) = load_font(Path::new(&path)) {
                faces.push((400, font));
            }
        }
        if faces.is_empty() {
            if let Ok(dir) = std::env::var("SWEET_FONT_DIR") {
                for (weight, name) in LATO_FILES {
                    if let Some(font) = load_font(Path::new(&dir).join(name)) {
                        faces.push((*weight, font));
                    }
                }
                if faces.is_empty() {
                    if let Ok(entries) = fs::read_dir(&dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.extension().and_then(|ext| ext.to_str()) != Some("ttf") {
                                continue;
                            }
                            let name = path
                                .file_stem()
                                .and_then(|name| name.to_str())
                                .unwrap_or("")
                                .to_ascii_lowercase();
                            let weight = if name.contains("thin") {
                                100
                            } else if name.contains("extralight") {
                                200
                            } else if name.contains("light") {
                                300
                            } else if name.contains("semibold") || name.contains("demibold") {
                                600
                            } else if name.contains("extrabold") || name.contains("ultrabold") {
                                800
                            } else if name.contains("bold") {
                                700
                            } else if name.contains("medium") {
                                500
                            } else {
                                400
                            };
                            if let Some(font) = load_font(path) {
                                faces.push((weight, font));
                            }
                        }
                    }
                }
            }
        }
        if faces.is_empty() {
            for (weight, path) in LATO {
                if let Some(font) = load_font(path) {
                    faces.push((*weight, font));
                }
            }
        }
        if faces.is_empty() {
            if let Some(font) = load_font("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf") {
                faces.push((400, font));
            }
            if let Some(font) = load_font("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf") {
                faces.push((700, font));
            }
        }
        if faces.is_empty() {
            return Err(SweetError::new(
                "no UI font found. Install Lato or DejaVu Sans, or set SWEET_FONT to a .ttf file",
            ));
        }
        let nerd = Font::from_bytes(
            include_bytes!("../assets/SymbolsNerdFont-Regular.ttf").to_vec(),
            fontdue::FontSettings::default(),
        )
        .map_err(|_| SweetError::new("the bundled Nerd Font Symbols font could not be loaded"))?;
        faces.sort_by_key(|(weight, _)| *weight);
        Ok(Self {
            faces,
            nerd,
            cache: Mutex::new(HashMap::new()),
        })
    }

    fn face_index(&self, weight: u16) -> usize {
        self.faces
            .iter()
            .enumerate()
            .min_by_key(|(_, (face_weight, _))| face_weight.abs_diff(weight))
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    fn face(&self, weight: u16) -> &Font {
        &self.faces[self.face_index(weight)].1
    }

    pub fn layout(
        &self,
        text: &str,
        size: f32,
        weight: u16,
        line_height: f32,
        max_width: f32,
        nowrap: bool,
    ) -> TextLayout {
        let font = self.face(weight);
        let size = size.max(0.0);
        let natural = font
            .horizontal_line_metrics(size.max(1.0))
            .map(|metrics| metrics.new_line_size)
            .unwrap_or(size * 1.2);
        let line_box = if line_height > 0.0 {
            line_height
        } else {
            natural
        };
        if text.is_empty() {
            return TextLayout {
                width: 0.0,
                height: 0.0,
                line_box,
                lines: Vec::new(),
            };
        }

        let mut lines = Vec::new();
        let mut base = 0usize;
        for paragraph in split_newlines(text) {
            push_wrapped(font, base, paragraph, size, max_width, nowrap, &mut lines);
            base += paragraph.len() + 1;
        }
        let width = lines.iter().fold(0.0f32, |max, line| max.max(line.width));
        let height = line_box * lines.len() as f32;
        TextLayout {
            width,
            height,
            line_box,
            lines,
        }
    }

    pub fn draw(
        &self,
        pixmap: &mut tiny_skia::Pixmap,
        text: &str,
        origin_x: f32,
        origin_y: f32,
        box_width: f32,
        size: f32,
        weight: u16,
        line_height: f32,
        align: TextAlign,
        color: Rgba,
        scale: f32,
        max_width: f32,
        nowrap: bool,
        clip: Clip,
    ) {
        if text.is_empty() || color.a == 0 || size <= 0.0 || scale <= 0.0 {
            return;
        }
        let layout = self.layout(text, size, weight, line_height, max_width, nowrap);
        let face = self.face_index(weight);
        let font = &self.faces[face].1;
        let metrics = font.horizontal_line_metrics(size);
        let ascent = metrics.map(|m| m.ascent).unwrap_or(size * 0.8);
        let content = metrics.map(|m| m.ascent - m.descent).unwrap_or(size);
        let extra = (layout.line_box - content).max(0.0);
        let baseline_offset = extra * 0.5 + ascent;
        let px = size * scale;

        for (index, line) in layout.lines.iter().enumerate() {
            let slack = (box_width - line.width).max(0.0);
            let align_x = match align {
                TextAlign::Left => 0.0,
                TextAlign::Center => slack * 0.5,
                TextAlign::Right => slack,
            };
            let mut pen = origin_x + align_x;
            let baseline = origin_y + layout.line_box * index as f32 + baseline_offset;
            let mut previous = None;
            for ch in text[line.start..line.end].chars() {
                if let Some(previous) = previous {
                    pen += font.horizontal_kern(previous, ch, size).unwrap_or(0.0);
                }
                let advance = self.stamp(
                    face,
                    font,
                    ch,
                    px,
                    pen * scale,
                    baseline * scale,
                    pixmap,
                    color,
                    clip,
                );
                pen += advance / scale;
                previous = Some(ch);
            }
        }
    }

    pub fn measure_nerd_glyph(&self, ch: char, size: f32) -> f32 {
        self.nerd.metrics(ch, size.max(1.0)).advance_width
    }

    pub fn draw_nerd_glyph(
        &self,
        pixmap: &mut tiny_skia::Pixmap,
        ch: char,
        origin_x: f32,
        origin_y: f32,
        box_width: f32,
        size: f32,
        line_height: f32,
        align: TextAlign,
        color: Rgba,
        scale: f32,
        clip: Clip,
    ) {
        if color.a == 0 || size <= 0.0 || scale <= 0.0 {
            return;
        }
        let metrics = self.nerd.horizontal_line_metrics(size);
        let ascent = metrics.map(|m| m.ascent).unwrap_or(size * 0.8);
        let content = metrics.map(|m| m.ascent - m.descent).unwrap_or(size);
        let baseline = origin_y + (line_height - content).max(0.0) * 0.5 + ascent;
        let advance = self.measure_nerd_glyph(ch, size);
        let offset = match align {
            TextAlign::Left => 0.0,
            TextAlign::Center => (box_width - advance).max(0.0) * 0.5,
            TextAlign::Right => (box_width - advance).max(0.0),
        };
        self.stamp(
            self.faces.len(),
            &self.nerd,
            ch,
            size * scale,
            (origin_x + offset) * scale,
            baseline * scale,
            pixmap,
            color,
            clip,
        );
    }

    /// Draw one glyph and return its advance in device pixels.
    fn stamp(
        &self,
        face: usize,
        font: &Font,
        ch: char,
        px: f32,
        device_x: f32,
        baseline: f32,
        pixmap: &mut tiny_skia::Pixmap,
        color: Rgba,
        clip: Clip,
    ) -> f32 {
        let key = (face as u16, px.to_bits(), ch);
        let mut cache = self.cache.lock().expect("glyph cache");
        if cache.len() >= GLYPH_CACHE_LIMIT && !cache.contains_key(&key) {
            cache.clear();
        }
        let glyph = cache.entry(key).or_insert_with(|| {
            let (metrics, bitmap) = font.rasterize(ch, px);
            CachedGlyph { metrics, bitmap }
        });
        let x = device_x + glyph.metrics.xmin as f32;
        let y = baseline - glyph.metrics.height as f32 - glyph.metrics.ymin as f32;
        blit(
            pixmap,
            &glyph.bitmap,
            glyph.metrics.width,
            x,
            y,
            color,
            clip,
        );
        glyph.metrics.advance_width
    }
}

const LATO_FILES: &[(u16, &str)] = &[
    (300, "Lato-Light.ttf"),
    (400, "Lato-Regular.ttf"),
    (500, "Lato-Medium.ttf"),
    (600, "Lato-Semibold.ttf"),
    (700, "Lato-Bold.ttf"),
    (900, "Lato-Black.ttf"),
];

const LATO: &[(u16, &str)] = &[
    (300, "/usr/share/fonts/truetype/lato/Lato-Light.ttf"),
    (400, "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"),
    (500, "/usr/share/fonts/truetype/lato/Lato-Medium.ttf"),
    (600, "/usr/share/fonts/truetype/lato/Lato-Semibold.ttf"),
    (700, "/usr/share/fonts/truetype/lato/Lato-Bold.ttf"),
    (900, "/usr/share/fonts/truetype/lato/Lato-Black.ttf"),
];

fn load_font(path: impl AsRef<Path>) -> Option<Font> {
    let bytes = fs::read(path).ok()?;
    Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
}

fn split_newlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            lines.push(&text[start..index]);
            start = index + 1;
        }
    }
    lines.push(&text[start..]);
    lines
}

fn push_wrapped(
    font: &Font,
    base: usize,
    paragraph: &str,
    size: f32,
    max_width: f32,
    nowrap: bool,
    out: &mut Vec<Line>,
) {
    if paragraph.is_empty() {
        out.push(Line {
            start: base,
            end: base,
            width: 0.0,
        });
        return;
    }
    let limit = if nowrap || !max_width.is_finite() {
        f32::INFINITY
    } else {
        max_width.max(0.0)
    };
    let mut line_start = 0usize;
    while line_start < paragraph.len() {
        let mut line_width = 0.0;
        let mut break_at = None;
        let mut previous = None;
        let mut line_end = paragraph.len();
        let mut next_start = paragraph.len();
        for (relative, ch) in paragraph[line_start..].char_indices() {
            let offset = line_start + relative;
            let advance = font.metrics(ch, size).advance_width
                + previous.map_or(0.0, |prev| font.horizontal_kern(prev, ch, size).unwrap_or(0.0));
            if line_width + advance > limit + 0.5 && offset > line_start {
                // Restart at the last word boundary. Characters already scanned
                // after it belong to the next line and must be measured again.
                line_end = if ch == ' ' { offset } else { break_at.unwrap_or(offset) };
                next_start = skip_spaces(paragraph, line_end);
                break;
            }
            line_width += advance;
            if ch == ' ' {
                break_at = Some(offset + ch.len_utf8());
            }
            previous = Some(ch);
        }
        let trimmed = trim_end_space(paragraph, line_start, line_end);
        out.push(Line {
            start: base + line_start,
            end: base + trimmed,
            width: measure(font, &paragraph[line_start..trimmed], size),
        });
        line_start = next_start;
    }
}

fn trim_end_space(text: &str, start: usize, mut end: usize) -> usize {
    while end > start && text.as_bytes()[end - 1] == b' ' {
        end -= 1;
    }
    end
}

fn skip_spaces(text: &str, mut index: usize) -> usize {
    while index < text.len() && text.as_bytes()[index] == b' ' {
        index += 1;
    }
    index
}

fn measure(font: &Font, text: &str, size: f32) -> f32 {
    let mut width = 0.0;
    let mut previous = None;
    for ch in text.chars() {
        if let Some(previous) = previous {
            width += font.horizontal_kern(previous, ch, size).unwrap_or(0.0);
        }
        width += font.metrics(ch, size).advance_width;
        previous = Some(ch);
    }
    width
}

fn blit(
    pixmap: &mut tiny_skia::Pixmap,
    bitmap: &[u8],
    glyph_width: usize,
    left: f32,
    top: f32,
    color: Rgba,
    clip: Clip,
) {
    if glyph_width == 0 || bitmap.is_empty() {
        return;
    }
    let glyph_height = bitmap.len() / glyph_width;
    let dest_x = left.floor() as i32;
    let dest_y = top.floor() as i32;
    let pixmap_width = pixmap.width() as i32;
    let pixmap_height = pixmap.height() as i32;
    let clip_x0 = clip.x.floor() as i32;
    let clip_y0 = clip.y.floor() as i32;
    let clip_x1 = (clip.x + clip.w).ceil() as i32;
    let clip_y1 = (clip.y + clip.h).ceil() as i32;
    let pixels = pixmap.pixels_mut();

    for row in 0..glyph_height {
        let y = dest_y + row as i32;
        if y < clip_y0 || y >= clip_y1 || y < 0 || y >= pixmap_height {
            continue;
        }
        for col in 0..glyph_width {
            let x = dest_x + col as i32;
            if x < clip_x0 || x >= clip_x1 || x < 0 || x >= pixmap_width {
                continue;
            }
            let coverage = bitmap[row * glyph_width + col];
            if coverage == 0 {
                continue;
            }
            let alpha = ((color.a as u16 * coverage as u16) / 255) as u8;
            if alpha == 0 {
                continue;
            }
            let index = (y as u32 * pixmap_width as u32 + x as u32) as usize;
            let dst = &mut pixels[index];
            let inv = 255 - alpha as u16;
            let r = ((color.r as u16 * alpha as u16) / 255) + (dst.red() as u16 * inv) / 255;
            let g = ((color.g as u16 * alpha as u16) / 255) + (dst.green() as u16 * inv) / 255;
            let b = ((color.b as u16 * alpha as u16) / 255) + (dst.blue() as u16 * inv) / 255;
            let a = alpha as u16 + (dst.alpha() as u16 * inv) / 255;
            if let Some(next) = tiny_skia::PremultipliedColorU8::from_rgba(
                r.min(255) as u8,
                g.min(255) as u8,
                b.min(255) as u8,
                a.min(255) as u8,
            ) {
                *dst = next;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_lines_fit_and_preserve_every_word() {
        let fonts = FontBook::load().unwrap();
        let text = "A name starts at the first song. Chopin - Nocturne op.9 No.2";
        for width in [45.0, 60.0, 90.0, 120.0] {
            let layout = fonts.layout(text, 12.0, 400, 16.0, width, false);
            for line in &layout.lines {
                assert!(
                    line.width <= width + 0.5,
                    "width {width}: {:?} measured {}",
                    &text[line.start..line.end],
                    line.width
                );
            }
            let joined: String = layout
                .lines
                .iter()
                .map(|line| &text[line.start..line.end])
                .collect();
            assert_eq!(joined.replace(' ', ""), text.replace(' ', ""));
            assert_eq!(layout.height, layout.lines.len() as f32 * 16.0);
        }
    }
}
