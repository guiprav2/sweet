//! In-process Node binding. JavaScript mutates the DOM through these methods.
//! The renderer has no JavaScript engine of its own.
//!
//! A native thread owns the window loop and presentation surfaces. Each window
//! has one persistent render worker with a latest-job/latest-frame handoff;
//! DOM calls stay on the Node thread and wake the window loop for redraws.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
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
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{CursorIcon, ResizeDirection, Window as OsWindow, WindowId};

mod fields;
mod media;

type ListenerFn = ThreadsafeFunction<UiEvent, UnknownReturnValue, UiEvent, Status, false, true>;

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
static RUNTIME: OnceLock<Result<EventLoopProxy<Cmd>, String>> = OnceLock::new();
#[cfg(feature = "android-apk")]
static ANDROID_INPUT_TARGET: AtomicU32 = AtomicU32::new(u32::MAX);
#[cfg(feature = "android-apk")]
static ANDROID_VIEWPORT_HEIGHT: AtomicU32 = AtomicU32::new(0);

#[napi(object)]
pub struct UiEvent {
    #[napi(js_name = "type")]
    pub kind: String,
    pub node_id: Option<u32>,
    pub x: f64,
    pub y: f64,
    #[napi(js_name = "deltaX")]
    pub delta_x: f64,
    #[napi(js_name = "deltaY")]
    pub delta_y: f64,
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
    video: Arc<Mutex<Option<media::Player>>>,
    video_request: Arc<AtomicU64>,
}

#[napi(object)]
pub struct VideoInfo {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
}

struct OpenVideo {
    doc: Arc<CoreDocument>,
    id: u32,
    source: String,
    slot: Arc<Mutex<Option<media::Player>>>,
    request: Arc<AtomicU64>,
    request_id: u64,
}

impl Task for OpenVideo {
    type Output = media::ResolvedSource;
    type JsValue = VideoInfo;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        media::trace(format_args!("open_video compute begin node={} request={}", self.id, self.request_id));
        let result = media::resolve_source(&self.source);
        match &result {
            Ok(_) => media::trace(format_args!(
                "open_video compute complete node={} request={}",
                self.id,
                self.request_id
            )),
            Err(err) => media::trace(format_args!(
                "open_video compute failed node={} request={} error={err}",
                self.id,
                self.request_id
            )),
        }
        result.map_err(|err| napi::Error::from_reason(err))
    }

    fn resolve(&mut self, _env: napi::Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        media::trace(format_args!("open_video resolve begin node={} request={}", self.id, self.request_id));
        if self.request.load(Ordering::Acquire) != self.request_id {
            media::trace(format_args!("open_video result stale node={} request={}", self.id, self.request_id));
            return Err(napi::Error::from_reason("video source changed"));
        }
        media::trace(format_args!("locking player slot to install node={} request={}", self.id, self.request_id));
        let mut slot = self.slot.lock().expect("video");
        if slot.is_none() {
            *slot = Some(media::Player::new(Arc::clone(&self.doc), NodeId(self.id)));
        }
        let info = slot.as_mut().expect("video").install(output);
        media::trace(format_args!("open_video resolve complete node={} request={}", self.id, self.request_id));
        Ok(VideoInfo {
            duration: info.duration,
            width: info.width,
            height: info.height,
        })
    }
}

impl Node {
    fn from_parts(document: &Document, id: NodeId) -> Self {
        Self {
            doc: Arc::clone(&document.inner),
            listeners: Arc::clone(&document.listeners),
            id: id.0,
            video: Arc::new(Mutex::new(None)),
            video_request: Arc::new(AtomicU64::new(0)),
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

    /// Resolve a local file, direct URL, or YouTube page and decode it in-process.
    /// Replaces the ffmpeg/ffprobe spawn the DOM used to probe a video element.
    /// Resolve a source off the Node thread. Downloads and container probes
    /// can take a while; the decoder itself still runs on the media thread.
    #[napi]
    #[allow(private_interfaces)]
    pub fn open_video(&self, source: String) -> AsyncTask<OpenVideo> {
        let request_id = self.video_request.fetch_add(1, Ordering::AcqRel) + 1;
        media::trace(format_args!("open_video queued node={} request={request_id}", self.id));
        AsyncTask::new(OpenVideo {
            doc: Arc::clone(&self.doc),
            id: self.id,
            source,
            slot: Arc::clone(&self.video),
            request: Arc::clone(&self.video_request),
            request_id,
        })
    }

    /// Configure the native video tag for compressed chunks supplied by JS.
    /// `codec_private` contains Annex-B parameter sets for H.264/HEVC or an
    /// AV1 sequence header OBU. JS owns fetching and container demuxing.
    #[napi]
    pub fn configure_video_stream(
        &self,
        codec: String,
        codec_private: Buffer,
        duration: f64,
        width: u32,
        height: u32,
    ) -> napi::Result<()> {
        let mut slot = self.video.lock().expect("video");
        if slot.is_none() {
            *slot = Some(media::Player::new(Arc::clone(&self.doc), NodeId(self.id)));
        }
        slot.as_mut()
            .expect("video")
            .configure_stream(&codec, codec_private.to_vec(), duration, width, height)
            .map_err(napi::Error::from_reason)
    }

    /// Queue one timestamped compressed access unit. With `reset_queue`, any
    /// buffered packets are discarded and decoding restarts at this chunk's
    /// keyframe, targeting `target_time` (used by seeking).
    #[napi]
    pub fn enqueue_video_chunk(
        &self,
        bytes: Buffer,
        timestamp: f64,
        keyframe: bool,
        reset_queue: bool,
        target_time: f64,
    ) -> napi::Result<bool> {
        let slot = self.video.lock().expect("video");
        let player = slot
            .as_ref()
            .ok_or_else(|| napi::Error::from_reason("video stream is not configured"))?;
        player
            .enqueue_stream_chunk(bytes.to_vec(), timestamp, keyframe, reset_queue, target_time)
            .map_err(napi::Error::from_reason)
    }

    #[napi]
    pub fn configure_audio_stream(
        &self,
        codec: String,
        extra_data: Buffer,
        sample_rate: u32,
        channels: u16,
        time_scale: u32,
    ) -> napi::Result<()> {
        let mut slot = self.video.lock().expect("video");
        if slot.is_none() {
            *slot = Some(media::Player::new(Arc::clone(&self.doc), NodeId(self.id)));
        }
        slot.as_ref()
            .expect("video")
            .configure_stream_audio(&codec, extra_data.to_vec(), sample_rate, channels, time_scale)
            .map_err(napi::Error::from_reason)
    }

    #[napi]
    pub fn enqueue_audio_chunk(
        &self,
        bytes: Buffer,
        timestamp: f64,
        duration: f64,
        reset_queue: bool,
        target_time: f64,
    ) -> napi::Result<bool> {
        let slot = self.video.lock().expect("video");
        let player = slot
            .as_ref()
            .ok_or_else(|| napi::Error::from_reason("video stream is not configured"))?;
        player
            .enqueue_stream_audio(bytes.to_vec(), timestamp, duration, reset_queue, target_time)
            .map_err(napi::Error::from_reason)
    }

    #[napi]
    pub fn audio_stream_queue_len(&self) -> u32 {
        self.video
            .lock()
            .expect("video")
            .as_ref()
            .map(|player| player.stream_audio_queue_len() as u32)
            .unwrap_or(0)
    }

    #[napi]
    pub fn finish_video_stream(&self) {
        if let Some(player) = self.video.lock().expect("video").as_ref() {
            player.finish_stream();
        }
    }

    #[napi]
    pub fn video_stream_queue_len(&self) -> u32 {
        self.video
            .lock()
            .expect("video")
            .as_ref()
            .map(|player| player.stream_queue_len() as u32)
            .unwrap_or(0)
    }

    #[napi]
    pub fn play_video(&self, seconds: f64) -> napi::Result<()> {
        media::trace(format_args!("N-API play_video node={} start={seconds:.3}s", self.id));
        let mut slot = self.video.lock().expect("video");
        let player = slot
            .as_mut()
            .ok_or_else(|| napi::Error::from_reason("video has no source"))?;
        player
            .play(seconds)
            .map_err(|err| napi::Error::from_reason(err))
    }

    #[napi]
    pub fn pause_video(&self) {
        media::trace(format_args!("N-API pause_video node={}", self.id));
        if let Some(player) = self.video.lock().expect("video").as_mut() {
            player.pause();
        }
    }

    #[napi]
    pub fn stop_video(&self, clear_frame: bool) {
        media::trace(format_args!("N-API stop_video node={} clear_frame={clear_frame}", self.id));
        if let Some(player) = self.video.lock().expect("video").as_mut() {
            player.stop_playback(clear_frame);
        }
    }

    #[napi]
    pub fn seek_video(&self, seconds: f64) -> napi::Result<()> {
        media::trace(format_args!("N-API seek_video node={} seconds={seconds:.3}", self.id));
        let mut slot = self.video.lock().expect("video");
        let Some(player) = slot.as_mut() else {
            return Ok(());
        };
        player.seek(seconds);
        Ok(())
    }

    #[napi]
    pub fn set_volume(&self, volume: f64) {
        if let Some(player) = self.video.lock().expect("video").as_ref() {
            player.set_volume(volume);
        }
    }

    #[napi]
    pub fn set_muted(&self, muted: bool) {
        if let Some(player) = self.video.lock().expect("video").as_ref() {
            player.set_muted(muted);
        }
    }

    #[napi]
    pub fn video_current_time(&self) -> f64 {
        self.video
            .lock()
            .expect("video")
            .as_ref()
            .map(|player| player.current_time())
            .unwrap_or(0.0)
    }

    #[napi]
    pub fn video_duration(&self) -> f64 {
        self.video
            .lock()
            .expect("video")
            .as_ref()
            .map(|player| player.duration())
            .unwrap_or(f64::NAN)
    }

    #[napi]
    pub fn video_size(&self) -> VideoInfo {
        let slot = self.video.lock().expect("video");
        let (width, height) = slot
            .as_ref()
            .map(|player| player.video_size())
            .unwrap_or((0, 0));
        let duration = slot
            .as_ref()
            .map(|player| player.duration())
            .unwrap_or(f64::NAN);
        VideoInfo {
            duration,
            width,
            height,
        }
    }

    #[napi]
    pub fn video_ended(&self) -> bool {
        self.video
            .lock()
            .expect("video")
            .as_ref()
            .is_some_and(|player| player.ended())
    }

    #[napi]
    pub fn video_playing(&self) -> bool {
        self.video
            .lock()
            .expect("video")
            .as_ref()
            .is_some_and(|player| player.playing())
    }

    #[napi]
    pub fn video_frame_count(&self) -> f64 {
        self.video
            .lock()
            .expect("video")
            .as_ref()
            .map(|player| player.frame_count() as f64)
            .unwrap_or(0.0)
    }

    #[napi]
    pub fn video_error(&self) -> Option<String> {
        self.video
            .lock()
            .expect("video")
            .as_ref()
            .and_then(|player| player.take_error())
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
                video: Arc::new(Mutex::new(None)),
                video_request: Arc::new(AtomicU64::new(0)),
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
                video: Arc::new(Mutex::new(None)),
                video_request: Arc::new(AtomicU64::new(0)),
            }))
    }

    #[napi]
    pub fn on(
        &self,
        event: String,
        callback: Function<UiEvent, UnknownReturnValue>,
    ) -> napi::Result<()> {
        match event.as_str() {
            "click" | "mousedown" | "mouseup" | "mousemove" | "mouseleave" | "wheel" | "input" | "change" | "keydown"
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
            "click" | "mousedown" | "mouseup" | "mousemove" | "resize" | "maximize" | "close" => {}
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
    FrameReady(u64),
    Minimize(u64),
    ToggleMaximize(u64),
    SetResizable(u64, bool),
    Close(u64),
    #[cfg(feature = "android-apk")]
    AndroidTextChanged { target: u32, value: String },
    #[cfg(feature = "android-apk")]
    AndroidViewportChanged(u32),
}

struct Live {
    token: u64,
    doc: Arc<CoreDocument>,
    listeners: Arc<Listeners>,
    close: Arc<CloseState>,
    window: Arc<OsWindow>,
    maximized: bool,
    presenter: GpuPresenter,
    renderer: RenderWorker,
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
    text_fields: Vec<TextField>,
    #[cfg(feature = "android-apk")]
    android_input_layout: Option<AndroidInputLayout>,
    repaint_at: Option<Instant>,
    opened: Instant,
    /// Egui's last text picture. Reused while the caret does not need a new frame.
    field_scratch: Vec<u32>,
    field_stamp: u64,
    field_until: Option<Instant>,
}

struct RenderJob {
    options: RenderOptions,
    output_width: u32,
    output_height: u32,
    opacity: f32,
    transparent: bool,
}

#[cfg(feature = "android-apk")]
#[derive(Clone, PartialEq)]
struct AndroidInputLayout {
    id: NodeId,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    font_size: f32,
    color: u32,
    password: bool,
    placeholder: String,
}

struct RenderedFrame {
    layout_epoch: u64,
    layout_width: u32,
    layout_height: u32,
    width: u32,
    height: u32,
    pixels: Vec<u32>,
    boxes: Vec<LayoutBox>,
    fields: Vec<TextField>,
    animating: bool,
    render_us: u128,
    error: Option<String>,
}

/// GPU-backed FIFO presentation for the CPU-rendered frame. The renderer may
/// finish early or late, but scanout is paced by the platform swapchain.
struct GpuPresenter {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    texture: wgpu::Texture,
    sampler: wgpu::Sampler,
    bind_group: wgpu::BindGroup,
    opaque_pipeline: wgpu::RenderPipeline,
    alpha_pipeline: wgpu::RenderPipeline,
}

impl GpuPresenter {
    fn new(window: Arc<OsWindow>, width: u32, height: u32, transparent: bool) -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window)
            .map_err(|error| format!("create GPU surface: {error}"))?;
        let adapter = futures_executor::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| "no compatible GPU adapter".to_string())?;
        // `downlevel_defaults` caps textures at 2048px, which is too small for
        // modern phone surfaces (for example, a 1080x2343 display). Keep the
        // conservative limits for other features, but use the actual adapter
        // limit for 2D textures so wgpu can configure the full native surface.
        let mut required_limits = wgpu::Limits::downlevel_defaults();
        required_limits.max_texture_dimension_2d = adapter.limits().max_texture_dimension_2d;
        let (device, queue) = futures_executor::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("sweet compositor"),
                required_features: wgpu::Features::empty(),
                required_limits,
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .map_err(|error| format!("create GPU device: {error}"))?;
        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied()
            .find(|format| *format == wgpu::TextureFormat::Bgra8Unorm)
            .or_else(|| caps.formats.iter().copied().find(|format| !format.is_srgb()))
            .unwrap_or(caps.formats[0]);
        let preferred_alpha = if transparent {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            wgpu::CompositeAlphaMode::Opaque
        };
        let alpha_mode = caps.alpha_modes.iter().copied()
            .find(|mode| *mode == preferred_alpha)
            .unwrap_or(caps.alpha_modes[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 1,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sweet CPU frame upload"),
            size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sweet frame texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sweet frame texture"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&texture_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sweet fullscreen compositor"),
            source: wgpu::ShaderSource::Wgsl(r#"
                struct Varyings { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
                @group(0) @binding(0) var frame_tex: texture_2d<f32>;
                @group(0) @binding(1) var frame_sampler: sampler;
                @vertex fn vs(@builtin(vertex_index) i: u32) -> Varyings {
                    let positions = array<vec2<f32>, 3>(vec2(-1.0,-1.0), vec2(3.0,-1.0), vec2(-1.0,3.0));
                    let p = positions[i];
                    var out: Varyings;
                    out.position = vec4(p, 0.0, 1.0);
                    out.uv = vec2(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
                    return out;
                }
                @fragment fn fs_opaque(in: Varyings) -> @location(0) vec4<f32> {
                    let c = textureSample(frame_tex, frame_sampler, in.uv);
                    return vec4(c.b, c.g, c.r, 1.0);
                }
                @fragment fn fs_alpha(in: Varyings) -> @location(0) vec4<f32> {
                    let c = textureSample(frame_tex, frame_sampler, in.uv);
                    return vec4(c.b, c.g, c.r, c.a);
                }
            "#.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sweet compositor pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let make_pipeline = |entry_point| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sweet compositor pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview: None,
            cache: None,
        });
        let opaque_pipeline = make_pipeline("fs_opaque");
        let alpha_pipeline = make_pipeline("fs_alpha");
        Ok(Self { _instance: instance, surface, device, queue, config, texture, sampler, bind_group, opaque_pipeline, alpha_pipeline })
    }

    fn present(&mut self, window: &OsWindow, width: u32, height: u32, pixels: &[u32], transparent: bool) -> Result<(), String> {
        if self.config.width != width || self.config.height != height {
            self.config.width = width.max(1);
            self.config.height = height.max(1);
            self.surface.configure(&self.device, &self.config);
            self.texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sweet CPU frame upload"),
                size: wgpu::Extent3d { width: self.config.width, height: self.config.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = self.texture.create_view(&wgpu::TextureViewDescriptor::default());
            // The bind group layout is pipeline-owned; recover it from the pipeline.
            let layout = self.opaque_pipeline.get_bind_group_layout(0);
            self.bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sweet frame texture"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
        }
        let bytes = unsafe { std::slice::from_raw_parts(pixels.as_ptr().cast::<u8>(), pixels.len() * 4) };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &self.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        let frame = self.surface.get_current_texture().map_err(|error| format!("acquire GPU frame: {error}"))?;
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("sweet present") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sweet frame composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(if transparent { &self.alpha_pipeline } else { &self.opaque_pipeline });
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        window.pre_present_notify();
        frame.present();
        Ok(())
    }
}

struct RenderExchange {
    pending: Option<RenderJob>,
    ready: Option<RenderedFrame>,
    spare_pixels: Vec<u32>,
    stopped: bool,
}

struct RenderWorker {
    exchange: Arc<(Mutex<RenderExchange>, Condvar)>,
    frame_event_queued: Arc<AtomicBool>,
}

impl RenderWorker {
    fn new(token: u64, doc: Arc<CoreDocument>, proxy: EventLoopProxy<Cmd>) -> Self {
        let exchange = Arc::new((
            Mutex::new(RenderExchange {
                pending: None,
                ready: None,
                spare_pixels: Vec::new(),
                stopped: false,
            }),
            Condvar::new(),
        ));
        let frame_event_queued = Arc::new(AtomicBool::new(false));
        let worker_exchange = Arc::clone(&exchange);
        let worker_queued = Arc::clone(&frame_event_queued);
        thread::Builder::new()
            .name(format!("sweet-render-{token}"))
            .spawn(move || loop {
                let job = {
                    let (lock, wake) = &*worker_exchange;
                    let mut state = lock.lock().expect("render exchange");
                    while state.pending.is_none() && !state.stopped {
                        state = wake.wait(state).expect("render exchange");
                    }
                    if state.stopped {
                        break;
                    }
                    state.pending.take().expect("pending render job")
                };
                let layout_epoch = doc.layout_epoch();
                let (layout_width, layout_height) = physical_size(&job.options);
                let mut pixels = {
                    let (lock, _) = &*worker_exchange;
                    let mut state = lock.lock().expect("render exchange");
                    std::mem::take(&mut state.spare_pixels)
                };
                let layout_count = (layout_width as usize).saturating_mul(layout_height as usize);
                pixels.resize(layout_count, 0);
                let started = Instant::now();
                let rendered = if job.transparent {
                    render_argb(&doc, &job.options, job.opacity, &mut pixels)
                } else {
                    render_xrgb(&doc, &job.options, &mut pixels)
                };
                let render_us = started.elapsed().as_micros();
                let (boxes, fields, animating, error) = match rendered {
                    Ok((boxes, fields, animating)) => (boxes, fields, animating, None),
                    Err(error) => (Vec::new(), Vec::new(), false, Some(error.0)),
                };
                if job.output_width != layout_width || job.output_height != layout_height {
                    let mut output = vec![0xff07_0709; (job.output_width as usize).saturating_mul(job.output_height as usize)];
                    let copy_width = layout_width.min(job.output_width) as usize;
                    let copy_height = layout_height.min(job.output_height) as usize;
                    for row in 0..copy_height {
                        let from = row * layout_width as usize;
                        let to = row * job.output_width as usize;
                        output[to..to + copy_width].copy_from_slice(&pixels[from..from + copy_width]);
                    }
                    pixels = output;
                }
                let frame = RenderedFrame {
                    layout_epoch,
                    layout_width,
                    layout_height,
                    width: job.output_width,
                    height: job.output_height,
                    pixels,
                    boxes,
                    fields,
                    animating,
                    render_us,
                    error,
                };
                {
                    let (lock, _) = &*worker_exchange;
                    let mut state = lock.lock().expect("render exchange");
                    if let Some(superseded) = state.ready.replace(frame) {
                        if superseded.pixels.capacity() > state.spare_pixels.capacity() {
                        state.spare_pixels = superseded.pixels;
                        }
                    }
                }
                if !worker_queued.swap(true, Ordering::AcqRel)
                    && proxy.send_event(Cmd::FrameReady(token)).is_err()
                {
                    worker_queued.store(false, Ordering::Release);
                    break;
                }
            })
            .expect("spawn render worker");
        Self {
            exchange,
            frame_event_queued,
        }
    }

    fn submit(&self, job: RenderJob) {
        let (lock, wake) = &*self.exchange;
        let mut state = lock.lock().expect("render exchange");
        if !state.stopped {
            state.pending = Some(job);
            wake.notify_one();
        }
    }

    fn take_ready(&self) -> Option<RenderedFrame> {
        self.frame_event_queued.store(false, Ordering::Release);
        self.exchange.0.lock().expect("render exchange").ready.take()
    }

    fn recycle_pixels(&self, pixels: Vec<u32>) {
        let (lock, wake) = &*self.exchange;
        let mut state = lock.lock().expect("render exchange");
        if state.spare_pixels.capacity() < pixels.capacity() {
            state.spare_pixels = pixels;
        }
        wake.notify_one();
    }
}

impl Drop for RenderWorker {
    fn drop(&mut self) {
        let (lock, wake) = &*self.exchange;
        let mut state = lock.lock().expect("render exchange");
        state.stopped = true;
        wake.notify_one();
    }
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
    #[cfg(feature = "android-apk")]
    android_app: Option<winit::platform::android::activity::AndroidApp>,
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
                #[cfg(any(target_os = "linux", all(target_os = "android", feature = "termux-x11")))]
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
                    #[cfg(feature = "android-apk")]
                    android_app: None,
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

#[cfg(feature = "android-apk")]
#[no_mangle]
pub fn android_main(android_app: winit::platform::android::activity::AndroidApp) {
    use std::ffi::{CStr, CString};
    use std::path::{Path, PathBuf};
    use winit::platform::android::EventLoopBuilderExtAndroid;

    fn copy_asset_tree(
        manager: &winit::platform::android::activity::ndk::asset::AssetManager,
        source: &str,
        destination: &Path,
    ) -> std::io::Result<()> {
        let source_c = CString::new(source).expect("asset path");
        let Some(directory) = manager.open_dir(&source_c) else {
            return Ok(());
        };
        for name in directory {
            let name = name.to_string_lossy();
            let asset_name = if source.is_empty() {
                name.to_string()
            } else {
                format!("{source}/{name}")
            };
            let output = destination.join(name.as_ref());
            let asset_c = CString::new(asset_name.as_str()).expect("asset path");
            if let Some(mut asset) = manager.open(&asset_c) {
                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut file = std::fs::File::create(output)?;
                std::io::copy(&mut asset, &mut file)?;
            }
        }
        Ok(())
    }

    unsafe extern "C" {
        fn sweet_node_start(script_path: *const std::ffi::c_char) -> i32;
        fn sweet_node_library_path() -> *const std::ffi::c_char;
    }

    let Some(files_dir) = android_app.internal_data_path() else {
        eprintln!("DJ Grok: Android did not provide an internal files directory");
        return;
    };
    let project_dir = files_dir.join("sweet-app");
    if let Err(error) = std::fs::remove_dir_all(&project_dir) {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("DJ Grok: could not reset app assets: {error}");
            return;
        }
    }
    let assets = android_app.asset_manager();
    let copy_assets = copy_asset_tree(&assets, "sweet-app", &project_dir)
        .and_then(|()| copy_asset_tree(&assets, "sweet-app/djgrok", &project_dir.join("djgrok")))
        .and_then(|()| {
            copy_asset_tree(
                &assets,
                "sweet-app/djgrok/covers",
                &project_dir.join("djgrok/covers"),
            )
        });
    if let Err(error) = copy_assets {
        eprintln!("DJ Grok: could not extract app assets: {error}");
        return;
    }

    let home = files_dir.join("home");
    let data_dir = files_dir.join("djgrok-data");
    let _ = std::fs::create_dir_all(&home);
    let _ = std::fs::create_dir_all(&data_dir);
    std::env::set_var("HOME", &home);
    std::env::set_var("DJGROK_DATA_DIR", &data_dir);
    std::env::set_var("SWEET_ANDROID_APK", "1");
    std::env::set_var("SWEET_FONT_DIR", project_dir.join("djgrok"));
    std::env::remove_var("LD_PRELOAD");
    unsafe {
        let library_path = sweet_node_library_path();
        if !library_path.is_null() {
            let path = CStr::from_ptr(library_path).to_string_lossy();
            std::env::set_var("SWEET_NATIVE_FILE", path.as_ref());
        }
    }

    let script_path: PathBuf = project_dir.join("djgrok/title.js");
    let script = match CString::new(script_path.to_string_lossy().as_bytes()) {
        Ok(script) => script,
        Err(_) => {
            eprintln!("DJ Grok: invalid JavaScript entrypoint path");
            return;
        }
    };

    let mut builder = EventLoop::<Cmd>::with_user_event();
    builder.with_android_app(android_app.clone());
    let event_loop = match builder.build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            eprintln!("DJ Grok: could not start Android window loop: {error}");
            return;
        }
    };
    let proxy = event_loop.create_proxy();
    if RUNTIME.set(Ok(proxy.clone())).is_err() {
        eprintln!("DJ Grok: Android runtime was already initialized");
        return;
    }

    if let Err(error) = thread::Builder::new()
        .name("sweet-node".into())
        .spawn(move || unsafe { sweet_node_start(script.as_ptr()) })
    {
        eprintln!("DJ Grok: could not start embedded Node.js: {error}");
        return;
    }

    let mut app = App {
        proxy,
        windows: HashMap::new(),
        tokens: HashMap::new(),
        android_app: Some(android_app),
    };
    if let Err(error) = event_loop.run_app(&mut app) {
        eprintln!("DJ Grok: Android window loop failed: {error}");
    }
    std::process::exit(0);
}

#[cfg(feature = "android-apk")]
fn sync_android_input_field(
    android_app: &winit::platform::android::activity::AndroidApp,
    live: &mut Live,
) {
    use jni::objects::{JObject, JValue};
    let Some(id) = live.focus.filter(|id| live.field_nodes.contains(id)) else {
        live.android_input_layout = None;
        return;
    };
    let Some(field) = live.text_fields.iter().find(|field| field.id == id) else {
        return;
    };
    let scale = live.window.as_ref().scale_factor() as f32;
    let color = ((field.color.a as u32) << 24)
        | ((field.color.r as u32) << 16)
        | ((field.color.g as u32) << 8)
        | field.color.b as u32;
    let layout = AndroidInputLayout {
        id,
        x: field.x,
        y: field.y,
        width: field.width,
        height: field.height,
        font_size: field.font_size,
        color,
        password: field.password,
        placeholder: field.placeholder.clone(),
    };
    if live.android_input_layout.as_ref() == Some(&layout) {
        return;
    }
    let result = (|| -> jni::errors::Result<()> {
        let vm = unsafe { jni::JavaVM::from_raw(android_app.vm_as_ptr().cast())? };
        let mut env = vm.attach_current_thread()?;
        let activity = unsafe {
            JObject::from_raw(android_app.activity_as_ptr() as jni::sys::jobject)
        };
        let value = JObject::from(env.new_string(&field.value)?);
        let placeholder = JObject::from(env.new_string(&field.placeholder)?);
        let call = env.call_method(
            &activity,
            "sweetSetInputField",
            "(Ljava/lang/String;Ljava/lang/String;FFFFFIZ)V",
            &[
                JValue::Object(&value),
                JValue::Object(&placeholder),
                JValue::Float(field.x * scale),
                JValue::Float(field.y * scale),
                JValue::Float(field.width * scale),
                JValue::Float(field.height * scale),
                JValue::Float(field.font_size * scale),
                JValue::Int(color as i32),
                JValue::Bool(field.password.into()),
            ],
        );
        std::mem::forget(activity);
        call?;
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("DJ Grok: could not sync Android input overlay: {error}");
    } else {
        live.android_input_layout = Some(layout);
    }
}

#[cfg(feature = "android-apk")]
#[allow(non_snake_case)]
#[no_mangle]
pub extern "system" fn Java_org_sweet_djgrok_SweetActivity_nativeTextChanged(
    mut env: jni::JNIEnv<'_>,
    _class: jni::objects::JClass<'_>,
    text: jni::objects::JString<'_>,
) {
    let Ok(value): Result<String, _> = env.get_string(&text).map(|value| value.into()) else {
        return;
    };
    let target = ANDROID_INPUT_TARGET.load(Ordering::Acquire);
    if target == u32::MAX {
        return;
    }
    if let Some(Ok(proxy)) = RUNTIME.get() {
        let _ = proxy.send_event(Cmd::AndroidTextChanged { target, value });
    }
}

#[cfg(feature = "android-apk")]
#[allow(non_snake_case)]
#[no_mangle]
pub extern "system" fn Java_org_sweet_djgrok_SweetActivity_nativeViewportChanged(
    _env: jni::JNIEnv<'_>,
    _class: jni::objects::JClass<'_>,
    height: jni::sys::jint,
) {
    let height = height.max(1) as u32;
    ANDROID_VIEWPORT_HEIGHT.store(height, Ordering::Release);
    if let Some(Ok(proxy)) = RUNTIME.get() {
        let _ = proxy.send_event(Cmd::AndroidViewportChanged(height));
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
                        // DOM/video producers can wake faster than the display. Fold their
                        // latest state into one frame, without disturbing the animation clock.
                        let next = next_frame(live.opened, Instant::now());
                        live.repaint_at = Some(live.repaint_at.map_or(next, |at| at.min(next)));
                    }
                }
            }
            Cmd::FrameReady(token) => {
                #[cfg(feature = "android-apk")]
                let android_app = self.android_app.clone();
                if let Some(id) = self.tokens.get(&token).copied() {
                    if let Some(live) = self.windows.get_mut(&id) {
                        if let Some(frame) = live.renderer.take_ready() {
                            if let Err(error) = present_frame(live, frame) {
                                eprintln!("sweet frame presentation failed: {error}");
                            } else {
                                #[cfg(feature = "android-apk")]
                                if let Some(android_app) = android_app.as_ref() {
                                    sync_android_input_field(android_app, live);
                                }
                            }
                        }
                    }
                }
            }
            Cmd::Minimize(token) => {
                if let Some(id) = self.tokens.get(&token).copied() {
                    if let Some(live) = self.windows.get(&id) {
                        live.window.as_ref().set_minimized(true);
                    }
                }
            }
            Cmd::ToggleMaximize(token) => {
                if let Some(id) = self.tokens.get(&token).copied() {
                    if let Some(live) = self.windows.get(&id) {
                        let window = live.window.as_ref();
                        window.set_maximized(!window.is_maximized());
                    }
                }
            }
            Cmd::SetResizable(token, resizable) => {
                if let Some(id) = self.tokens.get(&token).copied() {
                    if let Some(live) = self.windows.get_mut(&id) {
                        live.resizable = resizable;
                        live.window.as_ref().set_resizable(resizable);
                        if !resizable {
                            live.resize_drag = None;
                            live.window.as_ref().set_cursor(CursorIcon::Default);
                            live.cursor = CursorIcon::Default;
                        }
                    }
                }
            }
            Cmd::Close(token) => {
                if let Some(id) = self.tokens.get(&token).copied() {
                    self.destroy(id);
                    #[cfg(feature = "android-apk")]
                    if self.windows.is_empty() {
                        event_loop.exit();
                    }
                }
            }
            #[cfg(feature = "android-apk")]
            Cmd::AndroidTextChanged { target, value } => {
                if let Some(live) = self.windows.values_mut().next() {
                    let id = NodeId(target);
                    let previous = live.doc.control_value(id).unwrap_or_default();
                    let updated = live.doc.set_control_value(id, &value);
                    if updated.is_ok() && previous != value {
                        dispatch(
                            live,
                            "input",
                            Some(id),
                            0.0,
                            0.0,
                            ThreadsafeFunctionCallMode::NonBlocking,
                            None,
                            (0.0, 0.0),
                        );
                        live.window.as_ref().request_redraw();
                    }
                }
            }
            #[cfg(feature = "android-apk")]
            Cmd::AndroidViewportChanged(height) => {
                if let Some(live) = self.windows.values_mut().next() {
                    let size = android_layout_size(live.window.as_ref());
                    let scale = live.window.as_ref().scale_factor().max(0.01);
                    dispatch(
                        live,
                        "resize",
                        None,
                        size.width as f64 / scale,
                        height as f64 / scale,
                        ThreadsafeFunctionCallMode::NonBlocking,
                        None,
                        (0.0, 0.0),
                    );
                    live.window.as_ref().request_redraw();
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        // Android reports touchscreen input as Touch events. Sweet's DOM and
        // text-field focus code consume mouse events, so translate a single
        // touch into the equivalent pointer sequence before normal handling.
        #[cfg(target_os = "android")]
        if let WindowEvent::Touch(touch) = event {
            let device_id = touch.device_id;
            let position = touch.location;
            let cursor = || WindowEvent::CursorMoved { device_id, position };
            match touch.phase {
                winit::event::TouchPhase::Started => {
                    self.window_event(event_loop, window_id, cursor());
                    self.window_event(
                        event_loop,
                        window_id,
                        WindowEvent::MouseInput {
                            device_id,
                            state: ElementState::Pressed,
                            button: MouseButton::Left,
                        },
                    );
                }
                winit::event::TouchPhase::Moved => {
                    self.window_event(event_loop, window_id, cursor());
                }
                winit::event::TouchPhase::Ended => {
                    self.window_event(event_loop, window_id, cursor());
                    self.window_event(
                        event_loop,
                        window_id,
                        WindowEvent::MouseInput {
                            device_id,
                            state: ElementState::Released,
                            button: MouseButton::Left,
                        },
                    );
                }
                winit::event::TouchPhase::Cancelled => {
                    if let Some(live) = self.windows.get_mut(&window_id) {
                        live.pressed = None;
                    }
                    self.window_event(
                        event_loop,
                        window_id,
                        WindowEvent::MouseInput {
                            device_id,
                            state: ElementState::Released,
                            button: MouseButton::Left,
                        },
                    );
                }
            }
            return;
        }
        if matches!(event, WindowEvent::CloseRequested) {
            self.destroy(window_id);
            #[cfg(feature = "android-apk")]
            if self.windows.is_empty() {
                event_loop.exit();
            }
            return;
        }
        let Some(live) = self.windows.get_mut(&window_id) else {
            return;
        };
        if let Some(editor) = live.fields.as_mut() {
            let repaint = editor.on_event(live.window.as_ref(), &event);
            // egui answers RedrawRequested with repaint=true. Honoring that
            // queues the next frame immediately, so animation runs as fast as
            // paint will go and the motion stutters like a CSS timer.
            if repaint && !matches!(event, WindowEvent::RedrawRequested) {
                live.window.as_ref().request_redraw();
            }
        }
        match event {
            WindowEvent::RedrawRequested => {
                live.painted = true;
                submit_render(live);
            }
            WindowEvent::Resized(size) => {
                update_maximized(live);
                #[cfg(feature = "android-apk")]
                let size = android_layout_size(live.window.as_ref());
                let logical = size.to_logical::<f64>(live.window.as_ref().scale_factor());
                dispatch(
                    live,
                    "resize",
                    None,
                    logical.width,
                    logical.height,
                    ThreadsafeFunctionCallMode::NonBlocking,
                    None,
                    (0.0, 0.0),
                );
                live.window.as_ref().request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                update_maximized(live);
                let size = current_layout_size(live.window.as_ref());
                let logical = size.to_logical::<f64>(live.window.as_ref().scale_factor());
                dispatch(
                    live,
                    "resize",
                    None,
                    logical.width,
                    logical.height,
                    ThreadsafeFunctionCallMode::NonBlocking,
                    None,
                    (0.0, 0.0),
                );
                live.window.as_ref().request_redraw();
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
                    live.window.as_ref().set_cursor(cursor);
                }
                let hover_changed = hit != live.hover;
                if hover_changed {
                    dispatch_mouseleave(live, live.hover, hit, x, y);
                    live.hover = hit;
                    live.window.as_ref().request_redraw();
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
                        (0.0, 0.0),
                    );
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scale = live.window.as_ref().scale_factor() as f32;
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 40.0, y * 40.0),
                    MouseScrollDelta::PixelDelta(position) => {
                        (position.x as f32 / scale, position.y as f32 / scale)
                    }
                };
                let (x, y, hit) = at_pointer(live);
                if live.listeners.wants("wheel") {
                    dispatch(
                        live,
                        "wheel",
                        hit,
                        x,
                        y,
                        ThreadsafeFunctionCallMode::NonBlocking,
                        None,
                        (dx as f64, dy as f64),
                    );
                }
                scroll_at_pointer(live, dx, dy);
            }
            WindowEvent::CursorLeft { .. } => {
                dispatch_mouseleave(live, live.hover, None, live.last_x, live.last_y);
                live.hover = None;
                live.window.as_ref().set_cursor(CursorIcon::Default);
                live.window.as_ref().request_redraw();
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
                                let window = live.window.as_ref();
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
                            } else if live.window.as_ref().drag_resize_window(direction).is_ok() {
                                return;
                            }
                        }
                        if live.drag_anywhere && !blocks_window_drag(live, hit) {
                            live.pressed = None;
                            if live.window.as_ref().drag_window().is_ok() {
                                return;
                            }
                        }
                        // A paint here runs before mouseup is delivered, so the
                        // click handler cannot start until the frame finishes.
                        let previous_focus = live.focus;
                        if live.focus != hit {
                            live.focus_frame = true;
                        }
                        live.pressed = hit;
                        live.focus = hit;
                        #[cfg(feature = "android-apk")]
                        ANDROID_INPUT_TARGET.store(
                            hit.filter(|id| live.field_nodes.contains(id))
                                .map(|id| id.0)
                                .unwrap_or(u32::MAX),
                            Ordering::Release,
                        );
                        #[cfg(feature = "android-apk")]
                        if hit.is_some_and(|id| live.field_nodes.contains(&id)) {
                            live.window.as_ref().set_ime_allowed(true);
                        } else if previous_focus
                            .is_some_and(|id| live.field_nodes.contains(&id))
                        {
                            live.window.as_ref().set_ime_allowed(false);
                        }
                        dispatch(
                            live,
                            "mousedown",
                            hit,
                            x,
                            y,
                            ThreadsafeFunctionCallMode::NonBlocking,
                            None,
                            (0.0, 0.0),
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
                            (0.0, 0.0),
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
                                (0.0, 0.0),
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
                    live.window.as_ref().request_redraw();
                    live.repaint_at = None;
                } else {
                    soonest = Some(soonest.map_or(at, |soon| soon.min(at)));
                }
            }
            // X11 delivers redraws after pointer events. Asking at the end of
            // a turn that did not already paint keeps the click ahead of the
            // frame, and folds it into a DOM redraw queued by the handler.
            if live.focus_frame && !live.painted {
                live.window.as_ref().request_redraw();
            }
            live.focus_frame = false;
            live.painted = false;
        }
        event_loop.set_control_flow(match soonest {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
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
            let window = Arc::new(event_loop
                .create_window(attrs)
                .map_err(|err| err.to_string())?);
            let id = window.id();
            let initial_size = window.inner_size();
            let maximized = window.is_maximized();
            let presenter = GpuPresenter::new(
                Arc::clone(&window), initial_size.width, initial_size.height, borderless,
            )?;
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
            let renderer = RenderWorker::new(token, Arc::clone(&doc), self.proxy.clone());
            let mut live = Live {
                token,
                doc,
                listeners,
                close,
                window,
                maximized,
                presenter,
                renderer,
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
                text_fields: Vec::new(),
                #[cfg(feature = "android-apk")]
                android_input_layout: None,
                cursor: CursorIcon::Default,
                last_x: 0.0,
                last_y: 0.0,
                redraw_queued,
                focus_frame: false,
                painted: false,
                fields: Some(fields::Fields::new(window.as_ref())),
                repaint_at: None,
                opened: Instant::now(),
                field_scratch: Vec::new(),
                field_stamp: 0,
                field_until: None,
            };
            self.tokens.insert(token, id);
            self.windows.insert(id, live);
            if let Some(live) = self.windows.get(&id) {
                live.window.as_ref().request_redraw();
            }
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
            (0.0, 0.0),
        );
        live.close.signal();
    }
}

fn update_maximized(live: &mut Live) {
    set_maximized(live, live.window.is_maximized());
}

fn set_maximized(live: &mut Live, maximized: bool) {
    if live.maximized == maximized {
        return;
    }
    live.maximized = maximized;
    dispatch(
        live,
        "maximize",
        None,
        if maximized { 1.0 } else { 0.0 },
        0.0,
        ThreadsafeFunctionCallMode::NonBlocking,
        None,
        (0.0, 0.0),
    );
}

fn submit_render(live: &mut Live) {
    let (scale, physical, output) = {
        let window = live.window.as_ref();
        let physical = current_layout_size(window);
        let output = if cfg!(target_os = "android") {
            window.outer_size()
        } else {
            physical
        };
        (window.scale_factor().max(0.01), physical, output)
    };
    if physical.width == 0 || physical.height == 0 {
        return;
    }
    let options = RenderOptions {
        width: physical.width as f32 / scale as f32,
        height: physical.height as f32 / scale as f32,
        scale: scale as f32,
        hover: live.hover,
        focus: live.focus,
        time: frame_clock(live.opened, Instant::now()),
    };
    live.renderer.submit(RenderJob {
        options,
        output_width: output.width,
        output_height: output.height,
        opacity: live.opacity,
        transparent: live.transparent,
    });
}

fn present_frame(live: &mut Live, mut frame: RenderedFrame) -> Result<(), String> {
    if let Some(error) = frame.error.take() {
        live.renderer.recycle_pixels(frame.pixels);
        return Err(error);
    }
    let physical = current_layout_size(live.window.as_ref());
    if physical.width != frame.layout_width
        || physical.height != frame.layout_height
        || live.doc.layout_epoch() != frame.layout_epoch
    {
        live.renderer.recycle_pixels(frame.pixels);
        live.window.as_ref().request_redraw();
        return Ok(());
    }
    let paint_us = frame.render_us;
    let text_fields = frame.fields;
    live.text_fields = text_fields.clone();
    let animating = frame.animating;
    live.boxes = frame.boxes;
    let (frame_w, frame_h) = (frame.width, frame.height);
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
        let updates = if let Some(editor) = live.fields.as_mut() {
            live.field_scratch.fill(0);
            let updates = editor.run(
                live.window.as_ref(),
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
        blend_fields(
            &mut frame.pixels,
            &live.field_scratch,
            &text_fields,
            frame_w,
            frame_h,
        );
        updates
    } else {
        blend_fields(
            &mut frame.pixels,
            &live.field_scratch,
            &text_fields,
            frame_w,
            frame_h,
        );
        Vec::new()
    };
    live.presenter.present(
        live.window.as_ref(), frame_w, frame_h, &frame.pixels, live.transparent,
    )?;
    live.renderer.recycle_pixels(frame.pixels);
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
                (0.0, 0.0),
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
                (0.0, 0.0),
            );
        }
    }
    Ok(())
}

fn current_layout_size(window: &OsWindow) -> PhysicalSize<u32> {
    #[cfg(feature = "android-apk")]
    {
        let mut size = window.inner_size();
        let height = ANDROID_VIEWPORT_HEIGHT.load(Ordering::Acquire);
        if height > 0 {
            size.width = window.outer_size().width;
            size.height = height;
        }
        size
    }
    #[cfg(not(feature = "android-apk"))]
    {
        window.inner_size()
    }
}

#[cfg(feature = "android-apk")]
fn android_layout_size(window: &OsWindow) -> PhysicalSize<u32> {
    current_layout_size(window)
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
    let scale = live.window.as_ref().scale_factor().max(0.01);
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
    let window = live.window.as_ref();
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
    let window = live.window.as_ref();
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
                (0.0, 0.0),
            );
            live.window.as_ref().request_redraw();
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
        .any(|(id, event)| id.is_none() && is_pointer_interaction(event))
    {
        return true;
    }
    let mut current = target;
    let mut guard = 0usize;
    while let Some(id) = current {
        if live.field_nodes.contains(&id)
            || map
                .keys()
                .any(|(target, event)| *target == Some(id.0) && is_pointer_interaction(event))
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

fn is_pointer_interaction(event: &str) -> bool {
    matches!(event, "click" | "mousedown" | "mouseup" | "mousemove" | "wheel" | "scroll")
}

fn dispatch(
    live: &Live,
    event: &str,
    target: Option<NodeId>,
    x: f64,
    y: f64,
    mode: ThreadsafeFunctionCallMode,
    key: Option<&str>,
    delta: (f64, f64),
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
            delta_x: delta.0,
            delta_y: delta.1,
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

fn dispatch_mouseleave(
    live: &Live,
    from: Option<NodeId>,
    to: Option<NodeId>,
    x: f64,
    y: f64,
) {
    let targets = {
        let map = live.listeners.map.lock().expect("listeners");
        map.keys()
            .filter_map(|(id, event)| {
                let id = (*id)?;
                (event == "mouseleave"
                    && is_within(live, from, id)
                    && !is_within(live, to, id))
                .then_some(id)
            })
            .collect::<Vec<_>>()
    };
    for id in targets {
        let listeners = {
            let map = live.listeners.map.lock().expect("listeners");
            map.get(&(Some(id), "mouseleave".to_string()))
                .cloned()
                .unwrap_or_default()
        };
        for listener in listeners {
            let event = UiEvent {
                kind: "mouseleave".to_string(),
                node_id: Some(id),
                x,
                y,
                delta_x: 0.0,
                delta_y: 0.0,
                key: None,
            };
            let _ = listener.call(event, ThreadsafeFunctionCallMode::NonBlocking);
        }
    }
}

fn is_within(live: &Live, target: Option<NodeId>, ancestor: u32) -> bool {
    let mut current = target;
    let mut guard = 0usize;
    while let Some(id) = current {
        if id.0 == ancestor {
            return true;
        }
        current = live
            .boxes
            .iter()
            .rev()
            .find(|layout| layout.id == id)
            .and_then(|layout| layout.parent);
        guard += 1;
        if guard > 4096 {
            break;
        }
    }
    false
}

