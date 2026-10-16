use std::ffi::{c_char, CStr, CString};
use std::collections::HashMap;
use std::ptr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;
use std::time::Instant;
use std::sync::atomic::AtomicBool;

use sweet_core::{render_display_list, resize_edge, DisplayList, Document as CoreDocument, NodeId, Picture, PicturePixels, Quad, RenderOptions, ResizeAxes, ResizeEdge};
use yscv_video::Mp4VideoReader;

pub struct PhpDocument { core: Arc<CoreDocument> }

fn core(document: *const PhpDocument) -> &'static CoreDocument {
    unsafe { &*(*document).core }
}

fn string(value: *const c_char) -> Result<String, String> {
    if value.is_null() {
        return Err("string argument is null".into());
    }
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .map(str::to_owned)
        .map_err(|error| error.to_string())
}

fn node(id: u32) -> NodeId { NodeId(id) }

fn report(error: impl ToString, out_error: *mut *mut c_char) {
    if !out_error.is_null() {
        if let Ok(message) = CString::new(error.to_string()) {
            unsafe { *out_error = message.into_raw(); }
        }
    }
}

#[no_mangle]
pub extern "C" fn sweet_string_free(value: *mut c_char) {
    if !value.is_null() { unsafe { drop(CString::from_raw(value)); } }
}

#[no_mangle]
pub extern "C" fn sweet_document_new(error: *mut *mut c_char) -> *mut PhpDocument {
    match CoreDocument::new() {
        Ok(document) => Box::into_raw(Box::new(PhpDocument { core: Arc::new(document) })),
        Err(problem) => { report(problem, error); ptr::null_mut() }
    }
}

#[no_mangle]
pub extern "C" fn sweet_document_free(document: *mut PhpDocument) {
    if !document.is_null() { unsafe { drop(Box::from_raw(document)); } }
}

#[no_mangle]
pub extern "C" fn sweet_body(_document: *const PhpDocument) -> u32 { 0 }

#[no_mangle]
pub extern "C" fn sweet_create_element(document: *const PhpDocument, tag: *const c_char, error: *mut *mut c_char) -> u32 {
    let result = (|| { let tag = string(tag)?; core(document).create_element(&tag).map(|id| id.0).map_err(|e| e.to_string()) })();
    result.unwrap_or_else(|e| { report(e, error); u32::MAX })
}

#[no_mangle]
pub extern "C" fn sweet_create_text(document: *const PhpDocument, text: *const c_char, error: *mut *mut c_char) -> u32 {
    let result = (|| { let text = string(text)?; Ok::<u32, String>(core(document).create_text(&text).0) })();
    result.unwrap_or_else(|e| { report(e, error); u32::MAX })
}

#[no_mangle]
pub extern "C" fn sweet_append_child(document: *const PhpDocument, parent: u32, child: u32, error: *mut *mut c_char) -> bool {
    match core(document).append_child(node(parent), node(child)) { Ok(()) => true, Err(e) => { report(e, error); false } }
}

#[no_mangle]
pub extern "C" fn sweet_set_class(document: *const PhpDocument, id: u32, value: *const c_char, error: *mut *mut c_char) -> bool {
    let result = (|| { let value = string(value)?; core(document).set_class_name(node(id), &value).map_err(|e| e.to_string()) })();
    match result { Ok(()) => true, Err(e) => { report(e, error); false } }
}

#[no_mangle]
pub extern "C" fn sweet_set_text(document: *const PhpDocument, id: u32, value: *const c_char, error: *mut *mut c_char) -> bool {
    let result = (|| { let value = string(value)?; core(document).set_text_content(node(id), &value).map_err(|e| e.to_string()) })();
    match result { Ok(()) => true, Err(e) => { report(e, error); false } }
}

#[no_mangle]
pub extern "C" fn sweet_set_image_encoded(document: *const PhpDocument, id: u32, bytes: *const u8, length: usize, error: *mut *mut c_char) -> bool {
    if bytes.is_null() { report("image bytes pointer is null", error); return false; }
    let data = unsafe { std::slice::from_raw_parts(bytes, length) };
    match core(document).set_image_encoded(node(id), data) {
        Ok(_) => true,
        Err(problem) => { report(problem, error); false }
    }
}

#[no_mangle]
pub extern "C" fn sweet_set_text_content(document: *const PhpDocument, id: u32, value: *const c_char, error: *mut *mut c_char) -> bool {
    let result = (|| { let value = string(value)?; core(document).set_text_content(node(id), &value).map_err(|e| e.to_string()) })();
    match result { Ok(()) => true, Err(e) => { report(e, error); false } }
}

#[no_mangle]
pub extern "C" fn sweet_set_attribute(document: *const PhpDocument, id: u32, name: *const c_char, value: *const c_char, error: *mut *mut c_char) -> bool {
    let result = (|| { let name = string(name)?; let value = string(value)?; core(document).set_attribute(node(id), &name, &value).map_err(|e| e.to_string()) })();
    match result { Ok(()) => true, Err(e) => { report(e, error); false } }
}

#[no_mangle]
pub extern "C" fn sweet_remove_attribute(document: *const PhpDocument, id: u32, name: *const c_char, error: *mut *mut c_char) -> bool {
    let result = (|| { let name = string(name)?; core(document).remove_attribute(node(id), &name).map_err(|e| e.to_string()) })();
    match result { Ok(()) => true, Err(e) => { report(e, error); false } }
}

#[no_mangle]
pub extern "C" fn sweet_create_comment(document: *const PhpDocument, text: *const c_char) -> u32 {
    let text = string(text).unwrap_or_default(); core(document).create_comment(&text).0
}

#[no_mangle]
pub extern "C" fn sweet_insert_before(document: *const PhpDocument, parent: u32, child: u32, before: u32, error: *mut *mut c_char) -> bool {
    match core(document).insert_before(node(parent), node(child), (before != u32::MAX).then(|| node(before))) {
        Ok(()) => true, Err(e) => { report(e, error); false }
    }
}

#[no_mangle]
pub extern "C" fn sweet_remove_child(document: *const PhpDocument, parent: u32, child: u32, error: *mut *mut c_char) -> bool {
    match core(document).remove_child(node(parent), node(child)) { Ok(()) => true, Err(e) => { report(e, error); false } }
}

#[no_mangle]
pub extern "C" fn sweet_child_count(document: *const PhpDocument, id: u32, error: *mut *mut c_char) -> usize {
    match core(document).children(node(id)) { Ok(children) => children.len(), Err(e) => { report(e, error); usize::MAX } }
}

#[no_mangle]
pub extern "C" fn sweet_child_at(document: *const PhpDocument, id: u32, index: usize, error: *mut *mut c_char) -> u32 {
    match core(document).children(node(id)).and_then(|children| children.get(index).copied().ok_or_else(|| sweet_core::SweetError::new("child index out of range"))) {
        Ok(id) => id.0, Err(e) => { report(e, error); u32::MAX }
    }
}

#[no_mangle]
pub extern "C" fn sweet_parent(document: *const PhpDocument, id: u32) -> u32 {
    core(document).parent(node(id)).ok().flatten().map_or(u32::MAX, |parent| parent.0)
}

#[no_mangle]
pub extern "C" fn sweet_get_attribute(document: *const PhpDocument, id: u32, name: *const c_char, error: *mut *mut c_char) -> *mut c_char {
    let result = (|| { let name = string(name)?; core(document).attribute(node(id), &name).map_err(|e| e.to_string()) })();
    match result { Ok(Some(value)) => CString::new(value).map(CString::into_raw).unwrap_or_else(|e| { report(e, error); ptr::null_mut() }), Ok(None) => ptr::null_mut(), Err(e) => { report(e, error); ptr::null_mut() } }
}

#[no_mangle]
pub extern "C" fn sweet_get_class(document: *const PhpDocument, id: u32, error: *mut *mut c_char) -> *mut c_char {
    match core(document).class_name(node(id)) { Ok(value) => CString::new(value).map(CString::into_raw).unwrap_or(ptr::null_mut()), Err(e) => { report(e, error); ptr::null_mut() } }
}

#[no_mangle]
pub extern "C" fn sweet_get_text(document: *const PhpDocument, id: u32, error: *mut *mut c_char) -> *mut c_char {
    match core(document).text_content(node(id)) { Ok(value) => CString::new(value).map(CString::into_raw).unwrap_or(ptr::null_mut()), Err(e) => { report(e, error); ptr::null_mut() } }
}

#[no_mangle]
pub extern "C" fn sweet_node_type(document: *const PhpDocument, id: u32, error: *mut *mut c_char) -> u32 {
    match core(document).node_type(node(id)) { Ok(value) => value, Err(e) => { report(e, error); 0 } }
}

#[no_mangle]
pub extern "C" fn sweet_tag_name(document: *const PhpDocument, id: u32, error: *mut *mut c_char) -> *mut c_char {
    match core(document).tag_name(node(id)) { Ok(value) => CString::new(value).map(CString::into_raw).unwrap_or(ptr::null_mut()), Err(e) => { report(e, error); ptr::null_mut() } }
}

#[no_mangle]
pub extern "C" fn sweet_dark(document: *const PhpDocument) -> bool { core(document).dark() }
#[no_mangle]
pub extern "C" fn sweet_set_dark(document: *const PhpDocument, dark: bool) { core(document).set_dark(dark); }

#[no_mangle]
pub extern "C" fn sweet_render_png(document: *const PhpDocument, width: u32, height: u32, length: *mut usize, error: *mut *mut c_char) -> *mut u8 {
    match core(document).render_png(width, height) {
        Ok(bytes) => { unsafe { *length = bytes.len(); } let boxed = bytes.into_boxed_slice(); Box::into_raw(boxed) as *mut u8 }
        Err(problem) => { report(problem, error); ptr::null_mut() }
    }
}

#[no_mangle]
pub extern "C" fn sweet_bytes_free(value: *mut u8, length: usize) {
    if !value.is_null() { unsafe { drop(Box::from_raw(ptr::slice_from_raw_parts_mut(value, length))); } }
}

struct WindowState { closed: Mutex<bool>, wake: std::sync::Condvar }
impl WindowState {
    fn close(&self) { *self.closed.lock().unwrap() = true; self.wake.notify_all(); }
}
enum WindowCommand { Redraw, Close, Minimize, ToggleMaximize, SetResizable(bool), SetResizeAxes(bool, bool) }
struct RedrawHookState { proxy: winit::event_loop::EventLoopProxy<WindowCommand>, pending: std::sync::atomic::AtomicBool }
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct NativeEvent { pub kind: u32, pub node_id: u32, pub x: f64, pub y: f64, pub width: f64, pub height: f64 }
static NEXT_WINDOW: AtomicU64 = AtomicU64::new(1);
pub struct PhpWindow {
    proxy: winit::event_loop::EventLoopProxy<WindowCommand>,
    state: Arc<WindowState>,
    document: Arc<CoreDocument>,
    hook: u64,
    events: Arc<Mutex<Vec<NativeEvent>>>,
    media_stop: Arc<AtomicBool>,
}

struct ResizeDrag {
    direction: ResizeEdge,
    start_x: f64,
    start_y: f64,
    start_position: winit::dpi::PhysicalPosition<i32>,
    start_size: winit::dpi::PhysicalSize<u32>,
}

fn native_resize_direction(direction: ResizeEdge) -> winit::window::ResizeDirection {
    use winit::window::ResizeDirection as Native;
    match direction {
        ResizeEdge::East => Native::East,
        ResizeEdge::North => Native::North,
        ResizeEdge::NorthEast => Native::NorthEast,
        ResizeEdge::NorthWest => Native::NorthWest,
        ResizeEdge::South => Native::South,
        ResizeEdge::SouthEast => Native::SouthEast,
        ResizeEdge::SouthWest => Native::SouthWest,
        ResizeEdge::West => Native::West,
    }
}

fn resize_borderless(window: &winit::window::Window, drag: &ResizeDrag, x: f64, y: f64) {
    use winit::dpi::{PhysicalPosition, PhysicalSize};
    let dx = (x - drag.start_x).round() as i32;
    let dy = (y - drag.start_y).round() as i32;
    let west = matches!(drag.direction, ResizeEdge::West | ResizeEdge::NorthWest | ResizeEdge::SouthWest);
    let east = matches!(drag.direction, ResizeEdge::East | ResizeEdge::NorthEast | ResizeEdge::SouthEast);
    let north = matches!(drag.direction, ResizeEdge::North | ResizeEdge::NorthEast | ResizeEdge::NorthWest);
    let south = matches!(drag.direction, ResizeEdge::South | ResizeEdge::SouthEast | ResizeEdge::SouthWest);
    let width = drag.start_size.width as i32;
    let height = drag.start_size.height as i32;
    let next_width = if west { (width - dx).max(320) } else if east { (width + dx).max(320) } else { width };
    let next_height = if north { (height - dy).max(240) } else if south { (height + dy).max(240) } else { height };
    let position = PhysicalPosition::new(
        drag.start_position.x + if west { width - next_width } else { 0 },
        drag.start_position.y + if north { height - next_height } else { 0 },
    );
    let _ = window.request_inner_size(PhysicalSize::new(next_width as u32, next_height as u32));
    window.set_outer_position(position);
}

const QUAD_FLOATS: u32 = 76;

struct CachedPicture {
    stamp: u64,
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

struct GpuPresenter {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    uniform_layout: wgpu::BindGroupLayout,
    nearest: wgpu::Sampler,
    linear: wgpu::Sampler,
    pictures: HashMap<u64, CachedPicture>,
    uniform: Option<wgpu::Buffer>,
    uniform_group: Option<wgpu::BindGroup>,
    uniform_capacity: usize,
    stride: u32,
}

impl GpuPresenter {
    fn new(window: Arc<winit::window::Window>, width: u32, height: u32) -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window).map_err(|e| format!("create GPU surface: {e}"))?;
        let adapter = futures_executor::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: Some(&surface), force_fallback_adapter: false,
        })).ok_or_else(|| "no GPU adapter can present this window".to_string())?;
        let mut limits = wgpu::Limits::downlevel_defaults();
        limits.max_texture_dimension_2d = adapter.limits().max_texture_dimension_2d;
        let (device, queue) = futures_executor::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Sweet compositor"), required_features: wgpu::Features::empty(), required_limits: limits,
            memory_hints: wgpu::MemoryHints::Performance,
        }, None)).map_err(|e| format!("create GPU device: {e}"))?;
        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| *f == wgpu::TextureFormat::Bgra8Unorm)
            .or_else(|| caps.formats.iter().copied().find(|f| !f.is_srgb())).unwrap_or(caps.formats[0]);
        let alpha_mode = caps.alpha_modes.iter().copied().find(|mode| *mode == wgpu::CompositeAlphaMode::PreMultiplied)
            .or_else(|| caps.alpha_modes.iter().copied().find(|mode| *mode == wgpu::CompositeAlphaMode::Opaque))
            .unwrap_or(caps.alpha_modes[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, format, width: width.max(1), height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo, desired_maximum_frame_latency: 1, alpha_mode, view_formats: vec![],
        };
        surface.configure(&device, &config);
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Sweet picture layout"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Sweet quad uniforms"), entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0, visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: None }, count: None,
            }],
        });
        let nearest = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("Sweet nearest"), mag_filter: wgpu::FilterMode::Nearest, min_filter: wgpu::FilterMode::Nearest, address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, ..Default::default() });
        let linear = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("Sweet linear"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, ..Default::default() });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("Sweet compositor shader"), source: wgpu::ShaderSource::Wgsl(COMPOSITE_WGSL.into()) });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("Sweet compositor pipeline layout"), bind_group_layouts: &[&texture_layout, &uniform_layout], push_constant_ranges: &[] });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Sweet compositor"), layout: Some(&layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() }, depth_stencil: None,
            multisample: wgpu::MultisampleState::default(), fragment: Some(wgpu::FragmentState {
                module: &shader, entry_point: Some("fs"), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
            }), multiview: None, cache: None,
        });
        let align = device.limits().min_uniform_buffer_offset_alignment.max(256);
        let stride = QUAD_FLOATS * 4;
        let stride = stride.div_ceil(align) * align;
        Ok(Self { _instance: instance, surface, device, queue, config, pipeline, texture_layout, uniform_layout, nearest, linear,
            pictures: HashMap::new(), uniform: None, uniform_group: None, uniform_capacity: 0, stride })
    }

    fn present_display(&mut self, window: &winit::window::Window, list: &DisplayList, width: u32, height: u32) -> Result<(), String> {
        if width == 0 || height == 0 { return Ok(()); }
        if self.config.width != width || self.config.height != height {
            self.config.width = width; self.config.height = height; self.surface.configure(&self.device, &self.config);
        }
        let mut live = Vec::with_capacity(list.pictures.len());
        for picture in &list.pictures {
            if picture.width == 0 || picture.height == 0 || picture.pixels.is_empty() { continue; }
            self.ensure_picture(picture)?; live.push(picture.key);
        }
        self.pictures.retain(|key, _| live.contains(key));
        self.write_uniforms(&list.quads, list, width, height)?;
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => { self.surface.configure(&self.device, &self.config); return Ok(()); }
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(error) => return Err(format!("acquire GPU frame: {error}")),
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Sweet composite") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Sweet composite"), color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            if let Some(uniforms) = self.uniform_group.as_ref() {
                for (index, quad) in list.quads.iter().enumerate() {
                    let Some(source) = list.pictures.get(quad.picture as usize) else { continue; };
                    let Some(picture) = self.pictures.get(&source.key) else { continue; };
                    pass.set_bind_group(0, &picture.bind_group, &[]);
                    pass.set_bind_group(1, uniforms, &[index as u32 * self.stride]);
                    pass.draw(0..6, 0..1);
                }
            }
        }
        self.queue.submit(Some(encoder.finish())); window.pre_present_notify(); frame.present(); Ok(())
    }

    fn ensure_picture(&mut self, picture: &Picture) -> Result<(), String> {
        let expected = (picture.stride as usize).saturating_mul(picture.height as usize);
        if picture.pixels.len() < expected { return Err("picture is smaller than its size".into()); }
        if let Some(cached) = self.pictures.get_mut(&picture.key) {
            if cached.width == picture.width && cached.height == picture.height {
                if cached.stamp == picture.stamp { return Ok(()); }
                write_picture(&self.queue, &cached.texture, picture.width, picture.height, picture.stride, &picture.pixels[..expected]);
                cached.stamp = picture.stamp;
                return Ok(());
            }
        }
        let cached = upload_picture(&self.device, &self.queue, &self.texture_layout, &self.nearest, &self.linear,
            picture.width, picture.height, picture.stride, &picture.pixels[..expected], picture.stamp)?;
        self.pictures.insert(picture.key, cached); Ok(())
    }

    fn write_uniforms(&mut self, quads: &[Quad], list: &DisplayList, width: u32, height: u32) -> Result<(), String> {
        let bytes = self.stride as usize * quads.len().max(1);
        if self.uniform_capacity < bytes {
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("Sweet quad uniforms"), size: bytes as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Sweet quad uniforms"), layout: &self.uniform_layout, entries: &[wgpu::BindGroupEntry {
                binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &buffer, offset: 0, size: std::num::NonZeroU64::new((QUAD_FLOATS * 4) as u64) }),
            }] });
            self.uniform = Some(buffer); self.uniform_group = Some(group); self.uniform_capacity = bytes;
        }
        let stride = self.stride as usize;
        let mut staging = vec![0u8; bytes];
        for (index, quad) in quads.iter().enumerate() {
            let picture = list.pictures.get(quad.picture as usize);
            let straight = picture.is_none_or(|p| p.kind == PicturePixels::Straight);
            let floats = quad_uniforms(quad, list.global_opacity, width, height, straight, false);
            let start = index * stride;
            for (slot, value) in floats.iter().enumerate() { staging[start + slot * 4..start + slot * 4 + 4].copy_from_slice(&value.to_ne_bytes()); }
        }
        if let Some(buffer) = &self.uniform { self.queue.write_buffer(buffer, 0, &staging); }
        Ok(())
    }
}

const COMPOSITE_WGSL: &str = r#"
struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) local: vec2<f32>,
}

struct Quad {
    c0: vec4<f32>,
    c1: vec4<f32>,
    c2: vec4<f32>,
    c3: vec4<f32>,
    uv0: vec4<f32>,
    uv1: vec4<f32>,
    uv2: vec4<f32>,
    uv3: vec4<f32>,
    l0: vec4<f32>,
    l1: vec4<f32>,
    l2: vec4<f32>,
    l3: vec4<f32>,
    clip: vec4<f32>,
    round: vec4<f32>,
    radii: vec4<f32>,
    local_clip: vec4<f32>,
    local_radii: vec4<f32>,
    // x straight, y swap rb, z opacity, w discard outside uv.
    flags: vec4<f32>,
    viewport: vec4<f32>,
}

@group(0) @binding(0) var picture_tex: texture_2d<f32>;
@group(0) @binding(1) var picture_nearest: sampler;
@group(0) @binding(2) var picture_linear: sampler;
@group(1) @binding(0) var<uniform> quad: Quad;

fn inside_round(p: vec2<f32>, box: vec4<f32>, radii: vec4<f32>) -> bool {
    if (box.z <= 0.0 || box.w <= 0.0) { return true; }
    if (p.x < box.x || p.y < box.y || p.x > box.x + box.z || p.y > box.y + box.w) {
        return false;
    }
    let right = p.x > box.x + box.z * 0.5;
    let bottom = p.y > box.y + box.w * 0.5;
    var radius = radii.x;
    if (right && !bottom) { radius = radii.y; }
    if (right && bottom) { radius = radii.z; }
    if (!right && bottom) { radius = radii.w; }
    radius = min(radius, min(box.z, box.w) * 0.5);
    if (radius < 0.5) { return true; }
    let cx = box.x + select(radius, box.z - radius, right);
    let cy = box.y + select(radius, box.w - radius, bottom);
    let corner_x = select(p.x < box.x + radius, p.x > box.x + box.z - radius, right);
    let corner_y = select(p.y < box.y + radius, p.y > box.y + box.w - radius, bottom);
    if (!corner_x || !corner_y) { return true; }
    let dx = p.x - cx;
    let dy = p.y - cy;
    return dx * dx + dy * dy <= radius * radius;
}

@vertex
fn vs(@builtin(vertex_index) vid: u32) -> Varyings {
    var corner: vec4<f32>;
    var uv: vec4<f32>;
    var local: vec4<f32>;
    switch vid {
        case 0u: { corner = quad.c0; uv = quad.uv0; local = quad.l0; }
        case 1u: { corner = quad.c1; uv = quad.uv1; local = quad.l1; }
        case 2u: { corner = quad.c2; uv = quad.uv2; local = quad.l2; }
        case 3u: { corner = quad.c0; uv = quad.uv0; local = quad.l0; }
        case 4u: { corner = quad.c2; uv = quad.uv2; local = quad.l2; }
        default: { corner = quad.c3; uv = quad.uv3; local = quad.l3; }
    }
    let w = max(corner.z, 0.0001);
    let ndc = vec2<f32>(
        (corner.x / max(quad.viewport.x, 1.0)) * 2.0 - 1.0,
        1.0 - (corner.y / max(quad.viewport.y, 1.0)) * 2.0,
    );
    var out: Varyings;
    out.position = vec4<f32>(ndc * w, 0.0, w);
    out.uv = uv.xy * w;
    out.local = local.xy * w;
    return out;
}

@fragment
fn fs(frag: Varyings) -> @location(0) vec4<f32> {
    let screen = frag.position.xy;
    if (!inside_round(screen, quad.clip, vec4<f32>(0.0))) { discard; }
    if (!inside_round(screen, quad.round, quad.radii)) { discard; }
    if (!inside_round(frag.local, quad.local_clip, quad.local_radii)) { discard; }
    let uv = frag.uv;
    if (quad.flags.w > 0.5 && (uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0)) {
        discard;
    }
    let sample_uv = clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0));
    var color: vec4<f32>;
    if (quad.viewport.z > 0.5) {
        color = textureSample(picture_tex, picture_linear, sample_uv);
    } else {
        color = textureSample(picture_tex, picture_nearest, sample_uv);
    }
    if (quad.flags.y > 0.5) {
        color = color.bgra;
    }
    if (quad.flags.x > 0.5) {
        color = vec4<f32>(color.rgb * color.a, color.a);
    }
    return color * quad.flags.z;
}
"#;


fn quad_uniforms(quad: &Quad, global_opacity: f32, width: u32, height: u32, straight: bool, swap_rb: bool) -> [f32; 76] {
    let mut data = [0.0; 76];
    for index in 0..4 {
        data[index * 4] = quad.corners[index].0;
        data[index * 4 + 1] = quad.corners[index].1;
        data[index * 4 + 2] = quad.clip_w[index];
        data[16 + index * 4] = quad.uvs[index].0;
        data[16 + index * 4 + 1] = quad.uvs[index].1;
        data[32 + index * 4] = quad.local[index].0;
        data[32 + index * 4 + 1] = quad.local[index].1;
    }
    data[48..52].copy_from_slice(&quad.clip);
    data[52..56].copy_from_slice(&quad.round);
    data[56..60].copy_from_slice(&quad.radii);
    data[60..64].copy_from_slice(&quad.local_clip);
    data[64..68].copy_from_slice(&quad.local_radii);
    data[68] = if straight { 1.0 } else { 0.0 };
    data[69] = if swap_rb { 1.0 } else { 0.0 };
    data[70] = (quad.opacity * global_opacity).clamp(0.0, 1.0);
    data[71] = if quad.discard_outside_uv { 1.0 } else { 0.0 };
    data[72] = width as f32;
    data[73] = height as f32;
    data[74] = if quad.linear { 1.0 } else { 0.0 };
    data
}

fn upload_picture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    nearest: &wgpu::Sampler,
    linear: &wgpu::Sampler,
    width: u32,
    height: u32,
    stride: u32,
    pixels: &[u8],
    stamp: u64,
) -> Result<CachedPicture, String> {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sweet picture"),
        size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    write_picture(queue, &texture, width, height, stride, pixels);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sweet picture"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(nearest) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(linear) },
        ],
    });
    Ok(CachedPicture { stamp, width, height, texture, bind_group })
}

fn write_picture(queue: &wgpu::Queue, texture: &wgpu::Texture, width: u32, height: u32, stride: u32, pixels: &[u8]) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        pixels,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride.max(width.max(1) * 4)), rows_per_image: Some(height.max(1)) },
        wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
    );
}

fn video_nodes(document: &CoreDocument, id: NodeId, out: &mut Vec<(NodeId, String)>) {
    if document.tag_name(id).is_ok_and(|tag| tag.eq_ignore_ascii_case("video")) {
        if let Ok(Some(src)) = document.attribute(id, "src") { out.push((id, src)); }
    }
    if let Ok(children) = document.children(id) {
        for child in children { video_nodes(document, child, out); }
    }
}

fn blocks_window_drag(document: &CoreDocument, id: u32) -> bool {
    let mut current = (id != u32::MAX).then(|| node(id));
    let mut depth = 0;
    while let Some(id) = current {
        if document.tag_name(id).is_ok_and(|tag| matches!(tag.as_str(), "button" | "input" | "textarea" | "select")) {
            return true;
        }
        current = document.parent(id).ok().flatten();
        depth += 1;
        if depth > 4096 { break; }
    }
    false
}

fn video_path(src: &str) -> PathBuf {
    let path = Path::new(src);
    if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) }
}

fn start_video(document: Arc<CoreDocument>, node: NodeId, src: String, stop: Arc<AtomicBool>) {
    let path = video_path(&src);
    let _ = thread::Builder::new().name(format!("sweet-php-video-{}", node.0)).spawn(move || {
        while !stop.load(Ordering::Acquire) {
            let mut reader = match Mp4VideoReader::open(&path) {
                Ok(reader) => reader,
                Err(error) => { eprintln!("sweet PHP video {}: {error}", path.display()); return; }
            };
            // yscv-video currently reports a zero presentation timestamp for
            // frames decoded from MP4. Pace this sample at its probed 24 fps
            // instead of interpreting those zeroes as immediate frames.
            let frame_interval = Duration::from_micros(1_000_000 / 24);
            let origin = Instant::now();
            let mut frame_index = 0u64;
            loop {
                if stop.load(Ordering::Acquire) { return; }
                let frame = match reader.next_frame() {
                    Ok(Some(frame)) => frame,
                    Ok(None) => break,
                    Err(error) => { eprintln!("sweet PHP video {}: {error}", path.display()); return; }
                };
                let due = origin + frame_interval.saturating_mul(frame_index as u32);
                if let Some(mut delay) = due.checked_duration_since(Instant::now()) {
                    while !delay.is_zero() && !stop.load(Ordering::Acquire) {
                        thread::sleep(delay.min(Duration::from_millis(10)));
                        delay = due.saturating_duration_since(Instant::now());
                    }
                }
                if stop.load(Ordering::Acquire) { return; }
                let mut rgba = Vec::with_capacity(frame.width.saturating_mul(frame.height).saturating_mul(4));
                for rgb in frame.rgb8_data.chunks_exact(3) { rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]); }
                let rgba: Arc<[u8]> = Arc::from(rgba);
                if document.set_image_rgba_shared(node, frame.width as u32, frame.height as u32, (frame.width * 4) as u32, rgba).is_err() { return; }
                frame_index += 1;
            }
            // Drop the reader and reopen it at EOF. Reopening also resets the
            // decoder's reorder and drain state before the next loop.
        }
    });
}

#[no_mangle]
pub extern "C" fn sweet_window_new(document: *const PhpDocument, title: *const c_char, width: u32, height: u32, drag_anywhere: bool, resize_horizontal: bool, resize_vertical: bool, error: *mut *mut c_char) -> *mut PhpWindow {
    let title = match string(title) { Ok(value) => value, Err(e) => { report(e, error); return ptr::null_mut(); } };
    let document = unsafe { Arc::clone(&(*document).core) };
    let state = Arc::new(WindowState { closed: Mutex::new(false), wake: std::sync::Condvar::new() });
    let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
    let state_thread = Arc::clone(&state);
    let doc_thread = Arc::clone(&document);
    let hook = NEXT_WINDOW.fetch_add(1, Ordering::Relaxed);
    let events = Arc::new(Mutex::new(Vec::new()));
    let events_thread = Arc::clone(&events);
    let redraw_hook_slot = Arc::new(Mutex::new(None::<Arc<RedrawHookState>>));
    let redraw_hook_slot_thread = Arc::clone(&redraw_hook_slot);
    let thread_result = thread::Builder::new().name(format!("sweet-php-window-{hook}")).spawn(move || {
        use winit::application::ApplicationHandler;
        use winit::dpi::LogicalSize;
        use winit::event::WindowEvent;
        use winit::event::{ElementState, MouseButton};
        use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
        use winit::window::{CursorIcon, Window as NativeWindow, WindowAttributes, WindowId};
        struct App {
            title: String, width: u32, height: u32,
            doc: Arc<CoreDocument>,
            window: Option<Arc<NativeWindow>>,
            gpu: Option<GpuPresenter>, has_video: bool, drag_anywhere: bool, resizable: bool, resize_axes: ResizeAxes,
            reply: Option<std::sync::mpsc::SyncSender<Result<winit::event_loop::EventLoopProxy<WindowCommand>, String>>>,
            proxy: winit::event_loop::EventLoopProxy<WindowCommand>, redraw_hook_slot: Arc<Mutex<Option<Arc<RedrawHookState>>>>,
            events: Arc<Mutex<Vec<NativeEvent>>>, boxes: Vec<sweet_core::LayoutBox>, cursor: (f64, f64), pressed: Option<u32>,
            resize_drag: Option<ResizeDrag>, cursor_icon: CursorIcon,
            opened: Instant, animating: bool,
        }
        impl ApplicationHandler<WindowCommand> for App {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                if self.window.is_some() { return; }
                let attrs: WindowAttributes = NativeWindow::default_attributes().with_title(self.title.clone()).with_decorations(false).with_transparent(true).with_resizable(self.resizable).with_inner_size(LogicalSize::new(self.width, self.height));
                let result = (|| -> Result<(), String> {
                    let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
                    let size = window.inner_size();
                    let gpu = GpuPresenter::new(window.clone(), size.width, size.height)?;
                    self.window = Some(window.clone()); self.gpu = Some(gpu);
                    window.request_redraw(); Ok(())
                })();
                if let Some(reply) = self.reply.take() {
                    let _ = reply.send(result.map(|_| self.proxy.clone()));
                }
                if self.window.is_none() { event_loop.exit(); }
            }
            fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WindowCommand) {
                let Some(window) = self.window.as_ref() else { return; };
                match event {
                    WindowCommand::Redraw => { if let Some(hook) = self.redraw_hook_slot.lock().unwrap().as_ref() { hook.pending.store(false, Ordering::Release); } window.request_redraw(); },
                    WindowCommand::Close => { self.events.lock().unwrap().push(NativeEvent { kind: 3, ..Default::default() }); event_loop.exit(); },
                    WindowCommand::Minimize => window.set_minimized(true),
                    WindowCommand::ToggleMaximize => window.set_maximized(!window.is_maximized()),
                    WindowCommand::SetResizable(value) => {
                        self.resizable = value;
                        window.set_resizable(value);
                        if !value {
                            self.resize_drag = None;
                            self.cursor_icon = CursorIcon::Default;
                            window.set_cursor(CursorIcon::Default);
                        }
                    },
                    WindowCommand::SetResizeAxes(horizontal, vertical) => { self.resize_axes = ResizeAxes { horizontal, vertical }; self.resize_drag = None; },
                }
            }
            fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
                match event {
                    WindowEvent::CloseRequested => { self.events.lock().unwrap().push(NativeEvent { kind: 3, ..Default::default() }); event_loop.exit(); }
                    WindowEvent::Resized(size) => { let scale = self.window.as_ref().map_or(1.0, |window| window.scale_factor()); self.events.lock().unwrap().push(NativeEvent { kind: 2, width: size.width as f64 / scale, height: size.height as f64 / scale, ..Default::default() }); if let Some(window) = self.window.as_ref() { window.request_redraw(); } },
                    WindowEvent::CursorMoved { position, .. } => {
                        self.cursor = (position.x, position.y);
                        let (Some(window),) = (self.window.as_ref(),) else { return; };
                        if let Some(drag) = self.resize_drag.as_ref() { resize_borderless(window, drag, position.x, position.y); }
                        let scale = self.window.as_ref().map_or(1.0, |window| window.scale_factor()) as f32;
                        let logical_x = position.x / scale as f64;
                        let logical_y = position.y / scale as f64;
                        let id = self.boxes.iter().rev().find(|b| b.hits(logical_x as f32, logical_y as f32)).map_or(u32::MAX, |b| b.id.0);
                        let size = window.inner_size().to_logical::<f64>(window.scale_factor());
                        let resize = self.resize_drag.as_ref().map(|drag| native_resize_direction(drag.direction))
                            .or_else(|| self.resizable.then(|| resize_edge(logical_x, logical_y, size.width, size.height, 12.0, self.resize_axes)).flatten().map(native_resize_direction));
                        let cursor = resize.map(CursorIcon::from).unwrap_or_else(|| {
                            if self.boxes.iter().rev().find(|b| b.id.0 == id).is_some_and(|b| b.cursor == sweet_core::CursorStyle::Pointer) { CursorIcon::Pointer } else { CursorIcon::Default }
                        });
                        if cursor != self.cursor_icon { window.set_cursor(cursor); self.cursor_icon = cursor; }
                        self.events.lock().unwrap().push(NativeEvent { kind: 4, node_id: id, x: position.x / scale as f64, y: position.y / scale as f64, ..Default::default() });
                        window.request_redraw();
                    }
                    WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                        let scale = self.window.as_ref().map_or(1.0, |window| window.scale_factor()) as f32;
                        let id = self.boxes.iter().rev().find(|b| b.hits(self.cursor.0 as f32 / scale, self.cursor.1 as f32 / scale)).map_or(u32::MAX, |b| b.id.0);
                        let position = (self.cursor.0 / scale as f64, self.cursor.1 / scale as f64);
                        if state == ElementState::Pressed {
                            let Some(window) = self.window.as_ref() else { return; };
                            if self.resizable {
                                let size = window.inner_size().to_logical::<f64>(window.scale_factor());
                                if let Some(direction) = resize_edge(position.0, position.1, size.width, size.height, 12.0, self.resize_axes) {
                                    if self.drag_anywhere {
                                        if let Ok(start_position) = window.outer_position() {
                                            self.resize_drag = Some(ResizeDrag { direction, start_x: self.cursor.0, start_y: self.cursor.1, start_position, start_size: window.inner_size() });
                                            self.pressed = None;
                                            return;
                                        }
                                    } else if window.drag_resize_window(native_resize_direction(direction)).is_ok() {
                                        self.pressed = None;
                                        return;
                                    }
                                }
                            }
                            if self.drag_anywhere && !blocks_window_drag(&self.doc, id) {
                                if let Some(window) = self.window.as_ref() {
                                    if window.drag_window().is_ok() { self.pressed = None; return; }
                                }
                            }
                            self.pressed = (id != u32::MAX).then_some(id);
                            self.events.lock().unwrap().push(NativeEvent { kind: 5, node_id: id, x: position.0, y: position.1, ..Default::default() });
                        } else {
                            self.resize_drag = None;
                            self.events.lock().unwrap().push(NativeEvent { kind: 6, node_id: id, x: position.0, y: position.1, ..Default::default() });
                            if let Some(pressed) = self.pressed.take() { if pressed == id { self.events.lock().unwrap().push(NativeEvent { kind: 1, node_id: id, x: position.0, y: position.1, ..Default::default() }); } }
                        }
                    }
                    WindowEvent::RedrawRequested => {
                        let (Some(window), Some(gpu)) = (self.window.as_ref(), self.gpu.as_mut()) else { return; };
                        let size = window.inner_size();
                        if size.width == 0 || size.height == 0 { return; }
                        let scale = window.scale_factor() as f32;
                        let options = RenderOptions { width: size.width as f32 / scale, height: size.height as f32 / scale, scale, hover: None, focus: None, time: self.opened.elapsed().as_secs_f32() };
                        let Ok(list) = render_display_list(&self.doc, &options, 1.0, true, size.width, size.height) else { return; };
                        self.boxes = list.boxes.clone();
                        self.animating = list.animating && !self.has_video;
                        let _ = gpu.present_display(window, &list, size.width, size.height);
                    }
                    _ => {}
                }
            }
            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                if self.animating {
                    event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(16)));
                    if let Some(window) = self.window.as_ref() { window.request_redraw(); }
                } else { event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait); }
            }
        }
        let mut builder = EventLoop::<WindowCommand>::with_user_event();
        #[cfg(target_os = "linux")]
        {
            use winit::platform::x11::EventLoopBuilderExtX11;
            builder.with_any_thread(true);
        }
        let event_loop = match builder.build() {
            Ok(loop_) => loop_, Err(e) => { let _ = reply_tx.send(Err(e.to_string())); state_thread.close(); return; }
        };
        let proxy = event_loop.create_proxy();
        let video_nodes = { let mut found = Vec::new(); video_nodes(&doc_thread, NodeId(0), &mut found); found };
        let has_video = !video_nodes.is_empty();
        let app = App { title, width, height, doc: doc_thread, window: None, gpu: None, has_video, drag_anywhere, resizable: true, resize_axes: ResizeAxes { horizontal: resize_horizontal, vertical: resize_vertical }, reply: Some(reply_tx), proxy, redraw_hook_slot: redraw_hook_slot_thread, events: events_thread, boxes: Vec::new(), cursor: (0.0, 0.0), pressed: None, resize_drag: None, cursor_icon: CursorIcon::Default, opened: Instant::now(), animating: false };
        let mut app = app;
        event_loop.set_control_flow(ControlFlow::Wait);
        let _ = event_loop.run_app(&mut app);
        state_thread.close();
    });
    if let Err(problem) = thread_result { report(problem, error); return ptr::null_mut(); }
    let proxy = match reply_rx.recv_timeout(Duration::from_secs(20)) {
        Ok(Ok(proxy)) => proxy,
        Ok(Err(problem)) => { report(problem, error); return ptr::null_mut(); }
        Err(problem) => { report(problem, error); return ptr::null_mut(); }
    };
    let redraw_hook = Arc::new(RedrawHookState { proxy: proxy.clone(), pending: std::sync::atomic::AtomicBool::new(false) });
    *redraw_hook_slot.lock().unwrap() = Some(Arc::clone(&redraw_hook));
    let redraw_hook_callback = Arc::clone(&redraw_hook);
    document.set_redraw_hook(hook, Arc::new(move || {
        if !redraw_hook_callback.pending.swap(true, Ordering::AcqRel) {
            if redraw_hook_callback.proxy.send_event(WindowCommand::Redraw).is_err() { redraw_hook_callback.pending.store(false, Ordering::Release); }
        }
    }));
    let media_stop = Arc::new(AtomicBool::new(false));
    let mut videos = Vec::new();
    video_nodes(&document, NodeId(0), &mut videos);
    if let Some((node, src)) = videos.into_iter().next() { start_video(Arc::clone(&document), node, src, Arc::clone(&media_stop)); }
    Box::into_raw(Box::new(PhpWindow { proxy, state, document, hook, events, media_stop }))
}

#[no_mangle]
pub extern "C" fn sweet_window_close(window: *mut PhpWindow) { if !window.is_null() { let _ = unsafe { &*window }.proxy.send_event(WindowCommand::Close); } }
#[no_mangle]
pub extern "C" fn sweet_window_redraw(window: *mut PhpWindow) { if !window.is_null() { let _ = unsafe { &*window }.proxy.send_event(WindowCommand::Redraw); } }
#[no_mangle]
pub extern "C" fn sweet_window_minimize(window: *mut PhpWindow) { if !window.is_null() { let _ = unsafe { &*window }.proxy.send_event(WindowCommand::Minimize); } }
#[no_mangle]
pub extern "C" fn sweet_window_toggle_maximize(window: *mut PhpWindow) { if !window.is_null() { let _ = unsafe { &*window }.proxy.send_event(WindowCommand::ToggleMaximize); } }
#[no_mangle]
pub extern "C" fn sweet_window_set_resizable(window: *mut PhpWindow, value: bool) { if !window.is_null() { let _ = unsafe { &*window }.proxy.send_event(WindowCommand::SetResizable(value)); } }
#[no_mangle]
pub extern "C" fn sweet_window_set_resize_axes(window: *mut PhpWindow, horizontal: bool, vertical: bool) { if !window.is_null() { let _ = unsafe { &*window }.proxy.send_event(WindowCommand::SetResizeAxes(horizontal, vertical)); } }
#[no_mangle]
pub extern "C" fn sweet_window_is_closed(window: *const PhpWindow) -> bool { unsafe { &*window }.state.closed.lock().map(|v| *v).unwrap_or(true) }
#[no_mangle]
pub extern "C" fn sweet_window_poll_event(window: *const PhpWindow, out: *mut NativeEvent) -> bool {
    let window = unsafe { &*window }; let mut events = window.events.lock().unwrap();
    if events.is_empty() { return false; } unsafe { *out = events.remove(0); } true
}
#[no_mangle]
pub extern "C" fn sweet_window_wait_closed(window: *const PhpWindow) { let state = &unsafe { &*window }.state; let mut closed = state.closed.lock().unwrap(); while !*closed { closed = state.wake.wait(closed).unwrap(); } }
#[no_mangle]
pub extern "C" fn sweet_window_free(window: *mut PhpWindow) {
    if !window.is_null() {
        let window = unsafe { Box::from_raw(window) };
        window.media_stop.store(true, Ordering::Release);
        window.document.clear_redraw_hook(window.hook);
        let still_running = !window.state.closed.lock().map(|closed| *closed).unwrap_or(true);
        if still_running { let _ = window.proxy.send_event(WindowCommand::Close); }
    }
}

#[no_mangle]
pub extern "C" fn sweet_text_content(document: *const PhpDocument, id: u32, error: *mut *mut c_char) -> *mut c_char {
    match core(document).text_content(node(id)) {
        Ok(value) => CString::new(value).map(CString::into_raw).unwrap_or_else(|e| { report(e, error); ptr::null_mut() }),
        Err(problem) => { report(problem, error); ptr::null_mut() }
    }
}
