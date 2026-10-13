//! In-process Node binding. JavaScript mutates the DOM through these methods.
//! The renderer has no JavaScript engine of its own.
//!
//! A thread in this process runs the window loop. DOM calls stay on whatever
//! thread Node uses and wake that loop with a redraw command.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use napi::bindgen_prelude::{AsyncTask, Buffer, Function};
use napi::threadsafe_function::{
    ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use napi::{Status, Task};
use napi_derive::napi;
use sweet_core::{
    physical_size, render_argb, render_xrgb, CursorStyle, Document as CoreDocument, LayoutBox,
    NodeId, RenderOptions, SweetError, TextField,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{
    ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy, OwnedDisplayHandle,
};
use winit::window::{CursorIcon, ResizeDirection, Window as OsWindow, WindowId};

mod fields;

type ListenerFn = ThreadsafeFunction<UiEvent, UnknownReturnValue, UiEvent, Status, false, true>;

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
static RUNTIME: OnceLock<Result<EventLoopProxy<Cmd>, String>> = OnceLock::new();

#[napi(object)]
pub struct UiEvent {
    #[napi(js_name = "type")]
    pub kind: String,
    pub node_id: Option<u32>,
    pub x: f64,
    pub y: f64,
    pub key: Option<String>,
}

#[napi(object)]
pub struct ImageSize {
    pub width: u32,
    pub height: u32,
}

#[napi(object)]
pub struct LayoutInfo {
    pub id: u32,
    pub parent: Option<u32>,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[napi(object)]
pub struct WindowOptions {
    pub title: Option<String>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub borderless: Option<bool>,
    #[napi(js_name = "dragAnywhere")]
    pub drag_anywhere: Option<bool>,
    pub resizable: Option<bool>,
    pub opacity: Option<f64>,
}

struct Listeners {
    map: Mutex<HashMap<(Option<u32>, String), Vec<Arc<ListenerFn>>>>,
}

impl Listeners {
    fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }

    fn add(&self, id: Option<u32>, event: String, listener: Arc<ListenerFn>) {
        self.map
            .lock()
            .expect("listeners")
            .entry((id, event))
            .or_default()
            .push(listener);
    }

    fn wants(&self, event: &str) -> bool {
        self.map
            .lock()
            .expect("listeners")
            .keys()
            .any(|(_, name)| name == event)
    }
}

struct CloseState {
    closed: Mutex<bool>,
    cv: Condvar,
}

impl CloseState {
    fn new() -> Self {
        Self {
            closed: Mutex::new(false),
            cv: Condvar::new(),
        }
    }

    fn signal(&self) {
        let mut closed = self.closed.lock().expect("close");
        *closed = true;
        self.cv.notify_all();
    }

    fn wait(&self) {
        let mut closed = self.closed.lock().expect("close");
        while !*closed {
            closed = self.cv.wait(closed).expect("close");
        }
    }
}

struct WaitClose {
    state: Arc<CloseState>,
}

impl Task for WaitClose {
    type Output = bool;
    type JsValue = bool;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        self.state.wait();
        Ok(true)
    }

    fn resolve(&mut self, _env: napi::Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        Ok(output)
    }
}

fn failure(err: SweetError) -> napi::Error {
    napi::Error::from_reason(err.0)
}

fn listen(callback: Function<UiEvent, UnknownReturnValue>) -> napi::Result<Arc<ListenerFn>> {
    // Weak so a listener does not keep Node alive after the window promise settles.
    let function = callback
        .build_threadsafe_function()
        .weak::<true>()
        .build()?;
    Ok(Arc::new(function))
}

#[napi]
pub struct Document {
    inner: Arc<CoreDocument>,
    listeners: Arc<Listeners>,
}

#[napi]
impl Document {
    #[napi(constructor)]
    pub fn new() -> napi::Result<Self> {
        let inner = CoreDocument::new().map_err(failure)?;
        Ok(Self {
            inner: Arc::new(inner),
            listeners: Arc::new(Listeners::new()),
        })
    }

    #[napi(getter)]
    pub fn dark(&self) -> bool {
        self.inner.dark()
    }

    #[napi(setter)]
    pub fn set_dark(&self, dark: bool) {
        self.inner.set_dark(dark);
    }

    #[napi(getter)]
    pub fn body(&self) -> Node {
        Node::from_parts(self, self.inner.body())
    }

    #[napi]
    pub fn create_element(&self, tag: String) -> napi::Result<Node> {
        let id = self.inner.create_element(&tag).map_err(failure)?;
        Ok(Node::from_parts(self, id))
    }

    #[napi]
    pub fn create_text(&self, text: String) -> Node {
        Node::from_parts(self, self.inner.create_text(&text))
    }

    #[napi]
    pub fn create_comment(&self, data: String) -> Node {
        Node::from_parts(self, self.inner.create_comment(&data))
    }

    #[napi]
    pub fn render_png(&self, width: u32, height: u32) -> napi::Result<Buffer> {
        let bytes = self.inner.render_png(width, height).map_err(failure)?;
        Ok(Buffer::from(bytes))
    }

    #[napi]
    pub fn layout_boxes(&self, width: f64, height: f64) -> napi::Result<Vec<LayoutInfo>> {
        let frame = self
            .inner
            .render(&RenderOptions {
                width: width as f32,
                height: height as f32,
                scale: 1.0,
                hover: None,
                focus: None,
                time: 0.0,
            })
            .map_err(failure)?;
        Ok(frame
            .boxes
            .into_iter()
            .map(|b| LayoutInfo {
                id: b.id.0,
                parent: b.parent.map(|id| id.0),
                x: b.x as f64,
                y: b.y as f64,
                width: b.width as f64,
                height: b.height as f64,
            })
            .collect())
    }
}

#[napi]
pub struct Node {
    doc: Arc<CoreDocument>,
    listeners: Arc<Listeners>,
    id: u32,
}

impl Node {
    fn from_parts(document: &Document, id: NodeId) -> Self {
        Self {
            doc: Arc::clone(&document.inner),
            listeners: Arc::clone(&document.listeners),
            id: id.0,
        }
    }

    fn same(&self, other: &Node) -> napi::Result<()> {
        if Arc::ptr_eq(&self.doc, &other.doc) {
            Ok(())
        } else {
            Err(napi::Error::from_reason("node belongs to another document"))
        }
    }
}

#[napi]
impl Node {
    #[napi(getter)]
    pub fn node_id(&self) -> u32 {
        self.id
    }

    #[napi(getter)]
    pub fn tag_name(&self) -> napi::Result<String> {
        self.doc.tag_name(NodeId(self.id)).map_err(failure)
    }

    #[napi(getter)]
    pub fn node_type(&self) -> napi::Result<u32> {
        self.doc.node_type(NodeId(self.id)).map_err(failure)
    }

    #[napi(getter)]
    pub fn class_name(&self) -> napi::Result<String> {
        self.doc.class_name(NodeId(self.id)).map_err(failure)
    }

    #[napi(setter)]
    pub fn set_class_name(&self, value: String) -> napi::Result<()> {
        self.doc
            .set_class_name(NodeId(self.id), &value)
            .map_err(failure)
    }

    #[napi(getter)]
    pub fn text_content(&self) -> napi::Result<String> {
        self.doc.text_content(NodeId(self.id)).map_err(failure)
    }

    #[napi(setter)]
    pub fn set_text_content(&self, value: String) -> napi::Result<()> {
        self.doc
            .set_text_content(NodeId(self.id), &value)
            .map_err(failure)
    }

    #[napi(getter)]
    pub fn id(&self) -> napi::Result<String> {
        self.doc.id_attr(NodeId(self.id)).map_err(failure)
    }

    #[napi(setter)]
    pub fn set_id(&self, value: String) -> napi::Result<()> {
        self.doc
            .set_id_attr(NodeId(self.id), &value)
            .map_err(failure)
    }

    #[napi(getter)]
    pub fn scroll_left(&self) -> napi::Result<f64> {
        self.doc
            .scroll_offset(NodeId(self.id))
            .map(|value| value.0 as f64)
            .map_err(failure)
    }

    #[napi(setter)]
    pub fn set_scroll_left(&self, value: f64) -> napi::Result<()> {
        let (_, top) = self.doc.scroll_offset(NodeId(self.id)).map_err(failure)?;
        self.doc
            .set_scroll_offset(NodeId(self.id), value as f32, top)
            .map_err(failure)
    }

    #[napi(getter)]
    pub fn scroll_top(&self) -> napi::Result<f64> {
        self.doc
            .scroll_offset(NodeId(self.id))
            .map(|value| value.1 as f64)
            .map_err(failure)
    }

    #[napi(setter)]
    pub fn set_scroll_top(&self, value: f64) -> napi::Result<()> {
        let (left, _) = self.doc.scroll_offset(NodeId(self.id)).map_err(failure)?;
        self.doc
            .set_scroll_offset(NodeId(self.id), left, value as f32)
            .map_err(failure)
    }

    #[napi(getter)]
    pub fn scroll_width(&self) -> napi::Result<f64> {
        self.doc
            .scroll_metrics(NodeId(self.id))
            .map(|value| value.0 as f64)
            .map_err(failure)
    }

    #[napi(getter)]
    pub fn scroll_height(&self) -> napi::Result<f64> {
        self.doc
            .scroll_metrics(NodeId(self.id))
            .map(|value| value.1 as f64)
            .map_err(failure)
    }

    #[napi(getter)]
    pub fn client_width(&self) -> napi::Result<f64> {
        self.doc
            .scroll_metrics(NodeId(self.id))
            .map(|value| value.2 as f64)
            .map_err(failure)
    }

    #[napi(getter)]
    pub fn client_height(&self) -> napi::Result<f64> {
        self.doc
            .scroll_metrics(NodeId(self.id))
            .map(|value| value.3 as f64)
            .map_err(failure)
    }

    #[napi]
    pub fn set_attribute(&self, name: String, value: String) -> napi::Result<()> {
        self.doc
            .set_attribute(NodeId(self.id), &name, &value)
            .map_err(failure)
    }

    #[napi]
    pub fn get_attribute(&self, name: String) -> napi::Result<Option<String>> {
        self.doc.attribute(NodeId(self.id), &name).map_err(failure)
    }

    #[napi]
    pub fn remove_attribute(&self, name: String) -> napi::Result<()> {
        self.doc
            .remove_attribute(NodeId(self.id), &name)
            .map_err(failure)
    }

    /// Decode a PNG or JPEG and store it on this element. An `img` paints it.
    #[napi]
    pub fn set_image_encoded(&self, bytes: Buffer) -> napi::Result<ImageSize> {
        let (width, height) = self
            .doc
            .set_image_encoded(NodeId(self.id), bytes.as_ref())
            .map_err(failure)?;
        Ok(ImageSize { width, height })
    }

    /// Replace an element's pixels without an encode/decode round trip. Video
    /// elements use this for frames produced by the media pipeline.
    #[napi]
    pub fn set_image_rgba(&self, width: u32, height: u32, bytes: Buffer) -> napi::Result<()> {
        self.doc
            .set_image_rgba(NodeId(self.id), width, height, bytes.to_vec())
            .map_err(failure)
    }

    #[napi]
    pub fn clear_image(&self) -> napi::Result<()> {
        self.doc.clear_image(NodeId(self.id)).map_err(failure)
    }

    #[napi(getter)]
    pub fn value(&self) -> napi::Result<String> {
        self.doc.control_value(NodeId(self.id)).map_err(failure)
    }

    #[napi(setter)]
    pub fn set_value(&self, value: String) -> napi::Result<()> {
        self.doc
            .set_control_value(NodeId(self.id), &value)
            .map_err(failure)
    }

    #[napi(getter)]
    pub fn disabled(&self) -> napi::Result<bool> {
        self.doc.is_disabled(NodeId(self.id)).map_err(failure)
    }

    #[napi(setter)]
    pub fn set_disabled(&self, disabled: bool) -> napi::Result<()> {
        self.doc
            .set_disabled(NodeId(self.id), disabled)
            .map_err(failure)
    }

    #[napi]
    pub fn append_child(&self, child: &Node) -> napi::Result<()> {
        self.same(child)?;
        self.doc
            .append_child(NodeId(self.id), NodeId(child.id))
            .map_err(failure)
    }

    #[napi]
    pub fn insert_before(&self, child: &Node, before: Option<&Node>) -> napi::Result<()> {
        self.same(child)?;
        if let Some(before) = before {
            self.same(before)?;
        }
        let reference = before.map(|node| NodeId(node.id));
        self.doc
            .insert_before(NodeId(self.id), NodeId(child.id), reference)
            .map_err(failure)
    }

    #[napi]
    pub fn remove_child(&self, child: &Node) -> napi::Result<()> {
        self.same(child)?;
        self.doc
            .remove_child(NodeId(self.id), NodeId(child.id))
            .map_err(failure)
    }

    #[napi]
    pub fn children(&self) -> napi::Result<Vec<Node>> {
        let ids = self.doc.children(NodeId(self.id)).map_err(failure)?;
        Ok(ids
            .into_iter()
            .map(|id| Node {
                doc: Arc::clone(&self.doc),
                listeners: Arc::clone(&self.listeners),
                id: id.0,
            })
            .collect())
    }

    #[napi]
    pub fn parent(&self) -> napi::Result<Option<Node>> {
        Ok(self
            .doc
            .parent(NodeId(self.id))
            .map_err(failure)?
            .map(|id| Node {
                doc: Arc::clone(&self.doc),
                listeners: Arc::clone(&self.listeners),
                id: id.0,
            }))
    }

    #[napi]
    pub fn on(
        &self,
        event: String,
        callback: Function<UiEvent, UnknownReturnValue>,
    ) -> napi::Result<()> {
        match event.as_str() {
            "click" | "mousedown" | "mouseup" | "mousemove" | "input" | "change" | "keydown"
            | "scroll" => {}
            _ => return Err(napi::Error::from_reason(format!("unknown event `{event}`"))),
        }
        self.listeners.add(Some(self.id), event, listen(callback)?);
        Ok(())
    }
}

#[napi]
pub struct Window {
    token: u64,
    proxy: EventLoopProxy<Cmd>,
    close: Arc<CloseState>,
    listeners: Arc<Listeners>,
    resizable: Arc<AtomicBool>,
}

#[napi]
impl Window {
    #[napi(constructor)]
    pub fn new(document: &Document, options: Option<WindowOptions>) -> napi::Result<Self> {
        let options = options.unwrap_or(WindowOptions {
            title: None,
            width: None,
            height: None,
            borderless: None,
            drag_anywhere: None,
            resizable: None,
            opacity: None,
        });
        let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
        let close = Arc::new(CloseState::new());
        let listeners = Arc::clone(&document.listeners);
        let resizable = Arc::new(AtomicBool::new(options.resizable.unwrap_or(true)));
        let proxy = runtime()?;
        let (reply_tx, reply_rx) = mpsc::channel();
        proxy
            .send_event(Cmd::Open(OpenArgs {
                token,
                reply: reply_tx,
                doc: Arc::clone(&document.inner),
                listeners: Arc::clone(&document.listeners),
                close: Arc::clone(&close),
                title: options.title.unwrap_or_else(|| "sweet".to_string()),
                width: options.width.unwrap_or(960.0).max(1.0),
                height: options.height.unwrap_or(600.0).max(1.0),
                borderless: options.borderless.unwrap_or(false),
                drag_anywhere: options.drag_anywhere.unwrap_or(false),
                resizable: resizable.load(Ordering::Relaxed),
                opacity: options.opacity.unwrap_or(1.0).clamp(0.1, 1.0) as f32,
            }))
            .map_err(|_| napi::Error::from_reason("sweet window thread is not running"))?;
        match reply_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                token,
                proxy,
                close,
                listeners,
                resizable,
            }),
            Ok(Err(message)) => Err(napi::Error::from_reason(message)),
            Err(_) => Err(napi::Error::from_reason(
                "sweet window thread exited before the window opened",
            )),
        }
    }

    #[napi]
    pub fn on(
        &self,
        event: String,
        callback: Function<UiEvent, UnknownReturnValue>,
    ) -> napi::Result<()> {
        match event.as_str() {
            "click" | "mousedown" | "mouseup" | "mousemove" | "resize" | "close" => {}
            _ => return Err(napi::Error::from_reason(format!("unknown event `{event}`"))),
        }
        self.listeners.add(None, event, listen(callback)?);
        Ok(())
    }

    #[napi]
    pub fn close(&self) {
        let _ = self.proxy.send_event(Cmd::Close(self.token));
    }

    #[napi]
    pub fn minimize(&self) {
        let _ = self.proxy.send_event(Cmd::Minimize(self.token));
    }

    #[napi(js_name = "toggleMaximize")]
    pub fn toggle_maximize(&self) {
        let _ = self.proxy.send_event(Cmd::ToggleMaximize(self.token));
    }

    #[napi(getter)]
    pub fn resizable(&self) -> bool {
        self.resizable.load(Ordering::Acquire)
    }

    #[napi(setter)]
    pub fn set_resizable(&self, value: bool) {
        self.resizable.store(value, Ordering::Release);
        let _ = self.proxy.send_event(Cmd::SetResizable(self.token, value));
    }

    #[napi]
    #[allow(private_interfaces)]
    pub fn closed(&self) -> AsyncTask<WaitClose> {
        AsyncTask::new(WaitClose {
            state: Arc::clone(&self.close),
        })
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        if self.proxy.send_event(Cmd::Close(self.token)).is_err() {
            self.close.signal();
        }
    }
}

struct OpenArgs {
    token: u64,
    reply: mpsc::Sender<Result<(), String>>,
    doc: Arc<CoreDocument>,
    listeners: Arc<Listeners>,
    close: Arc<CloseState>,
    title: String,
    width: f64,
    height: f64,
    borderless: bool,
    drag_anywhere: bool,
    resizable: bool,
    opacity: f32,
}

enum Cmd {
    Open(OpenArgs),
    Redraw(u64),
    Minimize(u64),
    ToggleMaximize(u64),
    SetResizable(u64, bool),
    Close(u64),
}

struct Live {
    token: u64,
    doc: Arc<CoreDocument>,
    listeners: Arc<Listeners>,
    close: Arc<CloseState>,
    // The display context has to stay alive for as long as the surface does.
    #[allow(dead_code)]
    context: softbuffer::Context<OwnedDisplayHandle>,
    surface: softbuffer::Surface<OwnedDisplayHandle, OsWindow>,
    boxes: Vec<LayoutBox>,
    hover: Option<NodeId>,
    focus: Option<NodeId>,
    pressed: Option<NodeId>,
    drag_anywhere: bool,
    resizable: bool,
    opacity: f32,
    transparent: bool,
    resize_drag: Option<ResizeDrag>,
    field_nodes: Vec<NodeId>,
    cursor: CursorIcon,
    last_x: f64,
    last_y: f64,
    /// Set while a DOM redraw is waiting on the window thread, so a burst of
    /// writes wakes that thread once.
    redraw_queued: Arc<AtomicBool>,
    /// Focus changed on press. The frame is requested after pointer events,
    /// so mouseup can queue `click` before any paint.
    focus_frame: bool,
    painted: bool,
    fields: Option<fields::Fields>,
    repaint_at: Option<Instant>,
    opened: Instant,
    /// Egui's last text picture. Reused while the caret does not need a new frame.
    field_scratch: Vec<u32>,
    field_stamp: u64,
    field_until: Option<Instant>,
}

struct ResizeDrag {
    direction: ResizeDirection,
    start_x: f64,
    start_y: f64,
    start_position: PhysicalPosition<i32>,
    start_size: PhysicalSize<u32>,
}

struct App {
    proxy: EventLoopProxy<Cmd>,
    windows: HashMap<WindowId, Live>,
    tokens: HashMap<u64, WindowId>,
}

fn runtime() -> napi::Result<EventLoopProxy<Cmd>> {
    let slot = RUNTIME.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel(1);
        let spawned = thread::Builder::new()
            .name("sweet-window".into())
            .spawn(move || {
                let mut builder = EventLoop::<Cmd>::with_user_event();
                // The window loop is a thread inside the Node process. winit allows that
                // when any_thread is set (X11 here, and the Win32 event loop too).
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
        Err(message) => Err(napi::Error::from_reason(message.clone())),
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
            Cmd::Minimize(token) => {
                if let Some(id) = self.tokens.get(&token).copied() {
                    if let Some(live) = self.windows.get(&id) {
                        live.surface.window().set_minimized(true);
                    }
                }
            }
            Cmd::ToggleMaximize(token) => {
                if let Some(id) = self.tokens.get(&token).copied() {
                    if let Some(live) = self.windows.get(&id) {
                        let window = live.surface.window();
                        window.set_maximized(!window.is_maximized());
                    }
                }
            }
            Cmd::SetResizable(token, resizable) => {
                if let Some(id) = self.tokens.get(&token).copied() {
                    if let Some(live) = self.windows.get_mut(&id) {
                        live.resizable = resizable;
                        live.surface.window().set_resizable(resizable);
                        if !resizable {
                            live.resize_drag = None;
                            live.surface.window().set_cursor(CursorIcon::Default);
                            live.cursor = CursorIcon::Default;
                        }
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
            // egui answers RedrawRequested with repaint=true. Honoring that
            // queues the next frame immediately, so animation runs as fast as
            // paint will go and the motion stutters like a CSS timer.
            if repaint && !matches!(event, WindowEvent::RedrawRequested) {
                live.surface.window().request_redraw();
            }
        }
        match event {
            WindowEvent::RedrawRequested => {
                live.painted = true;
                let _ = paint(live);
            }
            WindowEvent::Resized(size) => {
                let logical = size.to_logical::<f64>(live.surface.window().scale_factor());
                dispatch(
                    live,
                    "resize",
                    None,
                    logical.width,
                    logical.height,
                    ThreadsafeFunctionCallMode::NonBlocking,
                    None,
                );
                live.surface.window().request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = live.surface.window().inner_size();
                let logical = size.to_logical::<f64>(live.surface.window().scale_factor());
                dispatch(
                    live,
                    "resize",
                    None,
                    logical.width,
                    logical.height,
                    ThreadsafeFunctionCallMode::NonBlocking,
                    None,
                );
                live.surface.window().request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                resize_borderless(live, position.x, position.y);
                let (x, y, hit) = pointer(live, position.x, position.y);
                live.last_x = x;
                live.last_y = y;
                let resize_cursor = live
                    .resizable
                    .then(|| resize_direction(live, x, y))
                    .flatten()
                    .map(CursorIcon::from);
                let cursor = resize_cursor.unwrap_or_else(|| match box_cursor(&live.boxes, hit) {
                    CursorStyle::Pointer => CursorIcon::Pointer,
                    CursorStyle::Default => CursorIcon::Default,
                });
                if cursor != live.cursor {
                    live.cursor = cursor;
                    live.surface.window().set_cursor(cursor);
                }
                let hover_changed = hit != live.hover;
                if hover_changed {
                    live.hover = hit;
                    live.surface.window().request_redraw();
                }
                if live.listeners.wants("mousemove") {
                    dispatch(
                        live,
                        "mousemove",
                        hit,
                        x,
                        y,
                        ThreadsafeFunctionCallMode::NonBlocking,
                        None,
                    );
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scale = live.surface.window().scale_factor() as f32;
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 40.0, y * 40.0),
                    MouseScrollDelta::PixelDelta(position) => {
                        (position.x as f32 / scale, position.y as f32 / scale)
                    }
                };
                scroll_at_pointer(live, dx, dy);
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let (x, y, hit) = at_pointer(live);
                match state {
                    ElementState::Pressed => {
                        if let Some(direction) = live
                            .resizable
                            .then(|| resize_direction(live, x, y))
                            .flatten()
                        {
                            if live.drag_anywhere {
                                let window = live.surface.window();
                                if let (Ok(start_position), start_size) =
                                    (window.outer_position(), window.inner_size())
                                {
                                    let scale = window.scale_factor();
                                    live.resize_drag = Some(ResizeDrag {
                                        direction,
                                        start_x: x * scale,
                                        start_y: y * scale,
                                        start_position,
                                        start_size,
                                    });
                                    live.pressed = None;
                                    return;
                                }
                            } else if live.surface.window().drag_resize_window(direction).is_ok() {
                                return;
                            }
                        }
                        if live.drag_anywhere && !blocks_window_drag(live, hit) {
                            live.pressed = None;
                            if live.surface.window().drag_window().is_ok() {
                                return;
                            }
                        }
                        // A paint here runs before mouseup is delivered, so the
                        // click handler cannot start until the frame finishes.
                        if live.focus != hit {
                            live.focus_frame = true;
                        }
                        live.pressed = hit;
                        live.focus = hit;
                        dispatch(
                            live,
                            "mousedown",
                            hit,
                            x,
                            y,
                            ThreadsafeFunctionCallMode::NonBlocking,
                            None,
                        );
                    }
                    ElementState::Released => {
                        live.resize_drag = None;
                        dispatch(
                            live,
                            "mouseup",
                            hit,
                            x,
                            y,
                            ThreadsafeFunctionCallMode::NonBlocking,
                            None,
                        );
                        if hit == live.pressed {
                            dispatch(
                                live,
                                "click",
                                hit,
                                x,
                                y,
                                ThreadsafeFunctionCallMode::NonBlocking,
                                None,
                            );
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
            // X11 delivers redraws after pointer events. Asking at the end of
            // a turn that did not already paint keeps the click ahead of the
            // frame, and folds it into a DOM redraw queued by the handler.
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
            listeners,
            close,
            title,
            width,
            height,
            borderless,
            drag_anywhere,
            resizable,
            opacity,
            ..
        } = open;
        let outcome = (|| -> Result<(), String> {
            let attrs = OsWindow::default_attributes()
                .with_title(title)
                .with_inner_size(LogicalSize::new(width, height))
                .with_decorations(!borderless)
                .with_transparent(borderless)
                .with_resizable(resizable);
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
                listeners,
                close,
                context,
                surface,
                boxes: Vec::new(),
                hover: None,
                focus: None,
                pressed: None,
                drag_anywhere,
                resizable,
                opacity: if borderless { opacity } else { 1.0 },
                transparent: borderless,
                resize_drag: None,
                field_nodes: Vec::new(),
                cursor: CursorIcon::Default,
                last_x: 0.0,
                last_y: 0.0,
                redraw_queued,
                focus_frame: false,
                painted: false,
                fields: None,
                repaint_at: None,
                opened: Instant::now(),
                field_scratch: Vec::new(),
                field_stamp: 0,
                field_until: None,
            };
            live.fields = Some(fields::Fields::new(live.surface.window()));
            if let Err(err) = paint(&mut live) {
                live.doc.clear_redraw_hook(token);
                return Err(err);
            }
            self.tokens.insert(token, id);
            self.windows.insert(id, live);
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
        // Wait until close handlers return, then resolve `closed`. A handler must
        // not wait on that promise, or this thread and JavaScript deadlock.
        dispatch(
            &live,
            "close",
            None,
            0.0,
            0.0,
            ThreadsafeFunctionCallMode::Blocking,
            None,
        );
        live.close.signal();
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
    let painted_at = Instant::now();
    let (boxes, text_fields, animating) = if live.transparent {
        render_argb(&live.doc, &options, live.opacity, &mut buffer)
    } else {
        render_xrgb(&live.doc, &options, &mut buffer)
    }
    .map_err(|err| err.0)?;
    let paint_us = painted_at.elapsed().as_micros();
    live.boxes = boxes;
    live.field_nodes.clear();
    live.field_nodes
        .extend(text_fields.iter().map(|field| field.id));
    let pixels = (frame_w as usize).saturating_mul(frame_h as usize);
    if live.field_scratch.len() != pixels {
        live.field_scratch = vec![0u32; pixels];
        live.field_stamp = 0;
    }
    let stamp = field_stamp(&text_fields, live.focus);
    let rerun_fields =
        stamp != live.field_stamp || live.field_until.is_some_and(|at| Instant::now() >= at);
    let mut repaint_at = live.field_until;
    if animating {
        // Lock to the window's 60 Hz epoch. Adding 16ms after the paint
        // finishes drifts and skips vsync, which reads as CSS-style stutter.
        let next = next_frame(live.opened, Instant::now());
        repaint_at = Some(repaint_at.map_or(next, |at| at.min(next)));
    }
    live.repaint_at = repaint_at;
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
        if animating {
            if let Some(at) = live.field_until {
                live.repaint_at = Some(live.repaint_at.map_or(at, |soon| soon.min(at)));
            }
        } else {
            live.repaint_at = live.field_until;
        }
        let mut buffer = live.surface.buffer_mut().map_err(|err| err.to_string())?;
        blend_fields(
            &mut buffer,
            &live.field_scratch,
            &text_fields,
            frame_w,
            frame_h,
        );
        buffer.present().map_err(|err| err.to_string())?;
        updates
    } else {
        blend_fields(
            &mut buffer,
            &live.field_scratch,
            &text_fields,
            frame_w,
            frame_h,
        );
        buffer.present().map_err(|err| err.to_string())?;
        Vec::new()
    };
    note_frame(live.opened, paint_us, animating);
    for update in updates {
        let previous = text_fields
            .iter()
            .find(|field| field.id == update.id)
            .map(|field| field.value.as_str())
            .unwrap_or("");
        if update.value != previous {
            let _ = live.doc.set_control_value(update.id, &update.value);
            dispatch(
                live,
                "input",
                Some(update.id),
                0.0,
                0.0,
                ThreadsafeFunctionCallMode::NonBlocking,
                None,
            );
        }
        if update.submit {
            dispatch(
                live,
                "keydown",
                Some(update.id),
                0.0,
                0.0,
                ThreadsafeFunctionCallMode::NonBlocking,
                Some("Enter"),
            );
        }
    }
    Ok(())
}

const FRAME_NS: u64 = 1_000_000_000 / 60;

fn frame_index(opened: Instant, now: Instant) -> u64 {
    now.saturating_duration_since(opened).as_nanos() as u64 / FRAME_NS
}

/// Animation time steps by exactly one frame per 60 Hz slot, so a spin does
/// not speed up and slow down when a paint finishes early or late inside the slot.
fn frame_clock(opened: Instant, now: Instant) -> f32 {
    frame_index(opened, now) as f32 / 60.0
}

/// Next 60 Hz slot after `now`, measured from the moment the window opened.
fn next_frame(opened: Instant, now: Instant) -> Instant {
    let frame = frame_index(opened, now);
    opened + Duration::from_nanos(frame.saturating_add(1).saturating_mul(FRAME_NS))
}

fn note_frame(_opened: Instant, paint_us: u128, animating: bool) {
    if !animating {
        return;
    }
    thread_local! {
        static ON: bool = std::env::var_os("SWEET_PROF").is_some();
        static STATS: std::cell::Cell<(u32, u128, Option<Instant>)> = const { std::cell::Cell::new((0, 0, None)) };
    }
    if !ON.with(|on| *on) {
        return;
    }
    let now = Instant::now();
    let (count, paint_sum, previous) = STATS.with(|cell| cell.get());
    let count = count + 1;
    let paint_sum = paint_sum + paint_us;
    if count % 60 == 0 {
        let interval_ms = previous
            .map(|then| now.saturating_duration_since(then).as_secs_f64() * 1000.0 / 60.0)
            .unwrap_or(0.0);
        eprintln!(
            "sweet frame {count} paint {:.2} ms interval {interval_ms:.2} ms",
            paint_sum as f64 / 1000.0 / 60.0
        );
    }
    let mark = if count % 60 == 0 { Some(now) } else { previous };
    let paint_sum = if count % 60 == 0 { 0 } else { paint_sum };
    STATS.with(|cell| cell.set((count, paint_sum, mark)));
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

fn blend_fields(
    buffer: &mut [u32],
    scratch: &[u32],
    fields: &[TextField],
    width: u32,
    height: u32,
) {
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
    let hit = hit_test(&live.boxes, x as f32, y as f32);
    (x, y, hit)
}

fn at_pointer(live: &Live) -> (f64, f64, Option<NodeId>) {
    let hit = hit_test(&live.boxes, live.last_x as f32, live.last_y as f32);
    (live.last_x, live.last_y, hit)
}

fn box_cursor(boxes: &[LayoutBox], hit: Option<NodeId>) -> CursorStyle {
    hit.and_then(|id| boxes.iter().rev().find(|b| b.id == id))
        .map(|b| b.cursor)
        .unwrap_or(CursorStyle::Default)
}

fn hit_test(boxes: &[LayoutBox], x: f32, y: f32) -> Option<NodeId> {
    boxes.iter().rev().find(|b| b.hits(x, y)).map(|b| b.id)
}

fn resize_direction(live: &Live, x: f64, y: f64) -> Option<ResizeDirection> {
    const EDGE: f64 = 12.0;
    let window = live.surface.window();
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    let west = x < EDGE;
    let east = x >= size.width - EDGE;
    let north = y < EDGE;
    let south = y >= size.height - EDGE;
    Some(match (west, east, north, south) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (_, true, true, _) => ResizeDirection::NorthEast,
        (true, _, _, true) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, _, _, _) => ResizeDirection::West,
        (_, true, _, _) => ResizeDirection::East,
        (_, _, true, _) => ResizeDirection::North,
        (_, _, _, true) => ResizeDirection::South,
        _ => return None,
    })
}

fn resize_borderless(live: &Live, physical_x: f64, physical_y: f64) {
    let Some(drag) = live.resize_drag.as_ref() else {
        return;
    };
    let dx = (physical_x - drag.start_x).round() as i32;
    let dy = (physical_y - drag.start_y).round() as i32;
    let west = matches!(
        drag.direction,
        ResizeDirection::West | ResizeDirection::NorthWest | ResizeDirection::SouthWest
    );
    let east = matches!(
        drag.direction,
        ResizeDirection::East | ResizeDirection::NorthEast | ResizeDirection::SouthEast
    );
    let north = matches!(
        drag.direction,
        ResizeDirection::North | ResizeDirection::NorthEast | ResizeDirection::NorthWest
    );
    let south = matches!(
        drag.direction,
        ResizeDirection::South | ResizeDirection::SouthEast | ResizeDirection::SouthWest
    );

    let width = drag.start_size.width as i32;
    let height = drag.start_size.height as i32;
    let next_width = if west {
        (width - dx).max(320)
    } else if east {
        (width + dx).max(320)
    } else {
        width
    };
    let next_height = if north {
        (height - dy).max(240)
    } else if south {
        (height + dy).max(240)
    } else {
        height
    };
    let position = PhysicalPosition::new(
        drag.start_position.x + if west { width - next_width } else { 0 },
        drag.start_position.y + if north { height - next_height } else { 0 },
    );
    let window = live.surface.window();
    let _ = window.request_inner_size(PhysicalSize::new(next_width as u32, next_height as u32));
    window.set_outer_position(position);
}

fn scroll_at_pointer(live: &mut Live, mut dx: f32, mut dy: f32) {
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    let mut current = hit_test(&live.boxes, live.last_x as f32, live.last_y as f32);
    let mut guard = 0usize;
    while let Some(id) = current {
        let Some(layout) = live.boxes.iter().rev().find(|b| b.id == id) else {
            break;
        };
        let parent = layout.parent;
        let (old_left, old_top) = live.doc.scroll_offset(id).unwrap_or((0.0, 0.0));
        let next_left = if layout.scrollable_x
            && (layout.max_scroll_left > 0.0 || layout.scrollbar_gutter > 0.0)
        {
            (old_left + dx).clamp(0.0, layout.max_scroll_left)
        } else {
            old_left
        };
        let next_top = if layout.scrollable_y
            && (layout.max_scroll_top > 0.0 || layout.scrollbar_gutter > 0.0)
        {
            (old_top - dy).clamp(0.0, layout.max_scroll_top)
        } else {
            old_top
        };
        let consumed_x = next_left - old_left;
        let consumed_y = old_top - next_top;
        if consumed_x != 0.0 || consumed_y != 0.0 {
            let _ = live.doc.set_scroll_offset(id, next_left, next_top);
            let x = live.last_x;
            let y = live.last_y;
            dispatch(
                live,
                "scroll",
                Some(id),
                x,
                y,
                ThreadsafeFunctionCallMode::NonBlocking,
                None,
            );
            live.surface.window().request_redraw();
            dx -= consumed_x;
            dy -= consumed_y;
        }
        if dx.abs() < 0.01 && dy.abs() < 0.01 {
            break;
        }
        current = parent;
        guard += 1;
        if guard > 4096 {
            break;
        }
    }
}

fn blocks_window_drag(live: &Live, target: Option<NodeId>) -> bool {
    let map = live.listeners.map.lock().expect("listeners");
    if map
        .keys()
        .any(|(id, event)| id.is_none() && event == "click")
    {
        return true;
    }
    let mut current = target;
    let mut guard = 0usize;
    while let Some(id) = current {
        if live.field_nodes.contains(&id)
            || map
                .keys()
                .any(|(target, event)| *target == Some(id.0) && event == "click")
        {
            return true;
        }
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
    false
}

fn dispatch(
    live: &Live,
    event: &str,
    target: Option<NodeId>,
    x: f64,
    y: f64,
    mode: ThreadsafeFunctionCallMode,
    key: Option<&str>,
) {
    let mut chain = Vec::new();
    let mut current = target;
    let mut guard = 0usize;
    while let Some(id) = current {
        chain.push(Some(id.0));
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
    chain.push(None);
    let listeners = {
        let map = live.listeners.map.lock().expect("listeners");
        let mut found = Vec::new();
        for id in chain {
            if let Some(list) = map.get(&(id, event.to_string())) {
                found.extend(list.iter().cloned());
            }
        }
        found
    };
    for listener in listeners {
        let ui = UiEvent {
            kind: event.to_string(),
            node_id: target.map(|node| node.0),
            x,
            y,
            key: key.map(str::to_string),
        };
        if mode == ThreadsafeFunctionCallMode::Blocking {
            let (tx, rx) = mpsc::sync_channel(1);
            let status = listener.call_with_return_value(
                ui,
                ThreadsafeFunctionCallMode::NonBlocking,
                move |_, _| {
                    let _ = tx.send(());
                    Ok(())
                },
            );
            if status == Status::Ok {
                let _ = rx.recv_timeout(Duration::from_secs(2));
            }
        } else {
            let _ = listener.call(ui, mode);
        }
    }
}

