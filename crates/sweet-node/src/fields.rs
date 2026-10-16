//! egui `TextEdit` widgets placed in sweet `input` and `textarea` boxes.
//!
//! Tailwind paints the chrome. egui owns caret, selection, IME, and shortcuts.
//! The meshes are drawn into the window buffer in software.

use std::collections::HashMap;
use std::time::Duration;

use egui::epaint::{emath::Pos2, ClippedPrimitive, ImageData, Primitive, TextureId, Vertex};
use egui::TexturesDelta;
use egui::{pos2, vec2, Align, Color32, Context, FontId, Id, Rect, TextEdit, ViewportId};
use egui_winit::State;
use sweet_core::{NodeId, TextField};
use winit::event::WindowEvent;
use winit::window::{Theme, Window};

struct Atlas {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

pub struct Fields {
    pub ctx: Context,
    pub state: State,
    textures: HashMap<u64, Atlas>,
    pub repaint_after: Option<Duration>,
}

pub struct FieldUpdate {
    pub id: NodeId,
    pub value: String,
    pub submit: bool,
}

impl Fields {
    pub fn new(window: &Window) -> Self {
        let ctx = Context::default();
        let state = State::new(
            ctx.clone(),
            ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None::<Theme>,
            None,
        );
        Self {
            ctx,
            state,
            textures: HashMap::new(),
            repaint_after: None,
        }
    }

    pub fn on_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        let response = self.state.on_window_event(window, event);
        response.repaint
    }

    pub fn run(
        &mut self,
        window: &Window,
        fields: &[TextField],
        focused: Option<NodeId>,
        pixels: &mut [u32],
        pixel_width: u32,
        pixel_height: u32,
    ) -> Vec<FieldUpdate> {
        self.run_inner(window, fields, focused, Some((pixels, pixel_width, pixel_height)))
    }

    pub fn process_input(
        &mut self,
        window: &Window,
        fields: &[TextField],
        focused: Option<NodeId>,
    ) -> Vec<FieldUpdate> {
        self.run_inner(window, fields, focused, None)
    }

    fn run_inner(
        &mut self,
        window: &Window,
        fields: &[TextField],
        focused: Option<NodeId>,
        target: Option<(&mut [u32], u32, u32)>,
    ) -> Vec<FieldUpdate> {
        let raw = self.state.take_egui_input(window);
        // Android's Activity EditText owns keyboard input there. Keep this
        // widget as a painter so its cached text state cannot overwrite the
        // value arriving from the Java IME bridge.
        let mut updates = Vec::new();
        let output = self.ctx.run(raw, |ctx| {
            for field in fields {
                let mut text = field.value.clone();
                let edit_id = Id::new(("sweet-edit", field.id.0));
                let already = ctx.memory(|memory| memory.has_focus(edit_id));
                let interactive = !field.disabled && !cfg!(target_os = "android");
                if interactive && focused == Some(field.id) && !already {
                    ctx.memory_mut(|memory| memory.request_focus(edit_id));
                }
                egui::Area::new(Id::new(("sweet-area", field.id.0)))
                    .fixed_pos(pos2(field.x, field.y))
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        let bg = field.selection_background;
                        let fg = field.selection_color;
                        ui.visuals_mut().selection.bg_fill = Color32::from_rgba_unmultiplied(bg.r, bg.g, bg.b, bg.a);
                        ui.visuals_mut().selection.stroke.color = Color32::from_rgba_unmultiplied(fg.r, fg.g, fg.b, fg.a);
                        ui.set_clip_rect(Rect::from_min_size(
                            pos2(field.clip_x, field.clip_y),
                            vec2(field.clip_w, field.clip_h),
                        ));
                        ui.set_min_size(vec2(field.width, field.height));
                        ui.set_max_size(vec2(field.width, field.height));
                        let mut edit = if field.multiline {
                            TextEdit::multiline(&mut text)
                        } else {
                            TextEdit::singleline(&mut text)
                        };
                        edit = edit
                            .id(edit_id)
                            .password(field.password)
                            .frame(false)
                            .margin(egui::Margin::ZERO)
                            .desired_width(field.width)
                            .min_size(vec2(field.width, field.height))
                            .vertical_align(if field.multiline {
                                Align::Min
                            } else {
                                Align::Center
                            })
                            .hint_text(field.placeholder.as_str())
                            .font(FontId::proportional(field.font_size))
                            .text_color(Color32::from_rgba_unmultiplied(
                                field.color.r,
                                field.color.g,
                                field.color.b,
                                field.color.a,
                            ))
                            .interactive(interactive);
                        let response = ui.add(edit);
                        let submit = !field.multiline
                            && response.has_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        if text != field.value || submit {
                            updates.push(FieldUpdate {
                                id: field.id,
                                value: text,
                                submit,
                            });
                        }
                    });
            }
        });
        self.apply_textures(&output.textures_delta);
        if let Some((pixels, pixel_width, pixel_height)) = target {
            let clipped = self.ctx.tessellate(output.shapes, output.pixels_per_point);
            paint_meshes(
                pixels,
                pixel_width,
                pixel_height,
                output.pixels_per_point,
                &clipped,
                &self.textures,
            );
        }
        for id in &output.textures_delta.free {
            if let TextureId::Managed(id) = id {
                self.textures.remove(id);
            }
        }
        self.state
            .handle_platform_output(window, output.platform_output);
        self.repaint_after = output
            .viewport_output
            .values()
            .map(|viewport| viewport.repaint_delay)
            .min();
        updates
    }
}

fn paint_meshes(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    pixels_per_point: f32,
    clipped: &[ClippedPrimitive],
    textures: &HashMap<u64, Atlas>,
) {
    for clipped in clipped {
        let Primitive::Mesh(mesh) = &clipped.primitive else {
            continue;
        };
        let Some(atlas) = texture(textures, mesh.texture_id) else {
            continue;
        };
        let clip = rect_px(clipped.clip_rect, pixels_per_point, width, height);
        for triangle in mesh.indices.chunks_exact(3) {
            let v0 = &mesh.vertices[triangle[0] as usize];
            let v1 = &mesh.vertices[triangle[1] as usize];
            let v2 = &mesh.vertices[triangle[2] as usize];
            fill_triangle(
                pixels,
                width,
                height,
                clip,
                pixels_per_point,
                v0,
                v1,
                v2,
                atlas,
            );
        }
    }
}

fn texture(textures: &HashMap<u64, Atlas>, id: TextureId) -> Option<&Atlas> {
    match id {
        TextureId::Managed(id) => textures.get(&id),
        TextureId::User(_) => None,
    }
}

fn rect_px(rect: Rect, scale: f32, width: u32, height: u32) -> [i32; 4] {
    [
        (rect.min.x * scale).floor().max(0.0) as i32,
        (rect.min.y * scale).floor().max(0.0) as i32,
        (rect.max.x * scale).ceil().min(width as f32) as i32,
        (rect.max.y * scale).ceil().min(height as f32) as i32,
    ]
}

fn fill_triangle(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    clip: [i32; 4],
    scale: f32,
    v0: &Vertex,
    v1: &Vertex,
    v2: &Vertex,
    atlas: &Atlas,
) {
    let p0 = pos_px(v0.pos, scale);
    let p1 = pos_px(v1.pos, scale);
    let p2 = pos_px(v2.pos, scale);
    let min_x = p0.x.min(p1.x).min(p2.x).floor().max(clip[0] as f32) as i32;
    let min_y = p0.y.min(p1.y).min(p2.y).floor().max(clip[1] as f32) as i32;
    let max_x = p0.x.max(p1.x).max(p2.x).ceil().min(clip[2] as f32) as i32;
    let max_y = p0.y.max(p1.y).max(p2.y).ceil().min(clip[3] as f32) as i32;
    let area = edge(p0, p1, p2);
    if area.abs() < 0.5 || max_x <= min_x || max_y <= min_y {
        return;
    }
    for y in min_y..max_y.min(height as i32) {
        for x in min_x..max_x.min(width as i32) {
            let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
            let mut w0 = edge(p1, p2, p);
            let mut w1 = edge(p2, p0, p);
            let mut w2 = edge(p0, p1, p);
            if area < 0.0 {
                w0 = -w0;
                w1 = -w1;
                w2 = -w2;
            }
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let inv = 1.0 / area.abs();
            w0 *= inv;
            w1 *= inv;
            w2 *= inv;
            let u = v0.uv.x * w0 + v1.uv.x * w1 + v2.uv.x * w2;
            let v = v0.uv.y * w0 + v1.uv.y * w1 + v2.uv.y * w2;
            let texel = sample(atlas, u, v);
            let color = blend_vertex(v0, v1, v2, w0, w1, w2);
            let src = modulate(color, texel);
            let index = (y as u32 * width + x as u32) as usize;
            if index < pixels.len() {
                pixels[index] = over(pixels[index], src);
            }
        }
    }
}

fn pos_px(pos: Pos2, scale: f32) -> Pos2 {
    Pos2::new(pos.x * scale, pos.y * scale)
}

fn edge(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    (p.x - a.x) * (b.y - a.y) - (p.y - a.y) * (b.x - a.x)
}

fn sample(atlas: &Atlas, u: f32, v: f32) -> [u8; 4] {
    if atlas.width == 0 || atlas.height == 0 {
        return [255, 255, 255, 255];
    }
    let x = (u * atlas.width as f32)
        .floor()
        .clamp(0.0, (atlas.width - 1) as f32) as usize;
    let y = (v * atlas.height as f32)
        .floor()
        .clamp(0.0, (atlas.height - 1) as f32) as usize;
    let offset = (y * atlas.width + x) * 4;
    [
        atlas.rgba[offset],
        atlas.rgba[offset + 1],
        atlas.rgba[offset + 2],
        atlas.rgba[offset + 3],
    ]
}

fn blend_vertex(v0: &Vertex, v1: &Vertex, v2: &Vertex, w0: f32, w1: f32, w2: f32) -> [u8; 4] {
    let mix = |a: u8, b: u8, c: u8| -> u8 {
        (a as f32 * w0 + b as f32 * w1 + c as f32 * w2)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    [
        mix(v0.color.r(), v1.color.r(), v2.color.r()),
        mix(v0.color.g(), v1.color.g(), v2.color.g()),
        mix(v0.color.b(), v1.color.b(), v2.color.b()),
        mix(v0.color.a(), v1.color.a(), v2.color.a()),
    ]
}

fn modulate(color: [u8; 4], texel: [u8; 4]) -> [u8; 4] {
    [
        ((color[0] as u16 * texel[0] as u16) / 255) as u8,
        ((color[1] as u16 * texel[1] as u16) / 255) as u8,
        ((color[2] as u16 * texel[2] as u16) / 255) as u8,
        ((color[3] as u16 * texel[3] as u16) / 255) as u8,
    ]
}

fn over(dst: u32, src: [u8; 4]) -> u32 {
    let sa = src[3] as u32;
    if sa == 0 {
        return dst;
    }
    let da = dst >> 24;
    let dr = (dst >> 16) & 0xff;
    let dg = (dst >> 8) & 0xff;
    let db = dst & 0xff;
    let inv = 255 - sa;
    let a = sa + da * inv / 255;
    let r = src[0] as u32 + dr * inv / 255;
    let g = src[1] as u32 + dg * inv / 255;
    let b = src[2] as u32 + db * inv / 255;
    (a << 24) | (r << 16) | (g << 8) | b
}

impl Fields {
    fn apply_textures(&mut self, delta: &TexturesDelta) {
        for (id, patch) in &delta.set {
            let TextureId::Managed(key) = id else {
                continue;
            };
            let [w, h] = patch.image.size();
            let rgba = image_rgba(&patch.image);
            if let Some(pos) = patch.pos {
                let Some(atlas) = self.textures.get_mut(&key) else {
                    continue;
                };
                blit_patch(atlas, pos, w, h, &rgba);
            } else {
                self.textures.insert(
                    *key,
                    Atlas {
                        width: w,
                        height: h,
                        rgba,
                    },
                );
            }
        }
    }
}

fn image_rgba(image: &ImageData) -> Vec<u8> {
    match image {
        ImageData::Color(image) => {
            let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
            for pixel in &image.pixels {
                let bytes: [u8; 4] = pixel.to_array();
                rgba.extend_from_slice(&bytes);
            }
            rgba
        }
        ImageData::Font(image) => {
            let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
            for pixel in image.srgba_pixels(None) {
                let bytes: [u8; 4] = pixel.to_array();
                rgba.extend_from_slice(&bytes);
            }
            rgba
        }
    }
}

fn blit_patch(atlas: &mut Atlas, pos: [usize; 2], patch_w: usize, patch_h: usize, rgba: &[u8]) {
    for row in 0..patch_h {
        let y = pos[1] + row;
        if y >= atlas.height {
            break;
        }
        for col in 0..patch_w {
            let x = pos[0] + col;
            if x >= atlas.width {
                break;
            }
            let src = (row * patch_w + col) * 4;
            let dst = (y * atlas.width + x) * 4;
            if src + 4 <= rgba.len() && dst + 4 <= atlas.rgba.len() {
                atlas.rgba[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
            }
        }
    }
}
