//! QuickJS host for sweet. JavaScript calls in through the C ABI in module.c.
//! The window loop lives on its own thread and posts pointer events back
//! through a pipe that `os.setReadHandler` drains on the QuickJS thread.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{CStr, CString};
use std::num::NonZeroU32;
#[cfg(unix)]
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

mod fields;

use sweet_core::{
    physical_size, render_xrgb, CursorStyle, Document as CoreDocument, LayoutBox, NodeId,
    RenderOptions, TextField,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{CursorIcon, Window as OsWindow, WindowId};

pub const SWEET_CLICK: u32 = 1;
pub const SWEET_MOUSEDOWN: u32 = 2;
pub const SWEET_MOUSEUP: u32 = 3;
pub const SWEET_MOUSEMOVE: u32 = 4;
pub const SWEET_INPUT: u32 = 5;
pub const SWEET_KEYDOWN: u32 = 7;
pub const SWEET_CLOSE: u32 = 8;

#[repr(C)]
pub struct SweetEvent {
    pub token: u64,
    pub doc_id: u64,
    pub x: f64,
    pub y: f64,
    pub kind: u32,
    pub has_key: u32,
    pub target: i32,
    pub listen_on: i32,
    pub key: [u8; 64],
}

pub struct DocInner {
    doc: Arc<CoreDocument>,
    id: u64,
}

struct Interest {
    map: Mutex<HashMap<u64, HashSet<String>>>,
}

struct Queue {
    events: Mutex<VecDeque<SweetEvent>>,
    /// Linux wakes QuickJS with `os.setReadHandler`. Windows QuickJS only
    /// waits on stdin, so the JS side polls `drain` from a timer instead.
    #[cfg(unix)]
    read_fd: RawFd,
    #[cfg(unix)]
    write_fd: RawFd,
}

static NEXT_DOC: AtomicU64 = AtomicU64::new(1);
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
static OPEN_WINDOWS: AtomicU32 = AtomicU32::new(0);
static INTEREST: OnceLock<Interest> = OnceLock::new();
static QUEUE: OnceLock<Queue> = OnceLock::new();
static RUNTIME: OnceLock<Result<EventLoopProxy<Cmd>, String>> = OnceLock::new();

fn interest() -> &'static Interest {
    INTEREST.get_or_init(|| Interest {
        map: Mutex::new(HashMap::new()),
    })
}

fn queue() -> &'static Queue {
    QUEUE.get_or_init(|| {
        #[cfg(unix)]
        {
            let mut fds = [0; 2];
            let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
            if rc != 0 {
                panic!("pipe failed");
            }
            for fd in fds {
                unsafe {
                    let flags = libc::fcntl(fd, libc::F_GETFL);
                    libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
                }
            }
            Queue {
                events: Mutex::new(VecDeque::new()),
                read_fd: fds[0],
                write_fd: fds[1],
            }
        }
        #[cfg(not(unix))]
        {
            Queue {
                events: Mutex::new(VecDeque::new()),
            }
        }
    })
}

fn set_err(err: *mut libc::c_char, err_len: usize, message: &str) {
    if err.is_null() || err_len == 0 {
        return;
    }
    let bytes = message.as_bytes();
    let n = bytes.len().min(err_len - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), err as *mut u8, n);
        *err.add(n) = 0;
    }
}

fn cstr<'a>(ptr: *const libc::c_char) -> Result<&'a str, String> {
    if ptr.is_null() {
        return Err("missing string".into());
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| "string is not utf-8".into())
}

fn owned_string(text: String) -> *mut libc::c_char {
    match CString::new(text) {
        Ok(value) => value.into_raw(),
        Err(_) => CString::new("").unwrap().into_raw(),
    }
}

fn doc_ref<'a>(doc: *mut DocInner) -> Result<&'a DocInner, String> {
    if doc.is_null() {
        Err("missing document".into())
    } else {
        Ok(unsafe { &*doc })
    }
}

fn push_event(event: SweetEvent) {
    let q = queue();
    let mut events = q.events.lock().expect("event queue");
    let wake = events.is_empty();
    events.push_back(event);
    drop(events);
    #[cfg(unix)]
    if wake {
        let byte = [1u8];
        unsafe {
            libc::write(q.write_fd, byte.as_ptr() as *const libc::c_void, 1);
        }
    }
    #[cfg(not(unix))]
    let _ = (q, wake);
}

fn wants(doc_id: u64, event: &str) -> bool {
    interest()
        .map
        .lock()
        .expect("interest")
        .get(&doc_id)
        .is_some_and(|names| names.contains(event))
}

fn enqueue_pointer(live: &Live, kind: u32, target: Option<NodeId>, x: f64, y: f64) {
    if kind == SWEET_MOUSEMOVE && !wants(live.doc_id, "mousemove") {
        return;
    }
    let mut chain = Vec::new();
    let mut current = target;
    let mut guard = 0usize;
    while let Some(id) = current {
        chain.push(id.0 as i32);
        current = live
            .boxes
            .iter()
            .rev()
            .find(|b| b.id == id)
            .and_then(|b| b.parent);
        guard += 1;
        if guard > 4096 {
            break;
        }
    }
    chain.push(-1);
    let hit = target.map(|id| id.0 as i32).unwrap_or(-1);
    for listen_on in chain {
        push_event(SweetEvent {
            token: live.token,
            doc_id: live.doc_id,
            x,
            y,
            kind,
            has_key: 0,
            target: hit,
            listen_on,
            key: [0; 64],
        });
    }
}

fn enqueue_node(live: &Live, kind: u32, target: NodeId, key: Option<&str>) {
    let mut key_bytes = [0u8; 64];
    let has_key = if let Some(key) = key {
        let bytes = key.as_bytes();
        let n = bytes.len().min(63);
        key_bytes[..n].copy_from_slice(&bytes[..n]);
        1
    } else {
        0
    };
    let mut chain = Vec::new();
    let mut current = Some(target);
    let mut guard = 0usize;
    while let Some(id) = current {
        chain.push(id.0 as i32);
        current = live
            .boxes
            .iter()
            .rev()
            .find(|b| b.id == id)
            .and_then(|b| b.parent);
        guard += 1;
        if guard > 4096 {
            break;
        }
    }
    chain.push(-1);
    let hit = target.0 as i32;
    for listen_on in chain {
        push_event(SweetEvent {
            token: live.token,
            doc_id: live.doc_id,
            x: 0.0,
            y: 0.0,
            kind,
            has_key,
            target: hit,
            listen_on,
            key: key_bytes,
        });
    }
}

struct OpenArgs {
    token: u64,
    reply: mpsc::Sender<Result<(), String>>,
    doc: Arc<CoreDocument>,
    doc_id: u64,
    title: String,
    width: f64,
    height: f64,
}

enum Cmd {
    Open(OpenArgs),
    Redraw(u64),
    Close(u64),
}

struct Live {
    token: u64,
    doc: Arc<CoreDocument>,
    doc_id: u64,
    opened: Instant,
    #[allow(dead_code)]
    context: softbuffer::Context<winit::event_loop::OwnedDisplayHandle>,
    surface: softbuffer::Surface<winit::event_loop::OwnedDisplayHandle, OsWindow>,
    boxes: Vec<LayoutBox>,
    hover: Option<NodeId>,
    focus: Option<NodeId>,
    pressed: Option<NodeId>,
    cursor: CursorStyle,
    last_x: f64,
    last_y: f64,
    redraw_queued: Arc<AtomicBool>,
    focus_frame: bool,
    painted: bool,
    fields: Option<fields::Fields>,
    field_scratch: Vec<u32>,
    field_stamp: u64,
    field_until: Option<Instant>,
    repaint_at: Option<Instant>,
}

struct App {
    proxy: EventLoopProxy<Cmd>,
    windows: HashMap<WindowId, Live>,
    tokens: HashMap<u64, WindowId>,
}

fn runtime() -> Result<EventLoopProxy<Cmd>, String> {
    let slot = RUNTIME.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel(1);
        let spawned = std::thread::Builder::new()
            .name("sweet-window".into())
            .spawn(move || {
                let mut builder = EventLoop::<Cmd>::with_user_event();
                #[cfg(target_os = "linux")]
                {
                    use winit::platform::x11::EventLoopBuilderExtX11;
                    builder.with_any_thread(true);
                }
                #[cfg(target_os = "windows")]
                {
                    use winit::platform::windows::EventLoopBuilderExtWindows;
                    builder.with_any_thread(true);
                }
                let event_loop = match builder.build() {
                    Ok(event_loop) => event_loop,
                    Err(err) => {
                        let _ = tx.send(Err(err.to_string()));
                        return;
                    }
                };
                let proxy = event_loop.create_proxy();
                if tx.send(Ok(proxy.clone())).is_err() {
                    return;
                }
                let mut app = App {
                    proxy,
                    windows: HashMap::new(),
                    tokens: HashMap::new(),
                };
                let _ = event_loop.run_app(&mut app);
            });
        if let Err(err) = spawned {
            return Err(err.to_string());
        }
        match rx.recv() {
            Ok(result) => result,
            Err(_) => Err("sweet window thread exited before the event loop started".into()),
        }
    });
    match slot {
        Ok(proxy) => Ok(proxy.clone()),
        Err(message) => Err(message.clone()),
    }
}

impl ApplicationHandler<Cmd> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Cmd) {
        match event {
            Cmd::Open(open) => self.open(event_loop, open),
            Cmd::Redraw(token) => {
                if let Some(id) = self.tokens.get(&token).copied() {
                    if let Some(live) = self.windows.get_mut(&id) {
                        live.redraw_queued.store(false, Ordering::Release);
                        live.surface.window().request_redraw();
                    }
                }
            }
            Cmd::Close(token) => {
                if let Some(id) = self.tokens.get(&token).copied() {
                    self.destroy(id);
                }
            }
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if matches!(event, WindowEvent::CloseRequested) {
            self.destroy(window_id);
            return;
        }
        let Some(live) = self.windows.get_mut(&window_id) else {
            return;
        };
        if let Some(editor) = live.fields.as_mut() {
            let repaint = editor.on_event(live.surface.window(), &event);
            if repaint {
                live.surface.window().request_redraw();
            }
        }
        match event {
            WindowEvent::RedrawRequested => {
                live.painted = true;
                let _ = paint(live);
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                live.surface.window().request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y, hit) = pointer(live, position.x, position.y);
                live.last_x = x;
                live.last_y = y;
                let cursor = box_cursor(&live.boxes, hit);
                if cursor != live.cursor {
                    live.cursor = cursor;
                    let icon = match cursor {
                        CursorStyle::Pointer => CursorIcon::Pointer,
                        CursorStyle::Default => CursorIcon::Default,
                    };
                    live.surface.window().set_cursor(icon);
                }
                if hit != live.hover {
                    live.hover = hit;
                    live.surface.window().request_redraw();
                }
                enqueue_pointer(live, SWEET_MOUSEMOVE, hit, x, y);
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let (x, y, hit) = at_pointer(live);
                match state {
                    ElementState::Pressed => {
                        if live.focus != hit {
                            live.focus_frame = true;
                        }
                        live.pressed = hit;
                        live.focus = hit;
                        enqueue_pointer(live, SWEET_MOUSEDOWN, hit, x, y);
                    }
                    ElementState::Released => {
                        enqueue_pointer(live, SWEET_MOUSEUP, hit, x, y);
                        if hit == live.pressed {
                            enqueue_pointer(live, SWEET_CLICK, hit, x, y);
                        }
                        live.pressed = None;
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let mut soonest: Option<Instant> = None;
        for live in self.windows.values_mut() {
            if let Some(at) = live.repaint_at {
                if Instant::now() >= at {
                    live.surface.window().request_redraw();
                    live.repaint_at = None;
                } else {
                    soonest = Some(soonest.map_or(at, |soon| soon.min(at)));
                }
            }
            if live.focus_frame && !live.painted {
                live.surface.window().request_redraw();
            }
            live.focus_frame = false;
            live.painted = false;
        }
        if let Some(at) = soonest {
            event_loop.set_control_flow(ControlFlow::WaitUntil(at));
        }
    }
}

impl App {
    fn open(&mut self, event_loop: &ActiveEventLoop, open: OpenArgs) {
        let OpenArgs {
            token,
            reply,
            doc,
            doc_id,
            title,
            width,
            height,
        } = open;
        let outcome = (|| -> Result<(), String> {
            let attrs = OsWindow::default_attributes()
                .with_title(title)
                .with_inner_size(LogicalSize::new(width, height));
            let window = event_loop
                .create_window(attrs)
                .map_err(|err| err.to_string())?;
            let id = window.id();
            let context = softbuffer::Context::new(event_loop.owned_display_handle())
                .map_err(|err| err.to_string())?;
            let surface =
                softbuffer::Surface::new(&context, window).map_err(|err| err.to_string())?;
            let redraw_queued = Arc::new(AtomicBool::new(false));
            let queued = Arc::clone(&redraw_queued);
            let hook_proxy = self.proxy.clone();
            doc.set_redraw_hook(
                token,
                Arc::new(move || {
                    if queued.swap(true, Ordering::AcqRel) {
                        return;
                    }
                    let _ = hook_proxy.send_event(Cmd::Redraw(token));
                }),
            );
            let mut live = Live {
                token,
                doc,
                doc_id,
                opened: Instant::now(),
                context,
                surface,
                boxes: Vec::new(),
                hover: None,
                focus: None,
                pressed: None,
                cursor: CursorStyle::Default,
                last_x: 0.0,
                last_y: 0.0,
                redraw_queued,
                focus_frame: false,
                painted: false,
                fields: None,
                field_scratch: Vec::new(),
                field_stamp: 0,
                field_until: None,
                repaint_at: None,
            };
            live.fields = Some(fields::Fields::new(live.surface.window()));
            if let Err(err) = paint(&mut live) {
                live.doc.clear_redraw_hook(token);
                return Err(err);
            }
            self.tokens.insert(token, id);
            self.windows.insert(id, live);
            OPEN_WINDOWS.fetch_add(1, Ordering::Release);
            Ok(())
        })();
        let _ = reply.send(outcome);
    }

    fn destroy(&mut self, id: WindowId) {
        let Some(live) = self.windows.remove(&id) else {
            return;
        };
        self.tokens.remove(&live.token);
        live.doc.clear_redraw_hook(live.token);
        OPEN_WINDOWS.fetch_sub(1, Ordering::Release);
        push_event(SweetEvent {
            token: live.token,
            doc_id: live.doc_id,
            x: 0.0,
            y: 0.0,
            kind: SWEET_CLOSE,
            has_key: 0,
            target: -1,
            listen_on: -1,
            key: [0; 64],
        });
    }
}

fn paint(live: &mut Live) -> Result<(), String> {
    let (scale, physical) = {
        let window = live.surface.window();
        (window.scale_factor().max(0.01), window.inner_size())
    };
    if physical.width == 0 || physical.height == 0 {
        return Ok(());
    }
    let options = RenderOptions {
        width: physical.width as f32 / scale as f32,
        height: physical.height as f32 / scale as f32,
        scale: scale as f32,
        hover: live.hover,
        focus: live.focus,
        time: frame_clock(live.opened, Instant::now()),
    };
    let (frame_w, frame_h) = physical_size(&options);
    let width = NonZeroU32::new(frame_w).ok_or_else(|| "empty frame".to_string())?;
    let height = NonZeroU32::new(frame_h).ok_or_else(|| "empty frame".to_string())?;
    live.surface
        .resize(width, height)
        .map_err(|err| err.to_string())?;
    let mut buffer = live.surface.buffer_mut().map_err(|err| err.to_string())?;
    let (boxes, text_fields, animating) =
        render_xrgb(&live.doc, &options, &mut buffer).map_err(|err| err.0)?;
    live.boxes = boxes;
    let pixels = (frame_w as usize).saturating_mul(frame_h as usize);
    if live.field_scratch.len() != pixels {
        live.field_scratch = vec![0u32; pixels];
        live.field_stamp = 0;
    }
    let stamp = field_stamp(&text_fields, live.focus);
    let rerun_fields = stamp != live.field_stamp
        || live.field_until.is_some_and(|at| Instant::now() >= at);
    let mut repaint_at = if animating {
        Some(next_frame(live.opened, Instant::now()))
    } else {
        live.field_until
    };
    let updates = if rerun_fields {
        drop(buffer);
        let updates = if let Some(editor) = live.fields.as_mut() {
            live.field_scratch.fill(0);
            let updates = editor.run(
                live.surface.window(),
                &text_fields,
                live.focus,
                &mut live.field_scratch,
                frame_w,
                frame_h,
            );
            let delay = editor.repaint_after;
            live.field_stamp = stamp;
            live.field_until = delay.and_then(|delay| {
                if delay < Duration::from_secs(2) {
                    Some(Instant::now() + delay)
                } else {
                    None
                }
            });
            updates
        } else {
            Vec::new()
        };
        if let Some(at) = live.field_until {
            repaint_at = Some(repaint_at.map_or(at, |soon| soon.min(at)));
        }
        let mut buffer = live.surface.buffer_mut().map_err(|err| err.to_string())?;
        blend_fields(&mut buffer, &live.field_scratch, &text_fields, frame_w, frame_h);
        buffer.present().map_err(|err| err.to_string())?;
        updates
    } else {
        blend_fields(&mut buffer, &live.field_scratch, &text_fields, frame_w, frame_h);
        buffer.present().map_err(|err| err.to_string())?;
        Vec::new()
    };
    live.repaint_at = repaint_at;
    for update in updates {
        let previous = text_fields
            .iter()
            .find(|field| field.id == update.id)
            .map(|field| field.value.as_str())
            .unwrap_or("");
        if update.value != previous {
            let _ = live.doc.set_control_value(update.id, &update.value);
            enqueue_node(live, SWEET_INPUT, update.id, None);
        }
        if update.submit {
            enqueue_node(live, SWEET_KEYDOWN, update.id, Some("Enter"));
        }
    }
    Ok(())
}

const FRAME_NS: u64 = 1_000_000_000 / 60;

fn frame_index(opened: Instant, now: Instant) -> u64 {
    now.saturating_duration_since(opened).as_nanos() as u64 / FRAME_NS
}

fn frame_clock(opened: Instant, now: Instant) -> f32 {
    frame_index(opened, now) as f32 / 60.0
}

fn next_frame(opened: Instant, now: Instant) -> Instant {
    let frame = frame_index(opened, now);
    opened + Duration::from_nanos(frame.saturating_add(1).saturating_mul(FRAME_NS))
}

fn field_stamp(fields: &[TextField], focus: Option<NodeId>) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    hash ^= focus.map(|id| id.0 as u64).unwrap_or(0);
    hash = hash.wrapping_mul(0x100000001b3);
    for field in fields {
        for value in [
            field.id.0 as u64,
            field.x.to_bits() as u64,
            field.y.to_bits() as u64,
            field.width.to_bits() as u64,
            field.height.to_bits() as u64,
        ] {
            hash ^= value;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        for byte in field.value.bytes().chain(field.placeholder.bytes()) {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

fn blend_fields(buffer: &mut [u32], scratch: &[u32], fields: &[TextField], width: u32, height: u32) {
    for field in fields {
        let x0 = (field.clip_x.floor() as i32).max(0) as u32;
        let y0 = (field.clip_y.floor() as i32).max(0) as u32;
        let x1 = ((field.clip_x + field.clip_w).ceil() as i32)
            .max(0)
            .min(width as i32) as u32;
        let y1 = ((field.clip_y + field.clip_h).ceil() as i32)
            .max(0)
            .min(height as i32) as u32;
        for y in y0..y1 {
            for x in x0..x1 {
                let index = (y * width + x) as usize;
                if scratch.get(index).is_some_and(|pixel| pixel >> 24 != 0) {
                    buffer[index] = fields::onto_screen(buffer[index], scratch[index]);
                }
            }
        }
    }
}

fn pointer(live: &Live, physical_x: f64, physical_y: f64) -> (f64, f64, Option<NodeId>) {
    let scale = live.surface.window().scale_factor().max(0.01);
    let x = physical_x / scale;
    let y = physical_y / scale;
    let hit = live.boxes.iter().rev().find(|b| b.hits(x as f32, y as f32)).map(|b| b.id);
    (x, y, hit)
}

fn at_pointer(live: &Live) -> (f64, f64, Option<NodeId>) {
    let hit = live
        .boxes
        .iter()
        .rev()
        .find(|b| b.hits(live.last_x as f32, live.last_y as f32))
        .map(|b| b.id);
    (live.last_x, live.last_y, hit)
}

fn box_cursor(boxes: &[LayoutBox], hit: Option<NodeId>) -> CursorStyle {
    hit.and_then(|id| boxes.iter().rev().find(|b| b.id == id))
        .map(|b| b.cursor)
        .unwrap_or(CursorStyle::Default)
}

#[no_mangle]
pub unsafe extern "C" fn sweet_doc_new(
    err: *mut libc::c_char,
    err_len: usize,
) -> *mut DocInner {
    match CoreDocument::new() {
        Ok(doc) => Box::into_raw(Box::new(DocInner {
            doc: Arc::new(doc),
            id: NEXT_DOC.fetch_add(1, Ordering::Relaxed),
        })),
        Err(error) => {
            set_err(err, err_len, &error.0);
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_doc_clone(doc: *mut DocInner) -> *mut DocInner {
    if doc.is_null() {
        return std::ptr::null_mut();
    }
    let inner = &*doc;
    Box::into_raw(Box::new(DocInner {
        doc: Arc::clone(&inner.doc),
        id: inner.id,
    }))
}

#[no_mangle]
pub unsafe extern "C" fn sweet_doc_free(doc: *mut DocInner) {
    if !doc.is_null() {
        drop(Box::from_raw(doc));
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_doc_id(doc: *mut DocInner) -> u64 {
    if doc.is_null() {
        0
    } else {
        (*doc).id
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_doc_same(a: *mut DocInner, b: *mut DocInner) -> i32 {
    if a.is_null() || b.is_null() {
        return 0;
    }
    i32::from(Arc::ptr_eq(&(*a).doc, &(*b).doc))
}

#[no_mangle]
pub unsafe extern "C" fn sweet_string_free(ptr: *mut libc::c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr));
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_bytes_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        drop(Vec::from_raw_parts(ptr, len, len));
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_u32_free(ptr: *mut u32, len: usize) {
    if !ptr.is_null() {
        drop(Vec::from_raw_parts(ptr, len, len));
    }
}

fn take_vec<T>(values: Vec<T>, out_ptr: *mut *mut T, out_len: *mut usize) {
    let boxed = values.into_boxed_slice();
    let len = boxed.len();
    unsafe {
        *out_ptr = Box::into_raw(boxed) as *mut T;
        *out_len = len;
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_body(doc: *mut DocInner) -> u32 {
    match doc_ref(doc) {
        Ok(inner) => inner.doc.body().0,
        Err(_) => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_dark(doc: *mut DocInner) -> i32 {
    match doc_ref(doc) {
        Ok(inner) => i32::from(inner.doc.dark()),
        Err(_) => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_set_dark(doc: *mut DocInner, dark: i32) {
    if let Ok(inner) = doc_ref(doc) {
        inner.doc.set_dark(dark != 0);
    }
}

fn node_string(
    doc: *mut DocInner,
    id: u32,
    err: *mut libc::c_char,
    err_len: usize,
    read: impl FnOnce(&CoreDocument, NodeId) -> Result<String, sweet_core::SweetError>,
) -> *mut libc::c_char {
    let inner = match doc_ref(doc) {
        Ok(inner) => inner,
        Err(message) => {
            set_err(err, err_len, &message);
            return std::ptr::null_mut();
        }
    };
    match read(&inner.doc, NodeId(id)) {
        Ok(text) => owned_string(text),
        Err(error) => {
            set_err(err, err_len, &error.0);
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_tag_name(
    doc: *mut DocInner,
    id: u32,
    err: *mut libc::c_char,
    err_len: usize,
) -> *mut libc::c_char {
    node_string(doc, id, err, err_len, |doc, id| doc.tag_name(id))
}

#[no_mangle]
pub unsafe extern "C" fn sweet_class_name(
    doc: *mut DocInner,
    id: u32,
    err: *mut libc::c_char,
    err_len: usize,
) -> *mut libc::c_char {
    node_string(doc, id, err, err_len, |doc, id| doc.class_name(id))
}

#[no_mangle]
pub unsafe extern "C" fn sweet_text_content(
    doc: *mut DocInner,
    id: u32,
    err: *mut libc::c_char,
    err_len: usize,
) -> *mut libc::c_char {
    node_string(doc, id, err, err_len, |doc, id| doc.text_content(id))
}

#[no_mangle]
pub unsafe extern "C" fn sweet_id_attr(
    doc: *mut DocInner,
    id: u32,
    err: *mut libc::c_char,
    err_len: usize,
) -> *mut libc::c_char {
    node_string(doc, id, err, err_len, |doc, id| doc.id_attr(id))
}

#[no_mangle]
pub unsafe extern "C" fn sweet_control_value(
    doc: *mut DocInner,
    id: u32,
    err: *mut libc::c_char,
    err_len: usize,
) -> *mut libc::c_char {
    node_string(doc, id, err, err_len, |doc, id| doc.control_value(id))
}

#[no_mangle]
pub unsafe extern "C" fn sweet_node_type(
    doc: *mut DocInner,
    id: u32,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let inner = match doc_ref(doc) {
        Ok(inner) => inner,
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    match inner.doc.node_type(NodeId(id)) {
        Ok(kind) => kind as i32,
        Err(error) => {
            set_err(err, err_len, &error.0);
            -1
        }
    }
}

fn set_node(
    doc: *mut DocInner,
    id: u32,
    err: *mut libc::c_char,
    err_len: usize,
    write: impl FnOnce(&CoreDocument, NodeId) -> Result<(), sweet_core::SweetError>,
) -> i32 {
    let inner = match doc_ref(doc) {
        Ok(inner) => inner,
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    match write(&inner.doc, NodeId(id)) {
        Ok(()) => 0,
        Err(error) => {
            set_err(err, err_len, &error.0);
            -1
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_set_class_name(
    doc: *mut DocInner,
    id: u32,
    value: *const libc::c_char,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let value = match cstr(value) {
        Ok(value) => value.to_string(),
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    set_node(doc, id, err, err_len, move |doc, id| {
        doc.set_class_name(id, &value)
    })
}

#[no_mangle]
pub unsafe extern "C" fn sweet_set_text_content(
    doc: *mut DocInner,
    id: u32,
    value: *const libc::c_char,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let value = match cstr(value) {
        Ok(value) => value.to_string(),
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    set_node(doc, id, err, err_len, move |doc, id| {
        doc.set_text_content(id, &value)
    })
}

#[no_mangle]
pub unsafe extern "C" fn sweet_set_id_attr(
    doc: *mut DocInner,
    id: u32,
    value: *const libc::c_char,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let value = match cstr(value) {
        Ok(value) => value.to_string(),
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    set_node(doc, id, err, err_len, move |doc, id| doc.set_id_attr(id, &value))
}

#[no_mangle]
pub unsafe extern "C" fn sweet_set_control_value(
    doc: *mut DocInner,
    id: u32,
    value: *const libc::c_char,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let value = match cstr(value) {
        Ok(value) => value.to_string(),
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    set_node(doc, id, err, err_len, move |doc, id| {
        doc.set_control_value(id, &value)
    })
}

#[no_mangle]
pub unsafe extern "C" fn sweet_disabled(
    doc: *mut DocInner,
    id: u32,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let inner = match doc_ref(doc) {
        Ok(inner) => inner,
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    match inner.doc.is_disabled(NodeId(id)) {
        Ok(disabled) => i32::from(disabled),
        Err(error) => {
            set_err(err, err_len, &error.0);
            -1
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_set_disabled(
    doc: *mut DocInner,
    id: u32,
    disabled: i32,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    set_node(doc, id, err, err_len, move |doc, id| {
        doc.set_disabled(id, disabled != 0)
    })
}

#[no_mangle]
pub unsafe extern "C" fn sweet_create_element(
    doc: *mut DocInner,
    tag: *const libc::c_char,
    out_id: *mut u32,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let tag = match cstr(tag) {
        Ok(tag) => tag.to_string(),
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    let inner = match doc_ref(doc) {
        Ok(inner) => inner,
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    match inner.doc.create_element(&tag) {
        Ok(id) => {
            *out_id = id.0;
            0
        }
        Err(error) => {
            set_err(err, err_len, &error.0);
            -1
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_create_text(
    doc: *mut DocInner,
    text: *const libc::c_char,
    out_id: *mut u32,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let text = match cstr(text) {
        Ok(text) => text.to_string(),
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    let inner = match doc_ref(doc) {
        Ok(inner) => inner,
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    *out_id = inner.doc.create_text(&text).0;
    0
}

#[no_mangle]
pub unsafe extern "C" fn sweet_create_comment(
    doc: *mut DocInner,
    text: *const libc::c_char,
    out_id: *mut u32,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let text = match cstr(text) {
        Ok(text) => text.to_string(),
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    let inner = match doc_ref(doc) {
        Ok(inner) => inner,
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    *out_id = inner.doc.create_comment(&text).0;
    0
}

#[no_mangle]
pub unsafe extern "C" fn sweet_set_attribute(
    doc: *mut DocInner,
    id: u32,
    name: *const libc::c_char,
    value: *const libc::c_char,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let name = match cstr(name) {
        Ok(name) => name.to_string(),
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    let value = match cstr(value) {
        Ok(value) => value.to_string(),
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    set_node(doc, id, err, err_len, move |doc, id| {
        doc.set_attribute(id, &name, &value)
    })
}

#[no_mangle]
pub unsafe extern "C" fn sweet_get_attribute(
    doc: *mut DocInner,
    id: u32,
    name: *const libc::c_char,
    out: *mut *mut libc::c_char,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let name = match cstr(name) {
        Ok(name) => name.to_string(),
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    let inner = match doc_ref(doc) {
        Ok(inner) => inner,
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    match inner.doc.attribute(NodeId(id), &name) {
        Ok(Some(value)) => {
            *out = owned_string(value);
            1
        }
        Ok(None) => 0,
        Err(error) => {
            set_err(err, err_len, &error.0);
            -1
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_remove_attribute(
    doc: *mut DocInner,
    id: u32,
    name: *const libc::c_char,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let name = match cstr(name) {
        Ok(name) => name.to_string(),
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    set_node(doc, id, err, err_len, move |doc, id| {
        doc.remove_attribute(id, &name)
    })
}

#[no_mangle]
pub unsafe extern "C" fn sweet_set_image(
    doc: *mut DocInner,
    id: u32,
    bytes: *const u8,
    len: usize,
    out_w: *mut u32,
    out_h: *mut u32,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    if bytes.is_null() && len != 0 {
        set_err(err, err_len, "missing image bytes");
        return -1;
    }
    let slice = unsafe { std::slice::from_raw_parts(bytes, len) };
    let inner = match doc_ref(doc) {
        Ok(inner) => inner,
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    match inner.doc.set_image_encoded(NodeId(id), slice) {
        Ok((w, h)) => {
            *out_w = w;
            *out_h = h;
            0
        }
        Err(error) => {
            set_err(err, err_len, &error.0);
            -1
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_clear_image(
    doc: *mut DocInner,
    id: u32,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    set_node(doc, id, err, err_len, |doc, id| doc.clear_image(id))
}

#[no_mangle]
pub unsafe extern "C" fn sweet_insert_before(
    doc: *mut DocInner,
    parent: u32,
    child: u32,
    has_before: i32,
    before: u32,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let reference = if has_before != 0 {
        Some(NodeId(before))
    } else {
        None
    };
    set_node(doc, parent, err, err_len, move |doc, parent| {
        doc.insert_before(parent, NodeId(child), reference)
    })
}

#[no_mangle]
pub unsafe extern "C" fn sweet_remove_child(
    doc: *mut DocInner,
    parent: u32,
    child: u32,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    set_node(doc, parent, err, err_len, move |doc, parent| {
        doc.remove_child(parent, NodeId(child))
    })
}

#[no_mangle]
pub unsafe extern "C" fn sweet_children(
    doc: *mut DocInner,
    id: u32,
    out_ptr: *mut *mut u32,
    out_len: *mut usize,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let inner = match doc_ref(doc) {
        Ok(inner) => inner,
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    match inner.doc.children(NodeId(id)) {
        Ok(ids) => {
            let values = ids.into_iter().map(|id| id.0).collect::<Vec<_>>();
            take_vec(values, out_ptr, out_len);
            0
        }
        Err(error) => {
            set_err(err, err_len, &error.0);
            -1
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_parent(
    doc: *mut DocInner,
    id: u32,
    out_id: *mut u32,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let inner = match doc_ref(doc) {
        Ok(inner) => inner,
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    match inner.doc.parent(NodeId(id)) {
        Ok(Some(parent)) => {
            *out_id = parent.0;
            1
        }
        Ok(None) => 0,
        Err(error) => {
            set_err(err, err_len, &error.0);
            -1
        }
    }
}

#[repr(C)]
pub struct SweetBox {
    pub id: u32,
    pub parent: i32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[no_mangle]
pub unsafe extern "C" fn sweet_render_png(
    doc: *mut DocInner,
    width: u32,
    height: u32,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let inner = match doc_ref(doc) {
        Ok(inner) => inner,
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    match inner.doc.render_png(width, height) {
        Ok(bytes) => {
            let boxed = bytes.into_boxed_slice();
            let len = boxed.len();
            *out_ptr = Box::into_raw(boxed) as *mut u8;
            *out_len = len;
            0
        }
        Err(error) => {
            set_err(err, err_len, &error.0);
            -1
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_layout_boxes(
    doc: *mut DocInner,
    width: f64,
    height: f64,
    out_ptr: *mut *mut SweetBox,
    out_len: *mut usize,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let inner = match doc_ref(doc) {
        Ok(inner) => inner,
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    let frame = match inner.doc.render(&RenderOptions {
        width: width as f32,
        height: height as f32,
        scale: 1.0,
        hover: None,
        focus: None,
        time: 0.0,
    }) {
        Ok(frame) => frame,
        Err(error) => {
            set_err(err, err_len, &error.0);
            return -1;
        }
    };
    let boxes = frame
        .boxes
        .into_iter()
        .map(|b| SweetBox {
            id: b.id.0,
            parent: b.parent.map(|id| id.0 as i32).unwrap_or(-1),
            x: b.x as f64,
            y: b.y as f64,
            width: b.width as f64,
            height: b.height as f64,
        })
        .collect::<Vec<_>>();
    let boxed = boxes.into_boxed_slice();
    let len = boxed.len();
    *out_ptr = Box::into_raw(boxed) as *mut SweetBox;
    *out_len = len;
    0
}

#[no_mangle]
pub unsafe extern "C" fn sweet_box_free(ptr: *mut SweetBox, len: usize) {
    if !ptr.is_null() {
        drop(Vec::from_raw_parts(ptr, len, len));
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_listen(doc: *mut DocInner, event: *const libc::c_char) {
    let Ok(inner) = doc_ref(doc) else {
        return;
    };
    let Ok(event) = cstr(event) else {
        return;
    };
    interest()
        .map
        .lock()
        .expect("interest")
        .entry(inner.id)
        .or_default()
        .insert(event.to_string());
}

#[no_mangle]
pub unsafe extern "C" fn sweet_window_open(
    doc: *mut DocInner,
    title: *const libc::c_char,
    width: f64,
    height: f64,
    out_token: *mut u64,
    err: *mut libc::c_char,
    err_len: usize,
) -> i32 {
    let inner = match doc_ref(doc) {
        Ok(inner) => inner,
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    let title = match cstr(title) {
        Ok(title) => title.to_string(),
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    let proxy = match runtime() {
        Ok(proxy) => proxy,
        Err(message) => {
            set_err(err, err_len, &message);
            return -1;
        }
    };
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    let (reply_tx, reply_rx) = mpsc::channel();
    if proxy
        .send_event(Cmd::Open(OpenArgs {
            token,
            reply: reply_tx,
            doc: Arc::clone(&inner.doc),
            doc_id: inner.id,
            title,
            width: width.max(1.0),
            height: height.max(1.0),
        }))
        .is_err()
    {
        set_err(err, err_len, "sweet window thread is not running");
        return -1;
    }
    match reply_rx.recv() {
        Ok(Ok(())) => {
            *out_token = token;
            0
        }
        Ok(Err(message)) => {
            set_err(err, err_len, &message);
            -1
        }
        Err(_) => {
            set_err(
                err,
                err_len,
                "sweet window thread exited before the window opened",
            );
            -1
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_window_close(token: u64) {
    if let Ok(proxy) = runtime() {
        let _ = proxy.send_event(Cmd::Close(token));
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_windows_alive() -> i32 {
    OPEN_WINDOWS.load(Ordering::Acquire) as i32
}

#[no_mangle]
pub unsafe extern "C" fn sweet_notify_fd() -> i32 {
    #[cfg(unix)]
    {
        queue().read_fd
    }
    #[cfg(not(unix))]
    {
        -1
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_kick() {
    #[cfg(unix)]
    {
        let fd = queue().read_fd;
        let mut buf = [0u8; 64];
        loop {
            let n = libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len());
            if n <= 0 {
                break;
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sweet_poll(out: *mut SweetEvent) -> i32 {
    if out.is_null() {
        return 0;
    }
    match queue().events.lock().expect("event queue").pop_front() {
        Some(event) => {
            *out = event;
            1
        }
        None => 0,
    }
}

#[repr(C)]
pub struct JSContext {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct JSModuleDef {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    fn sweet_js_init_module(ctx: *mut JSContext, name: *const libc::c_char) -> *mut JSModuleDef;
}

#[no_mangle]
pub unsafe extern "C" fn js_init_module(
    ctx: *mut JSContext,
    name: *const libc::c_char,
) -> *mut JSModuleDef {
    sweet_js_init_module(ctx, name)
}
