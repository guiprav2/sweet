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
    physical_size, render_xrgb, CursorStyle, Document as CoreDocument, LayoutBox, NodeId,
    RenderOptions, SweetError,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{
    ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy, OwnedDisplayHandle,
};
use winit::window::{CursorIcon, Window as OsWindow, WindowId};

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
            "click" | "mousedown" | "mouseup" | "mousemove" | "input" | "change" | "keydown" => {}
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
}

#[napi]
impl Window {
    #[napi(constructor)]
    pub fn new(document: &Document, options: Option<WindowOptions>) -> napi::Result<Self> {
        let options = options.unwrap_or(WindowOptions {
            title: None,
            width: None,
            height: None,
        });
        let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
        let close = Arc::new(CloseState::new());
        let listeners = Arc::clone(&document.listeners);
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
            }))
            .map_err(|_| napi::Error::from_reason("sweet window thread is not running"))?;
        match reply_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                token,
                proxy,
                close,
                listeners,
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
            "click" | "mousedown" | "mouseup" | "mousemove" | "close" => {}
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
}

enum Cmd {
    Open(OpenArgs),
    Redraw(u64),
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
    cursor: CursorStyle,
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
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let (x, y, hit) = at_pointer(live);
                match state {
                    ElementState::Pressed => {
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
                listeners,
                close,
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
                repaint_at: None,
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
    };
    let (frame_w, frame_h) = physical_size(&options);
    let width = NonZeroU32::new(frame_w).ok_or_else(|| "empty frame".to_string())?;
    let height = NonZeroU32::new(frame_h).ok_or_else(|| "empty frame".to_string())?;
    live.surface
        .resize(width, height)
        .map_err(|err| err.to_string())?;
    let mut buffer = live.surface.buffer_mut().map_err(|err| err.to_string())?;
    let (boxes, text_fields) =
        render_xrgb(&live.doc, &options, &mut buffer).map_err(|err| err.0)?;
    drop(buffer);
    live.boxes = boxes;
    let (scratch, updates, delay) = if let Some(editor) = live.fields.as_mut() {
        let mut scratch = vec![0u32; (frame_w as usize).saturating_mul(frame_h as usize)];
        let updates = editor.run(
            live.surface.window(),
            &text_fields,
            live.focus,
            &mut scratch,
            frame_w,
            frame_h,
        );
        let delay = editor.repaint_after;
        (scratch, updates, delay)
    } else {
        (Vec::new(), Vec::new(), None)
    };
    live.repaint_at = delay.and_then(|delay| {
        if delay < Duration::from_secs(2) {
            Some(Instant::now() + delay)
        } else {
            None
        }
    });
    let mut buffer = live.surface.buffer_mut().map_err(|err| err.to_string())?;
    for (index, pixel) in scratch.iter().enumerate() {
        if pixel >> 24 != 0 && index < buffer.len() {
            buffer[index] = fields::onto_screen(buffer[index], *pixel);
        }
    }
    buffer.present().map_err(|err| err.to_string())?;
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
    for b in boxes.iter().rev() {
        if !b.hit_test || b.width <= 0.0 || b.height <= 0.0 {
            continue;
        }
        let in_box = x >= b.x && y >= b.y && x < b.x + b.width && y < b.y + b.height;
        let in_clip =
            x >= b.clip_x && y >= b.clip_y && x < b.clip_x + b.clip_w && y < b.clip_y + b.clip_h;
        if in_box && in_clip {
            return Some(b.id);
        }
    }
    None
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
