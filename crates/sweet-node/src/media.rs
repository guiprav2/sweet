//! Pure-Rust video pipeline.
//!
//! JavaScript keeps the DOM video surface (`play`, `pause`, `currentTime`,
//! events). This module owns the work that used to be `ffmpeg` and `mpv`:
//! resolve a local file, direct URL, or YouTube page to an MP4/MKV, decode
//! frames with `yscv-video`, and play the audio track with `symphonia` into
//! `rodio`.
//!
//! `yscv-video`'s reader is file-oriented and seeks, so remote sources are
//! downloaded to a temp file first. YouTube is resolved with the standalone
//! `yt-dlp` binary (cached on first use, the same strategy the yt-dlp crate
//! uses) and only used to obtain a direct stream URL. Decoding after that
//! does not call ffmpeg, ffprobe, or mpv.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

static TRACE_ORIGIN: OnceLock<Instant> = OnceLock::new();
static TRACE_ENABLED: OnceLock<bool> = OnceLock::new();

pub(crate) fn trace(message: impl std::fmt::Display) {
    if !*TRACE_ENABLED.get_or_init(|| std::env::var_os("SWEET_MEDIA_TRACE").is_some()) {
        return;
    }
    let elapsed = TRACE_ORIGIN.get_or_init(Instant::now).elapsed();
    let thread = thread::current();
    eprintln!(
        "[sweet-media +{:>8.3}s {}] {message}",
        elapsed.as_secs_f64(),
        thread.name().unwrap_or("unnamed")
    );
}

use sweet_core::{Document as CoreDocument, NodeId};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::audio::Channels;
use symphonia::core::codecs::{CodecParameters, CODEC_TYPE_AAC, CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::formats::{FormatOptions, Packet};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use yscv_video::audio::AudioTrackInfo;
use yscv_video::mkv::{MkvCodec, MkvDemuxer};
use yscv_video::{
    Av1Decoder, DecodedFrame, H264Decoder, HevcDecoder, Mp4VideoReader, NalUnit, NalUnitType,
};

/// Preview size the old ffmpeg filter painted. Tailwind still scales the box.
const FRAME_WIDTH: u32 = 480;
const FRAME_HEIGHT: u32 = 270;
const TARGET_FPS: f64 = 12.0;
const STREAM_QUEUE_CAPACITY: usize = 48;
const STREAM_QUEUE_MAX_BYTES: usize = 24 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
enum StreamCodec {
    H264,
    Hevc,
    Av1,
}

impl StreamCodec {
    fn parse(codec: &str) -> Result<Self, String> {
        match codec.to_ascii_lowercase().as_str() {
            "h264" | "avc1" => Ok(Self::H264),
            "hevc" | "h265" | "hvc1" | "hev1" => Ok(Self::Hevc),
            "av1" | "av01" => Ok(Self::Av1),
            _ => Err(format!("unsupported streaming video codec: {codec}")),
        }
    }
}

struct StreamPacket {
    epoch: u64,
    data: Vec<u8>,
    timestamp: f64,
    keyframe: bool,
}

struct StreamQueueState {
    codec: Option<StreamCodec>,
    codec_private: Vec<u8>,
    packets: std::collections::VecDeque<StreamPacket>,
    queued_bytes: usize,
    epoch: u64,
    target_time: f64,
    ended: bool,
}

struct StreamQueue {
    state: Mutex<StreamQueueState>,
    ready: Condvar,
}

struct StreamAudioConfig {
    codec: String,
    extra_data: Vec<u8>,
    sample_rate: u32,
    channels: u16,
    time_scale: u32,
}

struct StreamAudioPacket {
    epoch: u64,
    data: Vec<u8>,
    timestamp: f64,
    duration: f64,
}

struct StreamAudioState {
    config: Option<StreamAudioConfig>,
    packets: std::collections::VecDeque<StreamAudioPacket>,
    epoch: u64,
    target_time: f64,
    ended: bool,
}

struct StreamAudioQueue {
    state: Mutex<StreamAudioState>,
    ready: Condvar,
}

impl StreamAudioQueue {
    fn new() -> Self {
        Self {
            state: Mutex::new(StreamAudioState {
                config: None,
                packets: std::collections::VecDeque::new(),
                epoch: 0,
                target_time: 0.0,
                ended: false,
            }),
            ready: Condvar::new(),
        }
    }

    fn pop(&self, stop: &AtomicBool) -> Option<StreamAudioPacket> {
        let mut state = self.state.lock().expect("audio stream queue");
        loop {
            if stop.load(Ordering::Acquire) {
                return None;
            }
            if let Some(packet) = state.packets.pop_front() {
                self.ready.notify_all();
                return Some(packet);
            }
            if state.ended {
                return None;
            }
            let (next, _) = self
                .ready
                .wait_timeout(state, Duration::from_millis(100))
                .expect("audio stream queue");
            state = next;
        }
    }
}

impl StreamQueue {
    fn new() -> Self {
        Self {
            state: Mutex::new(StreamQueueState {
                codec: None,
                codec_private: Vec::new(),
                packets: std::collections::VecDeque::new(),
                queued_bytes: 0,
                epoch: 0,
                target_time: 0.0,
                ended: false,
            }),
            ready: Condvar::new(),
        }
    }

    fn pop(&self, stop: &AtomicBool) -> Option<StreamPacket> {
        let mut state = self.state.lock().expect("video stream queue");
        loop {
            if stop.load(Ordering::Acquire) {
                return None;
            }
            if let Some(packet) = state.packets.pop_front() {
                state.queued_bytes = state.queued_bytes.saturating_sub(packet.data.len());
                self.ready.notify_all();
                return Some(packet);
            }
            if state.ended {
                return None;
            }
            let (next, _) = self
                .ready
                .wait_timeout(state, Duration::from_millis(100))
                .expect("video stream queue");
            state = next;
        }
    }
}

#[derive(Clone, Debug)]
pub struct MediaInfo {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub path: PathBuf,
    /// Separate audio file when the container has no playable audio track.
    pub audio_path: Option<PathBuf>,
}

pub struct ResolvedSource {
    pub info: MediaInfo,
    pub temp: Option<tempfile::TempDir>,
}

struct Clock {
    playing: bool,
    started_at: Option<Instant>,
    start_time: f64,
    duration: f64,
    volume: f32,
    muted: bool,
    ended: bool,
    width: u32,
    height: u32,
    error: Option<String>,
}

impl Clock {
    fn new() -> Self {
        Self {
            playing: false,
            started_at: None,
            start_time: 0.0,
            duration: f64::NAN,
            volume: 1.0,
            muted: false,
            ended: false,
            width: 0,
            height: 0,
            error: None,
        }
    }

    fn current_time(&self) -> f64 {
        if self.playing {
            if let Some(started) = self.started_at {
                return (self.start_time + started.elapsed().as_secs_f64()).max(0.0);
            }
        }
        self.start_time.max(0.0)
    }
}

/// One video element. The worker thread decodes; Node only calls the controls.
pub struct Player {
    doc: Arc<CoreDocument>,
    node: NodeId,
    generation: Arc<AtomicU64>,
    worker_stop: Option<Arc<AtomicBool>>,
    clock: Arc<Mutex<Clock>>,
    frames: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
    owned_temp: Option<Arc<tempfile::TempDir>>,
    path: Option<PathBuf>,
    audio_path: Option<PathBuf>,
    stream_queue: Arc<StreamQueue>,
    stream_audio_queue: Arc<StreamAudioQueue>,
    stream_mode: bool,
}

impl Player {
    pub fn new(doc: Arc<CoreDocument>, node: NodeId) -> Self {
        Self {
            doc,
            node,
            generation: Arc::new(AtomicU64::new(0)),
            worker_stop: None,
            clock: Arc::new(Mutex::new(Clock::new())),
            frames: Arc::new(AtomicU64::new(0)),
            worker: None,
            owned_temp: None,
            path: None,
            audio_path: None,
            stream_queue: Arc::new(StreamQueue::new()),
            stream_audio_queue: Arc::new(StreamAudioQueue::new()),
            stream_mode: false,
        }
    }

    pub fn configure_stream(
        &mut self,
        codec: &str,
        codec_private: Vec<u8>,
        duration: f64,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let codec = StreamCodec::parse(codec)?;
        self.stop_playback(true);
        self.owned_temp = None;
        let mut stream = self.stream_queue.state.lock().expect("video stream queue");
        stream.codec = Some(codec);
        stream.codec_private = codec_private;
        stream.packets.clear();
        stream.queued_bytes = 0;
        stream.epoch = stream.epoch.wrapping_add(1);
        stream.target_time = 0.0;
        stream.ended = false;
        drop(stream);
        self.stream_mode = true;
        self.path = None;
        self.audio_path = None;
        {
            let mut audio = self.stream_audio_queue.state.lock().expect("audio stream queue");
            audio.config = None;
            audio.packets.clear();
            audio.epoch = audio.epoch.wrapping_add(1);
            audio.ended = false;
        }
        let mut clock = self.clock.lock().expect("media clock");
        clock.duration = duration;
        clock.width = width;
        clock.height = height;
        clock.ended = false;
        clock.error = None;
        clock.playing = false;
        clock.started_at = None;
        clock.start_time = 0.0;
        self.frames.store(0, Ordering::Release);
        trace(format_args!("configured streaming decoder codec={codec:?} size={width}x{height} duration={duration:.3}s"));
        Ok(())
    }

    pub fn enqueue_stream_chunk(
        &self,
        data: Vec<u8>,
        timestamp: f64,
        keyframe: bool,
        reset_queue: bool,
        target_time: f64,
    ) -> Result<bool, String> {
        let mut stream = self.stream_queue.state.lock().expect("video stream queue");
        if stream.codec.is_none() {
            return Err("video stream is not configured".into());
        }
        if reset_queue {
            stream.packets.clear();
            stream.queued_bytes = 0;
            stream.epoch = stream.epoch.wrapping_add(1);
            stream.target_time = target_time.max(0.0);
            stream.ended = false;
            trace(format_args!("stream queue reset epoch={} target={:.3}s", stream.epoch, stream.target_time));
        } else if stream.packets.len() >= STREAM_QUEUE_CAPACITY
            || (stream.queued_bytes.saturating_add(data.len()) > STREAM_QUEUE_MAX_BYTES && !stream.packets.is_empty())
        {
            return Ok(false);
        }
        let epoch = stream.epoch;
        stream.queued_bytes = stream.queued_bytes.saturating_add(data.len());
        stream.packets.push_back(StreamPacket { epoch, data, timestamp, keyframe });
        self.stream_queue.ready.notify_all();
        Ok(true)
    }

    pub fn configure_stream_audio(
        &self,
        codec: &str,
        extra_data: Vec<u8>,
        sample_rate: u32,
        channels: u16,
        time_scale: u32,
    ) -> Result<(), String> {
        if !codec.eq_ignore_ascii_case("aac") && !codec.eq_ignore_ascii_case("mp4a") {
            return Err(format!("unsupported streaming audio codec: {codec}"));
        }
        let mut audio = self.stream_audio_queue.state.lock().expect("audio stream queue");
        audio.config = Some(StreamAudioConfig {
            codec: codec.to_string(),
            extra_data,
            sample_rate,
            channels,
            time_scale: time_scale.max(1),
        });
        audio.packets.clear();
        audio.epoch = audio.epoch.wrapping_add(1);
        audio.target_time = 0.0;
        audio.ended = false;
        Ok(())
    }

    pub fn enqueue_stream_audio(
        &self,
        data: Vec<u8>,
        timestamp: f64,
        duration: f64,
        reset_queue: bool,
        target_time: f64,
    ) -> Result<bool, String> {
        let mut audio = self.stream_audio_queue.state.lock().expect("audio stream queue");
        if audio.config.is_none() {
            return Err("video stream audio is not configured".into());
        }
        if reset_queue {
            audio.packets.clear();
            audio.epoch = audio.epoch.wrapping_add(1);
            audio.target_time = target_time.max(0.0);
            audio.ended = false;
        } else if audio.packets.len() >= STREAM_QUEUE_CAPACITY {
            return Ok(false);
        }
        let epoch = audio.epoch;
        audio.packets.push_back(StreamAudioPacket { epoch, data, timestamp, duration });
        self.stream_audio_queue.ready.notify_all();
        Ok(true)
    }

    pub fn finish_stream(&self) {
        let mut stream = self.stream_queue.state.lock().expect("video stream queue");
        stream.ended = true;
        self.stream_queue.ready.notify_all();
        let mut audio = self.stream_audio_queue.state.lock().expect("audio stream queue");
        audio.ended = true;
        self.stream_audio_queue.ready.notify_all();
    }

    pub fn stream_queue_len(&self) -> usize {
        self.stream_queue.state.lock().expect("video stream queue").packets.len()
    }

    pub fn stream_audio_queue_len(&self) -> usize {
        self.stream_audio_queue.state.lock().expect("audio stream queue").packets.len()
    }

    pub fn install(&mut self, resolved: ResolvedSource) -> MediaInfo {
        trace(format_args!("installing source {}", resolved.info.path.display()));
        self.stop_playback(true);
        self.owned_temp = None;
        let ResolvedSource { info, temp } = resolved;
        self.owned_temp = temp.map(Arc::new);
        self.path = Some(info.path.clone());
        self.audio_path = info.audio_path.clone();
        self.stream_mode = false;
        {
            let mut stream = self.stream_queue.state.lock().expect("video stream queue");
            stream.codec = None;
            stream.codec_private.clear();
            stream.packets.clear();
            stream.queued_bytes = 0;
            stream.epoch = stream.epoch.wrapping_add(1);
            stream.ended = false;
        }
        {
            let mut audio = self.stream_audio_queue.state.lock().expect("audio stream queue");
            audio.config = None;
            audio.packets.clear();
            audio.epoch = audio.epoch.wrapping_add(1);
            audio.ended = false;
        }
        let mut clock = self.clock.lock().expect("media clock");
        clock.duration = info.duration;
        clock.width = info.width;
        clock.height = info.height;
        clock.ended = false;
        clock.error = None;
        clock.playing = false;
        clock.started_at = None;
        clock.start_time = 0.0;
        drop(clock);
        self.frames.store(0, Ordering::Release);
        trace(format_args!(
            "source installed node={} path={} duration={:.3}s size={}x{} separate_audio={}",
            self.node.0,
            info.path.display(),
            info.duration,
            info.width,
            info.height,
            info.audio_path.is_some()
        ));
        info
    }

    pub fn play(&mut self, start: f64) -> Result<(), String> {
        trace(format_args!("play requested node={} start={start:.3}s", self.node.0));
        if self.stream_mode {
            return self.play_stream(start);
        }
        let path = self
            .path
            .clone()
            .ok_or_else(|| "video has no resolved source".to_string())?;
        if !path.is_file() {
            trace(format_args!("play failed: source file is missing {}", path.display()));
            return Err(format!("video file not found: {}", path.display()));
        }
        self.stop_playback(false);
        let stop = Arc::new(AtomicBool::new(false));
        self.worker_stop = Some(Arc::clone(&stop));
        let generation = self.generation.load(Ordering::Acquire);
        {
            let mut clock = self.clock.lock().expect("media clock");
            clock.playing = true;
            clock.ended = false;
            clock.error = None;
            clock.start_time = start.max(0.0);
            clock.started_at = Some(Instant::now());
        }
        let clock = Arc::clone(&self.clock);
        let current_generation = Arc::clone(&self.generation);
        let frames = Arc::clone(&self.frames);
        let doc = Arc::clone(&self.doc);
        let node = self.node;
        let audio_path = self.audio_path.clone();
        let temp_guard = self.owned_temp.clone();
        trace(format_args!(
            "starting decoder node={} generation={} path={} audio={:?}",
            self.node.0,
            generation,
            path.display(),
            audio_path
        ));
        self.worker = Some(
            thread::Builder::new()
                .name("sweet-media".into())
                .spawn(move || {
                    decode_loop(
                        doc,
                        node,
                        path,
                        audio_path,
                        temp_guard,
                        start.max(0.0),
                        stop,
                        current_generation,
                        generation,
                        clock,
                        frames,
                    )
                })
                .map_err(|err| err.to_string())?,
        );
        trace(format_args!("decoder thread spawned node={} generation={generation}", self.node.0));
        Ok(())
    }

    fn play_stream(&mut self, start: f64) -> Result<(), String> {
        self.stop_playback(false);
        {
            let mut stream = self.stream_queue.state.lock().expect("video stream queue");
            stream.packets.clear();
            stream.queued_bytes = 0;
            stream.epoch = stream.epoch.wrapping_add(1);
            stream.target_time = start.max(0.0);
            stream.ended = false;
        }
        self.reset_audio_queue(start.max(0.0));
        let stop = Arc::new(AtomicBool::new(false));
        self.worker_stop = Some(Arc::clone(&stop));
        let generation = self.generation.load(Ordering::Acquire);
        {
            let mut clock = self.clock.lock().expect("media clock");
            clock.playing = true;
            clock.ended = false;
            clock.error = None;
            clock.start_time = start.max(0.0);
            clock.started_at = Some(Instant::now());
        }
        let queue = Arc::clone(&self.stream_queue);
        let audio_queue = Arc::clone(&self.stream_audio_queue);
        let doc = Arc::clone(&self.doc);
        let clock = Arc::clone(&self.clock);
        let frames = Arc::clone(&self.frames);
        let current_generation = Arc::clone(&self.generation);
        let node = self.node;
        self.worker = Some(
            thread::Builder::new()
                .name("sweet-video-stream".into())
                .spawn(move || {
                    decode_stream_loop(
                        doc,
                        node,
                        queue,
                        audio_queue,
                        start.max(0.0),
                        stop,
                        current_generation,
                        generation,
                        clock,
                        frames,
                    )
                })
                .map_err(|err| err.to_string())?,
        );
        trace(format_args!("stream decoder thread spawned node={} generation={generation}", self.node.0));
        Ok(())
    }

    pub fn pause(&mut self) {
        let time = self.current_time();
        self.seek_paused(time);
    }

    pub fn seek_paused(&mut self, seconds: f64) {
        if self.stream_mode {
            self.reset_stream_queue(seconds);
            self.reset_audio_queue(seconds);
        }
        self.stop_playback(false);
        let mut clock = self.clock.lock().expect("media clock");
        clock.playing = false;
        clock.started_at = None;
        clock.ended = false;
        clock.start_time = seconds.max(0.0);
    }

    pub fn stop_playback(&mut self, clear_frame: bool) {
        let old_generation = self.generation.fetch_add(1, Ordering::AcqRel);
        trace(format_args!(
            "stop requested node={} generation={} clear_frame={clear_frame}",
            self.node.0, old_generation
        ));
        if let Some(stop) = self.worker_stop.take() {
            stop.store(true, Ordering::Release);
        }
        {
            let mut clock = self.clock.lock().expect("media clock");
            let time = clock.current_time();
            clock.playing = false;
            clock.started_at = None;
            clock.start_time = time;
        }
        // Dropping JoinHandle detaches the worker. It observes its own stop
        // token, so media controls never wait for a slow codec or audio sink.
        self.worker.take();
        if clear_frame {
            let _ = self.doc.clear_image(self.node);
        }
    }

    pub fn set_volume(&self, volume: f64) {
        self.clock.lock().expect("media clock").volume = volume.clamp(0.0, 1.0) as f32;
    }

    pub fn set_muted(&self, muted: bool) {
        self.clock.lock().expect("media clock").muted = muted;
    }

    pub fn current_time(&self) -> f64 {
        self.clock.lock().expect("media clock").current_time()
    }

    pub fn duration(&self) -> f64 {
        self.clock.lock().expect("media clock").duration
    }

    pub fn video_size(&self) -> (u32, u32) {
        let clock = self.clock.lock().expect("media clock");
        (clock.width, clock.height)
    }

    pub fn ended(&self) -> bool {
        self.clock.lock().expect("media clock").ended
    }

    pub fn playing(&self) -> bool {
        self.clock.lock().expect("media clock").playing
    }

    pub fn take_error(&self) -> Option<String> {
        self.clock.lock().expect("media clock").error.take()
    }

    pub fn frame_count(&self) -> u64 {
        self.frames.load(Ordering::Acquire)
    }

    pub fn seek(&mut self, seconds: f64) {
        let seconds = seconds.max(0.0);
        if self.stream_mode {
            self.reset_stream_queue(seconds);
            let mut clock = self.clock.lock().expect("media clock");
            let was_playing = clock.playing;
            clock.start_time = seconds;
            clock.started_at = was_playing.then(Instant::now);
            clock.ended = false;
            return;
        }
        if self.playing() {
            let _ = self.play(seconds);
        } else {
            self.seek_paused(seconds);
        }
    }

    fn reset_stream_queue(&self, target: f64) {
        let mut stream = self.stream_queue.state.lock().expect("video stream queue");
        stream.packets.clear();
        stream.queued_bytes = 0;
        stream.epoch = stream.epoch.wrapping_add(1);
        stream.target_time = target.max(0.0);
        stream.ended = false;
        self.stream_queue.ready.notify_all();
        trace(format_args!("stream queue reset epoch={} target={:.3}s", stream.epoch, stream.target_time));
    }

    fn reset_audio_queue(&self, target: f64) {
        let mut audio = self.stream_audio_queue.state.lock().expect("audio stream queue");
        audio.packets.clear();
        audio.epoch = audio.epoch.wrapping_add(1);
        audio.target_time = target.max(0.0);
        audio.ended = false;
        self.stream_audio_queue.ready.notify_all();
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop_playback(false);
    }
}

fn decode_loop(
    doc: Arc<CoreDocument>,
    node: NodeId,
    path: PathBuf,
    audio_path: Option<PathBuf>,
    _temp_guard: Option<Arc<tempfile::TempDir>>,
    start: f64,
    stop: Arc<AtomicBool>,
    current_generation: Arc<AtomicU64>,
    generation: u64,
    clock: Arc<Mutex<Clock>>,
    frames: Arc<AtomicU64>,
) {
    trace(format_args!(
        "worker entered node={} generation={} path={} start={start:.3}s",
        node.0,
        generation,
        path.display()
    ));
    let audio = spawn_audio(
        audio_path.unwrap_or_else(|| path.clone()),
        start,
        Arc::clone(&stop),
        Arc::clone(&clock),
    );
    trace(format_args!("video decode begin node={} generation={generation}", node.0));
    let video = decode_video(
        &doc,
        node,
        &path,
        start,
        &stop,
        &clock,
        &frames,
        &current_generation,
        generation,
    );
    trace(format_args!(
        "video decode returned node={} generation={generation} result={}",
        node.0,
        match &video { Ok(ended) => format!("ok ended={ended}"), Err(error) => format!("error: {error}") }
    ));
    if let Some(audio) = audio {
        trace(format_args!("waiting for audio worker node={} generation={generation}", node.0));
        stop.store(true, Ordering::Release);
        let _ = audio.join();
        trace(format_args!("audio worker joined node={} generation={generation}", node.0));
    }
    if current_generation.load(Ordering::Acquire) != generation {
        return;
    }
    let mut clock = clock.lock().expect("media clock");
    let elapsed = clock.current_time();
    clock.playing = false;
    clock.started_at = None;
    match video {
        Ok(true) => {
            if !clock.duration.is_finite() || clock.duration <= 0.0 {
                clock.duration = elapsed;
            }
            clock.start_time = clock.duration;
            clock.ended = true;
        }
        Ok(false) => {
            clock.start_time = elapsed;
        }
        Err(err) => {
            clock.error = Some(err);
            clock.start_time = elapsed;
        }
    }
    trace(format_args!(
        "worker finished node={} generation={} frames={}",
        node.0,
        generation,
        frames.load(Ordering::Acquire)
    ));
}

fn decode_stream_loop(
    doc: Arc<CoreDocument>,
    node: NodeId,
    queue: Arc<StreamQueue>,
    audio_queue: Arc<StreamAudioQueue>,
    start: f64,
    stop: Arc<AtomicBool>,
    current_generation: Arc<AtomicU64>,
    generation: u64,
    clock: Arc<Mutex<Clock>>,
    frames: Arc<AtomicU64>,
) {
    trace(format_args!("stream worker entered node={} generation={} start={start:.3}s", node.0, generation));
    let audio_queue_for_worker = Arc::clone(&audio_queue);
    let audio_stop = Arc::clone(&stop);
    let audio_clock = Arc::clone(&clock);
    let audio_worker = thread::Builder::new()
        .name("sweet-audio-stream".into())
        .spawn(move || decode_stream_audio(audio_queue_for_worker, audio_stop, audio_clock));
    let mut decoder: Option<Kind> = None;
    let mut active_epoch = u64::MAX;
    let mut target_time = start;
    let mut playback_origin = Instant::now();
    let mut decoded_frames = 0u64;
    let mut decoded_packets = 0u64;
    let mut last_decode_error = None;

    while !stop.load(Ordering::Acquire) {
        let Some(packet) = queue.pop(&stop) else {
            if stop.load(Ordering::Acquire) {
                break;
            }
            let ended = queue.state.lock().expect("video stream queue").ended;
            if ended {
                trace(format_args!("stream queue drained node={} generation={generation} packets={decoded_packets} frames={decoded_frames}", node.0));
                if current_generation.load(Ordering::Acquire) == generation {
                    let mut clock = clock.lock().expect("media clock");
                    clock.playing = false;
                    clock.started_at = None;
                    if decoded_frames == 0 {
                        let detail = last_decode_error
                            .map(|error| format!("; last decoder error: {error}"))
                            .unwrap_or_default();
                        clock.error = Some(format!(
                            "stream ended before decoding a video frame ({decoded_packets} packets received{detail})"
                        ));
                        clock.ended = false;
                    } else {
                        if clock.duration.is_finite() && clock.duration > 0.0 {
                            clock.start_time = clock.duration;
                        }
                        clock.ended = true;
                    }
                }
            }
            break;
        };

        if packet.epoch != active_epoch {
            let (codec, codec_private, target) = {
                let state = queue.state.lock().expect("video stream queue");
                let Some(codec) = state.codec else {
                    trace("stream packet arrived before decoder configuration");
                    continue;
                };
                (codec, state.codec_private.clone(), state.target_time)
            };
            decoder = Some(match codec {
                StreamCodec::H264 => Kind::H264(H264Decoder::new()),
                StreamCodec::Hevc => Kind::Hevc(HevcDecoder::new()),
                StreamCodec::Av1 => Kind::Av1(Av1Decoder::new()),
            });
            if !codec_private.is_empty() {
                if let Some(decoder) = decoder.as_mut() {
                    if let Err(err) = decode_annex(decoder, &codec_private) {
                        trace(format_args!("stream codec initialization rejected: {err}"));
                    }
                }
            }
            active_epoch = packet.epoch;
            target_time = target;
            playback_origin = Instant::now();
            trace(format_args!("stream decoder reset epoch={active_epoch} target={target_time:.3}s codec={codec:?}"));
        }

        let current_epoch = queue.state.lock().expect("video stream queue").epoch;
        if packet.epoch != current_epoch {
            continue;
        }
        let decoded = match decode_annex(decoder.as_mut().expect("configured decoder"), &packet.data) {
            Ok(frame) => frame,
            Err(err) => {
                decoded_packets += 1;
                last_decode_error = Some(err.clone());
                trace(format_args!("stream packet rejected timestamp={:.3}s keyframe={} error={err}", packet.timestamp, packet.keyframe));
                continue;
            }
        };
        decoded_packets += 1;
        let Some(frame) = decoded else {
            continue;
        };
        if packet.timestamp + 0.001 < target_time {
            continue;
        }

        let due = playback_origin + Duration::from_secs_f64((packet.timestamp - target_time).max(0.0));
        while Instant::now() < due {
            if stop.load(Ordering::Acquire)
                || current_generation.load(Ordering::Acquire) != generation
                || queue.state.lock().expect("video stream queue").epoch != active_epoch
            {
                break;
            }
            thread::sleep(Duration::from_millis(4));
        }
        if stop.load(Ordering::Acquire)
            || current_generation.load(Ordering::Acquire) != generation
            || queue.state.lock().expect("video stream queue").epoch != active_epoch
        {
            continue;
        }
        {
            let mut clock = clock.lock().expect("media clock");
            clock.start_time = packet.timestamp;
            clock.started_at = Some(Instant::now());
        }
        if let Err(err) = push_frame(&doc, node, &clock, &frames, &frame) {
            trace(format_args!("stream frame delivery failed: {err}"));
            let mut state = clock.lock().expect("media clock");
            state.error = Some(err);
            state.playing = false;
            state.started_at = None;
            break;
        }
        decoded_frames += 1;
    }
    if let Ok(audio_worker) = audio_worker {
        let _ = audio_worker.join();
    }
    trace(format_args!("stream worker finished node={} generation={} frames={}", node.0, generation, frames.load(Ordering::Acquire)));
}

fn decode_stream_audio(
    queue: Arc<StreamAudioQueue>,
    stop: Arc<AtomicBool>,
    clock: Arc<Mutex<Clock>>,
) {
    let mut active_epoch = u64::MAX;
    let mut decoder = None;
    let mut output: Option<(mpsc::SyncSender<Vec<f32>>, JoinHandle<()>)> = None;
    trace("stream audio worker entered");
    while !stop.load(Ordering::Acquire) {
        let Some(packet) = queue.pop(&stop) else {
            break;
        };
        let (epoch, config, target) = {
            let state = queue.state.lock().expect("audio stream queue");
            let Some(config) = state.config.as_ref() else {
                continue;
            };
            (state.epoch, StreamAudioConfig {
                codec: config.codec.clone(),
                extra_data: config.extra_data.clone(),
                sample_rate: config.sample_rate,
                channels: config.channels,
                time_scale: config.time_scale,
            }, state.target_time)
        };
        if packet.epoch != epoch {
            continue;
        }
        if active_epoch != packet.epoch {
            if let Some((sender, worker)) = output.take() {
                drop(sender);
                let _ = worker.join();
            }
            let layout = match config.channels {
                1 => Channels::FRONT_LEFT,
                _ => Channels::FRONT_LEFT | Channels::FRONT_RIGHT,
            };
            let mut params = CodecParameters::new();
            params
                .for_codec(CODEC_TYPE_AAC)
                .with_sample_rate(config.sample_rate.max(1))
                .with_channels(layout);
            params.extra_data = Some(config.extra_data.clone().into_boxed_slice());
            decoder = match symphonia::default::get_codecs().make(&params, &DecoderOptions::default()) {
                Ok(decoder) => Some(decoder),
                Err(err) => {
                    trace(format_args!("stream AAC decoder initialization failed: {err}"));
                    break;
                }
            };
            let (sender, receiver) = mpsc::sync_channel::<Vec<f32>>(12);
            let rate = config.sample_rate.max(1);
            let channels = config.channels.max(1);
            let output_clock = Arc::clone(&clock);
            let worker = thread::Builder::new()
                .name("sweet-rodio-stream".into())
                .spawn(move || play_rodio(receiver, rate, channels, output_clock));
            match worker {
                Ok(worker) => output = Some((sender, worker)),
                Err(err) => {
                    trace(format_args!("stream audio output thread failed: {err}"));
                    break;
                }
            }
            active_epoch = packet.epoch;
            trace(format_args!("stream audio decoder reset epoch={active_epoch} target={target:.3}s rate={} channels={}", config.sample_rate, config.channels));
        }
        if packet.timestamp + packet.duration <= target {
            continue;
        }
        let ticks = (packet.timestamp.max(0.0) * config.time_scale as f64) as u64;
        let duration = (packet.duration.max(0.0) * config.time_scale as f64) as u64;
        let encoded = Packet::new_from_slice(1, ticks, duration, &packet.data);
        let decoded = match decoder.as_mut().expect("configured AAC decoder").decode(&encoded) {
            Ok(decoded) => decoded,
            Err(err) => {
                trace(format_args!("stream AAC packet failed at {:.3}s: {err}", packet.timestamp));
                continue;
            }
        };
        let spec = *decoded.spec();
        let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        samples.copy_interleaved_ref(decoded);
        let active = queue.state.lock().expect("audio stream queue").epoch == active_epoch;
        if !active {
            continue;
        }
        let output_closed = output
            .as_ref()
            .is_some_and(|(sender, _)| sender.send(samples.samples().to_vec()).is_err());
        if output_closed {
            trace("stream audio output closed");
            if let Some((sender, worker)) = output.take() {
                drop(sender);
                let _ = worker.join();
            }
        }
    }
    if let Some((sender, worker)) = output.take() {
        drop(sender);
        let _ = worker.join();
    }
    trace("stream audio worker finished");
}

fn decode_video(
    doc: &CoreDocument,
    node: NodeId,
    path: &Path,
    start: f64,
    stop: &AtomicBool,
    clock: &Mutex<Clock>,
    frames: &AtomicU64,
    current_generation: &AtomicU64,
    generation: u64,
) -> Result<bool, String> {
    let ext = extension(path);
    trace(format_args!("selecting decoder path={} extension={ext}", path.display()));
    if ext == "mkv" || ext == "webm" {
        return decode_mkv(doc, node, path, start, stop, clock, frames, current_generation, generation);
    }
    decode_mp4(doc, node, path, start, stop, clock, frames, current_generation, generation)
}

fn decode_mp4(
    doc: &CoreDocument,
    node: NodeId,
    path: &Path,
    start: f64,
    stop: &AtomicBool,
    clock: &Mutex<Clock>,
    frames: &AtomicU64,
    current_generation: &AtomicU64,
    generation: u64,
) -> Result<bool, String> {
    trace(format_args!("opening MP4 reader {}", path.display()));
    let mut reader = Mp4VideoReader::open(path).map_err(|err| err.to_string())?;
    trace(format_args!("MP4 reader ready {}", path.display()));
    note_duration(clock, reader.audio_info());
    let mut index = 0u64;
    let mut last_push = Instant::now() - Duration::from_secs(1);
    let interval = Duration::from_secs_f64(1.0 / TARGET_FPS);
    while !stop.load(Ordering::Acquire) {
        let frame_number = index + 1;
        if frame_number == 1 || frame_number % 60 == 0 {
            trace(format_args!("MP4 requesting frame {frame_number} from {}", path.display()));
        }
        let frame = reader.next_frame().map_err(|err| err.to_string())?;
        let Some(frame) = frame else {
            trace(format_args!("MP4 reached EOF after {index} frames"));
            return Ok(true);
        };
        let timestamp = frame.timestamp_us as f64 / 1_000_000.0;
        // No time seek on the reader: skip pictures before the requested start.
        if timestamp + 0.001 < start {
            index += 1;
            continue;
        }
        let now = Instant::now();
        if index == 0 || now.duration_since(last_push) >= interval {
            if current_generation.load(Ordering::Acquire) != generation { return Ok(false); }
            push_frame(doc, node, clock, frames, &frame)?;
            if frames.load(Ordering::Acquire) == 1 || frames.load(Ordering::Acquire) % 30 == 0 {
                trace(format_args!("MP4 frame delivered count={} timestamp={timestamp:.3}s", frames.load(Ordering::Acquire)));
            }
            last_push = Instant::now();
            if wait_or_stop(interval, stop) {
                return Ok(false);
            }
        }
        index += 1;
    }
    Ok(false)
}

fn decode_mkv(
    doc: &CoreDocument,
    node: NodeId,
    path: &Path,
    start: f64,
    stop: &AtomicBool,
    clock: &Mutex<Clock>,
    frames: &AtomicU64,
    current_generation: &AtomicU64,
    generation: u64,
) -> Result<bool, String> {
    trace(format_args!("opening Matroska reader {}", path.display()));
    let mut demuxer = MkvDemuxer::open(path).map_err(|err| err.to_string())?;
    trace(format_args!("Matroska reader ready {} codec={:?} indexed_frames={}", path.display(), demuxer.codec(), demuxer.frame_count()));
    note_duration(clock, demuxer.audio_info());
    let mut decoder = match demuxer.codec() {
        MkvCodec::H264 => Kind::H264(H264Decoder::new()),
        MkvCodec::Hevc => Kind::Hevc(HevcDecoder::new()),
        MkvCodec::Av1 => Kind::Av1(Av1Decoder::new()),
        other => return Err(format!("unsupported mkv codec: {other:?}")),
    };
    let total_packets = demuxer.frame_count();
    let mut packet_index = 0usize;
    trace(format_args!("Matroska decoder ready codec={:?} packets={total_packets}", demuxer.codec()));
    let _ = decode_annex(&mut decoder, demuxer.codec_private());
    let mut last_push = Instant::now() - Duration::from_secs(1);
    let interval = Duration::from_secs_f64(1.0 / TARGET_FPS);
    while !stop.load(Ordering::Acquire) {
        let Some(packet) = demuxer.next_frame() else {
            trace(format_args!("Matroska reached EOF {}", path.display()));
            return Ok(true);
        };
        packet_index += 1;
        let log_packet = packet_index <= 3 || packet_index % 120 == 0;
        if log_packet {
            trace(format_args!("Matroska packet {packet_index}/{total_packets} timestamp={:.3}s bytes={}", packet.timestamp_ms as f64 / 1000.0, packet.data.len()));
        }
        let timestamp = packet.timestamp_ms as f64 / 1_000.0;
        if timestamp + 0.001 < start {
            let _ = decode_annex(&mut decoder, &packet.data);
            continue;
        }
        // yscv-video currently indexes every Matroska SimpleBlock, including
        // audio blocks. Skip packets that are not valid for this video codec.
        if log_packet {
            trace(format_args!("video codec decode begin packet={packet_index}"));
        }
        let decoded = match decode_annex(&mut decoder, &packet.data) {
            Ok(decoded) => decoded,
            Err(err) => {
                if log_packet {
                    trace(format_args!("video codec rejected packet={packet_index}: {err}"));
                }
                continue;
            }
        };
        if log_packet {
            trace(format_args!("video codec decode returned packet={packet_index} frame={}", decoded.is_some()));
        }
        if let Some(frame) = decoded {
            let now = Instant::now();
            if now.duration_since(last_push) >= interval {
                if current_generation.load(Ordering::Acquire) != generation { return Ok(false); }
                push_frame(doc, node, clock, frames, &frame)?;
                if frames.load(Ordering::Acquire) == 1 || frames.load(Ordering::Acquire) % 30 == 0 {
                    trace(format_args!("Matroska frame delivered count={} timestamp={timestamp:.3}s", frames.load(Ordering::Acquire)));
                }
                last_push = Instant::now();
                if wait_or_stop(interval, stop) {
                    return Ok(false);
                }
            }
        }
    }
    Ok(false)
}

enum Kind {
    H264(H264Decoder),
    Hevc(HevcDecoder),
    Av1(Av1Decoder),
}

fn decode_annex(decoder: &mut Kind, data: &[u8]) -> Result<Option<DecodedFrame>, String> {
    if data.is_empty() {
        return Ok(None);
    }
    let mut produced = None;
    if let Kind::Av1(decoder) = decoder {
        return decoder
            .decode_obu(data, 0)
            .map_err(|err| err.to_string());
    }
    for nal in split_nals(data) {
        let frame = match decoder {
            Kind::H264(decoder) => {
                let header = *nal.first().unwrap_or(&0);
                let unit = NalUnit {
                    nal_type: NalUnitType::from_byte(header),
                    nal_ref_idc: (header >> 5) & 3,
                    data: nal,
                };
                decoder.process_nal(&unit).map_err(|err| err.to_string())?
            }
            Kind::Hevc(decoder) => decoder.decode_nal(&nal).map_err(|err| err.to_string())?,
            Kind::Av1(_) => unreachable!(),
        };
        if frame.is_some() {
            produced = frame;
        }
    }
    Ok(produced)
}

fn split_nals(data: &[u8]) -> Vec<Vec<u8>> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
            continue;
        }
        if i + 4 < data.len() && data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 0 && data[i + 3] == 1
        {
            starts.push(i + 4);
            i += 4;
            continue;
        }
        i += 1;
    }
    if starts.is_empty() {
        return vec![data.to_vec()];
    }
    let mut nals = Vec::new();
    for (index, start) in starts.iter().copied().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(data.len());
        let mut end = end;
        if end >= 3 && data[end - 3] == 0 && data[end - 2] == 0 && data[end - 1] == 1 {
            end -= 3;
        }
        if end >= 4 && data[end - 4] == 0 {
            end -= 1;
        }
        if start < end {
            nals.push(data[start..end].to_vec());
        }
    }
    nals
}

fn wait_or_stop(total: Duration, stop: &AtomicBool) -> bool {
    let step = Duration::from_millis(20);
    let mut left = total;
    while left > Duration::ZERO {
        if stop.load(Ordering::Acquire) {
            return true;
        }
        let slice = if left > step { step } else { left };
        thread::sleep(slice);
        left = left.saturating_sub(slice);
    }
    stop.load(Ordering::Acquire)
}

fn push_frame(
    doc: &CoreDocument,
    node: NodeId,
    clock: &Mutex<Clock>,
    frames: &AtomicU64,
    frame: &DecodedFrame,
) -> Result<(), String> {
    let frame_count = frames.load(Ordering::Acquire) + 1;
    if frame_count == 1 || frame_count % 30 == 0 {
        trace(format_args!("converting frame #{frame_count} {}x{}", frame.width, frame.height));
    }
    let rgba = fit_rgb_to_rgba(&frame.rgb8_data, frame.width, frame.height);
    if frame_count == 1 || frame_count % 30 == 0 {
        trace(format_args!("storing frame #{frame_count} in DOM"));
    }
    doc.set_image_rgba(node, FRAME_WIDTH, FRAME_HEIGHT, rgba)
        .map_err(|err| err.0)?;
    let mut clock = clock.lock().expect("media clock");
    clock.width = frame.width as u32;
    clock.height = frame.height as u32;
    drop(clock);
    frames.fetch_add(1, Ordering::Release);
    if frame_count == 1 || frame_count % 30 == 0 {
        trace(format_args!("DOM frame #{frame_count} stored"));
    }
    Ok(())
}

fn note_duration(clock: &Mutex<Clock>, info: Option<&AudioTrackInfo>) {
    let Some(info) = info else {
        return;
    };
    if info.duration_ms == 0 {
        return;
    }
    let mut clock = clock.lock().expect("media clock");
    if !clock.duration.is_finite() || clock.duration <= 0.0 {
        clock.duration = info.duration_ms as f64 / 1000.0;
    }
}

/// Letterbox an RGB8 picture into the 480x270 preview the DOM used to request.
fn fit_rgb_to_rgba(rgb: &[u8], width: usize, height: usize) -> Vec<u8> {
    let out_w = FRAME_WIDTH as usize;
    let out_h = FRAME_HEIGHT as usize;
    let mut out = vec![0u8; out_w * out_h * 4];
    if width == 0 || height == 0 || rgb.len() < width.saturating_mul(height).saturating_mul(3) {
        return out;
    }
    let scale = (out_w as f32 / width as f32).min(out_h as f32 / height as f32);
    let dest_w = ((width as f32) * scale).round().max(1.0) as usize;
    let dest_h = ((height as f32) * scale).round().max(1.0) as usize;
    let x0 = (out_w - dest_w) / 2;
    let y0 = (out_h - dest_h) / 2;
    for y in 0..dest_h {
        let src_y = ((y as f32) * (height as f32) / (dest_h as f32))
            .floor()
            .clamp(0.0, (height - 1) as f32) as usize;
        for x in 0..dest_w {
            let src_x = ((x as f32) * (width as f32) / (dest_w as f32))
                .floor()
                .clamp(0.0, (width - 1) as f32) as usize;
            let src = (src_y * width + src_x) * 3;
            let dst = ((y0 + y) * out_w + (x0 + x)) * 4;
            out[dst] = rgb[src];
            out[dst + 1] = rgb[src + 1];
            out[dst + 2] = rgb[src + 2];
            out[dst + 3] = 255;
        }
    }
    out
}

fn spawn_audio(
    path: PathBuf,
    start: f64,
    stop: Arc<AtomicBool>,
    clock: Arc<Mutex<Clock>>,
) -> Option<JoinHandle<()>> {
    trace(format_args!("spawning audio worker path={} start={start:.3}s", path.display()));
    thread::Builder::new()
        .name("sweet-audio".into())
        .spawn(move || {
            if let Err(err) = play_audio(&path, start, &stop, Arc::clone(&clock)) {
                trace(format_args!("audio worker failed path={}: {err}", path.display()));
                if !clock.lock().expect("media clock").muted {
                    eprintln!("sweet audio: {err}");
                }
            }
        })
        .ok()
}

fn play_audio(
    path: &Path,
    start: f64,
    stop: &AtomicBool,
    clock: Arc<Mutex<Clock>>,
) -> Result<(), String> {
    trace(format_args!("opening audio source {}", path.display()));
    let file = std::fs::File::open(path).map_err(|err| err.to_string())?;
    trace("audio file opened; probing container");
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|err| err.to_string())?;
    let mut format = probed.format;
    trace(format_args!("audio container opened {} tracks={}", path.display(), format.tracks().len()));
    let track = format
        .tracks()
        .iter()
        .find(|track| {
            track.codec_params.codec != CODEC_TYPE_NULL
                && (track.codec_params.channels.is_some()
                    || track.codec_params.sample_rate.is_some())
        })
        .cloned()
        .ok_or_else(|| "container has no audio track".to_string())?;
    trace(format_args!("audio track selected id={} codec={:?} rate={:?} channels={:?}", track.id, track.codec_params.codec, track.codec_params.sample_rate, track.codec_params.channels));
    trace("creating audio codec decoder");
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|err| err.to_string())?;
    trace("audio codec decoder created");
    let rate = track.codec_params.sample_rate.unwrap_or(48_000);
    let channels = track
        .codec_params
        .channels
        .map(|channels| channels.count() as u16)
        .filter(|count| *count > 0)
        .unwrap_or(2);
    let track_id = track.id;
    let time_base = track.codec_params.time_base;
    let (sender, receiver) = mpsc::sync_channel::<Vec<f32>>(12);
    let playback = thread::Builder::new()
        .name("sweet-rodio".into())
        .spawn(move || play_rodio(receiver, rate, channels, Arc::clone(&clock)))
        .map_err(|err| err.to_string())?;
    trace(format_args!("audio decoder ready track_id={track_id} rate={rate} channels={channels}"));
    let mut skipped = start <= 0.0;
    let mut packet_count = 0u64;
    let mut decoded_count = 0u64;
    while !stop.load(Ordering::Acquire) {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(err) => {
                trace(format_args!("audio stream ended after {packet_count} packets: {err}"));
                break;
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        packet_count += 1;
        if !skipped {
            let seconds = time_base
                .map(|base| {
                    let ticks = packet.ts() as f64;
                    ticks * base.numer as f64 / base.denom as f64
                })
                .unwrap_or(0.0);
            if seconds + 0.05 < start {
                continue;
            }
            skipped = true;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(err) => {
                if packet_count <= 3 {
                    trace(format_args!("audio packet {packet_count} decode failed: {err}"));
                }
                continue;
            }
        };
        let spec = *decoded.spec();
        let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        samples.copy_interleaved_ref(decoded);
        let mixed: Vec<f32> = samples.samples().iter().copied().collect();
        decoded_count += 1;
        if decoded_count == 1 || decoded_count % 200 == 0 {
            trace(format_args!("audio decoded packets={decoded_count} samples={}", mixed.len()));
        }
        if decoded_count == 1 || decoded_count % 100 == 0 {
            trace(format_args!("queueing audio packet {decoded_count}"));
        }
        if sender.send(mixed).is_err() {
            trace("audio sample queue closed");
            break;
        }
        if decoded_count == 1 || decoded_count % 100 == 0 {
            trace(format_args!("queued audio packet {decoded_count}"));
        }
    }
    trace(format_args!("audio decode ended packets={packet_count} decoded={decoded_count}; joining output"));
    drop(sender);
    let _ = playback.join();
    trace("audio output thread joined");
    Ok(())
}

fn play_rodio(
    receiver: Receiver<Vec<f32>>,
    rate: u32,
    channels: u16,
    clock: Arc<Mutex<Clock>>,
) {
    trace(format_args!("opening audio device rate={rate} channels={channels}"));
    let (_stream, handle) = match rodio::OutputStream::try_default() {
        Ok(stream) => {
            trace("audio device opened");
            stream
        }
        Err(err) => {
            trace(format_args!("audio device open failed: {err}"));
            eprintln!("sweet audio output: {err}");
            return;
        }
    };
    let (sample_tx, sample_rx) = mpsc::sync_channel::<f32>(rate as usize);
    let gain_clock = Arc::clone(&clock);
    let forward = thread::spawn(move || {
        while let Ok(chunk) = receiver.recv() {
            let gain = {
                let clock = gain_clock.lock().expect("media clock");
                if clock.muted { 0.0 } else { clock.volume }
            };
            for sample in chunk {
                if sample_tx.send(sample * gain).is_err() {
                    return;
                }
            }
        }
    });
    let source = PcmSource {
        rx: sample_rx,
        rate,
        channels,
        done: false,
        underruns: 0,
    };
    if handle.play_raw(source).is_err() {
        trace("rodio rejected audio source");
        eprintln!("sweet audio: could not start playback");
    } else {
        trace("rodio accepted audio source");
    }
    let _ = forward.join();
    trace("audio forwarding thread exited");
}

struct PcmSource {
    rx: Receiver<f32>,
    rate: u32,
    channels: u16,
    done: bool,
    underruns: u64,
}

impl Iterator for PcmSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.rx.try_recv() {
            Ok(sample) => Some(sample),
            Err(TryRecvError::Empty) => {
                self.underruns += 1;
                if self.underruns == 1 || self.underruns % 48_000 == 0 {
                    trace(format_args!("stream audio output underrun samples={}", self.underruns));
                }
                // Rodio calls `next` on its output callback thread. Never block
                // that thread waiting for network/decode work; output silence
                // for a short underrun and consume audio again when it arrives.
                Some(0.0)
            }
            Err(TryRecvError::Disconnected) => {
                self.done = true;
                None
            }
        }
    }
}

impl rodio::Source for PcmSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        self.channels.max(1)
    }

    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

pub fn resolve_source(source: &str) -> Result<ResolvedSource, String> {
    trace(format_args!("resolve start source={}", source_label(source)));
    if is_youtube(source) {
        let result = resolve_youtube(source).map(|(info, temp)| ResolvedSource { info, temp });
        trace(format_args!("resolve YouTube {}", if result.is_ok() { "complete" } else { "failed" }));
        return result;
    }
    if let Some(rest) = source.strip_prefix("file://") {
        let info = probe_file(&PathBuf::from(rest))?;
        trace(format_args!("resolve local complete duration={:.3}s path={}", info.duration, info.path.display()));
        return Ok(ResolvedSource { info, temp: None });
    }
    if source.starts_with("http://") || source.starts_with("https://") {
        let result = download_to_temp(source).map(|(info, temp)| ResolvedSource { info, temp });
        trace(format_args!("resolve URL {}", if result.is_ok() { "complete" } else { "failed" }));
        return result;
    }
    let info = probe_file(&PathBuf::from(source))?;
    trace(format_args!("resolve local complete duration={:.3}s path={}", info.duration, info.path.display()));
    Ok(ResolvedSource { info, temp: None })
}

fn source_label(source: &str) -> String {
    if let Ok(url) = reqwest::Url::parse(source) {
        return format!("{}{}", url.host_str().unwrap_or("url"), url.path());
    }
    Path::new(source)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("local source")
        .to_string()
}

fn probe_file(path: &Path) -> Result<MediaInfo, String> {
    let bytes = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    trace(format_args!("probe start path={} bytes={bytes}", path.display()));
    if !path.is_file() {
        return Err(format!("video file not found: {}", path.display()));
    }
    let ext = extension(path);
    let duration = if ext == "mkv" || ext == "webm" {
        MkvDemuxer::open(path)
            .ok()
            .and_then(|demuxer| demuxer.audio_info().map(|info| info.duration_ms as f64 / 1000.0))
            .unwrap_or(f64::NAN)
    } else {
        Mp4VideoReader::open(path)
            .ok()
            .and_then(|reader| {
                reader
                    .audio_info()
                    .map(|info| info.duration_ms as f64 / 1000.0)
            })
            .filter(|duration| *duration > 0.0)
            .unwrap_or(f64::NAN)
    };
    trace(format_args!("probe complete path={} duration={duration:.3}s", path.display()));
    Ok(MediaInfo {
        duration,
        width: 0,
        height: 0,
        path: path.to_path_buf(),
        audio_path: None,
    })
}

fn download_to_temp(url: &str) -> Result<(MediaInfo, Option<tempfile::TempDir>), String> {
    download_named(url, "media.mp4")
}

fn download_named(
    url: &str,
    name: &str,
) -> Result<(MediaInfo, Option<tempfile::TempDir>), String> {
    let dir = tempfile::TempDir::new().map_err(|err| err.to_string())?;
    let path = dir.path().join(name);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    trace(format_args!("download begin source={} target={}", source_label(url), path.display()));
    runtime
        .block_on(download_file(url, &path))
        .map_err(|err| err.to_string())?;
    let mut info = probe_file(&path)?;
    info.path = path;
    Ok((info, Some(dir)))
}

async fn download_file(url: &str, path: &Path) -> Result<(), String> {
    let started = Instant::now();
    trace(format_args!("HTTP GET begin source={}", source_label(url)));
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(8))
        .build()
        .map_err(|err| err.to_string())?;
    let mut response = client.get(url).send().await.map_err(|err| err.to_string())?;
    trace(format_args!("HTTP response status={} content_length={:?}", response.status(), response.content_length()));
    if !response.status().is_success() {
        return Err(format!("media download failed: {}", response.status()));
    }
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|err| err.to_string())?;
    let content_length = response.content_length();
    let mut downloaded = 0u64;
    let mut next_log = 4 * 1024 * 1024u64;
    while let Some(chunk) = response.chunk().await.map_err(|err| err.to_string())? {
        downloaded += chunk.len() as u64;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|err| err.to_string())?;
        if downloaded >= next_log {
            let elapsed = started.elapsed().as_secs_f64().max(0.001);
            let rate_mib = downloaded as f64 / elapsed / (1024.0 * 1024.0);
            trace(format_args!(
                "download progress bytes={downloaded} content_length={content_length:?} elapsed={elapsed:.1}s average={rate_mib:.2}MiB/s"
            ));
            next_log = downloaded + 4 * 1024 * 1024;
        }
    }
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    let rate_mib = downloaded as f64 / elapsed / (1024.0 * 1024.0);
    trace(format_args!("download complete bytes={downloaded} elapsed={elapsed:.1}s average={rate_mib:.2}MiB/s target={}", path.display()));
    Ok(())
}

fn is_youtube(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or("").trim_start_matches("www.");
    host == "youtube.com" || host.ends_with(".youtube.com") || host == "youtu.be"
}

struct YtStream {
    url: String,
    vcodec: String,
    acodec: String,
    ext: String,
    width: u32,
    height: u32,
    abr: f64,
    tbr: f64,
    protocol: String,
}

/// YouTube usually has no single progressive MP4. Pick an H.264 video plus an
/// m4a audio stream (the codecs yscv-video and symphonia can open) and let
/// yt-dlp mux them. Fall back to a single progressive URL when one exists.
fn resolve_youtube(page: &str) -> Result<(MediaInfo, Option<tempfile::TempDir>), String> {
    trace(format_args!("YouTube metadata lookup begin source={}", source_label(page)));
    let binary = ytdlp_binary()?;
    trace(format_args!("running yt-dlp binary={}", binary.display()));
    let output = Command::new(&binary)
        .args([
            "--no-playlist",
            "--no-warnings",
            "--skip-download",
            "-J",
            page,
        ])
        .output()
        .map_err(|err| format!("yt-dlp failed to start: {err}"))?;
    trace(format_args!("yt-dlp exited status={} stdout_bytes={} stderr_bytes={}", output.status, output.stdout.len(), output.stderr.len()));
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("yt-dlp exited {}: {}", output.status, detail.trim()));
    }
    let info: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|err| format!("yt-dlp did not return video info: {err}"))?;
    let duration = info.get("duration").and_then(|v| v.as_f64()).unwrap_or(f64::NAN);
    let width = info.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let height = info.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let streams = youtube_streams(&info);
    trace(format_args!("YouTube metadata parsed duration={duration:.3}s formats={}", streams.len()));
    if streams.is_empty() {
        return Err("YouTube did not return a playable stream".into());
    }
    if let Some(muxed) = pick_progressive(&streams) {
        trace(format_args!("selected progressive stream ext={} {}x{}", muxed.ext, muxed.width, muxed.height));
        let (mut media, temp) = download_named(&muxed.url, &media_name(&muxed.ext))?;
        fill_media(&mut media, duration, muxed.width.max(width), muxed.height.max(height));
        return Ok((media, temp));
    }
    let video = pick_video(&streams).ok_or_else(|| {
        "YouTube did not return an H.264, HEVC, or AV1 video stream".to_string()
    })?;
    if video.url.is_empty() {
        return Err("YouTube video stream has no URL".into());
    }
    let audio = pick_audio(&streams).filter(|item| !item.url.is_empty());
    trace(format_args!("selected video stream ext={} {}x{}; separate_audio={}", video.ext, video.width, video.height, audio.is_some()));
    // Download the elementary streams ourselves. Merging them would call ffmpeg,
    // and yscv-video only needs the video container. Audio plays from its own file.
    let _ = binary;
    let _ = page;
    let (mut media, temp) = download_named(&video.url, &video_name(&video.ext))?;
    if let Some(audio) = audio {
        let (audio_info, audio_temp) = download_named(&audio.url, &audio_name(&audio.ext))?;
        let kept = temp
            .as_ref()
            .map(|dir| dir.path().join(audio_name(&audio.ext)))
            .unwrap_or_else(|| audio_info.path.clone());
        if audio_info.path != kept {
            std::fs::rename(&audio_info.path, &kept).map_err(|err| err.to_string())?;
        }
        media.audio_path = Some(kept);
        drop(audio_temp);
    }
    fill_media(&mut media, duration, video.width.max(width), video.height.max(height));
    Ok((media, temp))
}

fn media_name(ext: &str) -> String {
    match ext {
        "m4a" => "audio.m4a".to_string(),
        "webm" => "media.webm".to_string(),
        "mkv" => "media.mkv".to_string(),
        _ => "media.mp4".to_string(),
    }
}

fn video_name(ext: &str) -> String {
    media_name(ext).replace("media.", "video.")
}

fn audio_name(ext: &str) -> String {
    match ext {
        "m4a" => "audio.m4a".to_string(),
        "webm" => "audio.webm".to_string(),
        _ => "audio.mp4".to_string(),
    }
}

struct YtPick {
    url: String,
    width: u32,
    height: u32,
    ext: String,
}

fn youtube_streams(info: &serde_json::Value) -> Vec<(String, YtStream)> {
    info.get("formats")
        .and_then(|formats| formats.as_array())
        .into_iter()
        .flatten()
        .filter_map(|format| {
            let id = format.get("format_id").and_then(|v| v.as_str()).unwrap_or("stream").to_string();
            let url = format.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
            Some((
                id,
                YtStream {
                    url,
                    vcodec: format.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none").to_string(),
                    acodec: format.get("acodec").and_then(|v| v.as_str()).unwrap_or("none").to_string(),
                    ext: format.get("ext").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    width: format.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    height: format.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    abr: format.get("abr").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    tbr: format.get("tbr").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    protocol: format.get("protocol").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                },
            ))
        })
        .filter(|(_, stream)| stream.protocol != "m3u8" && stream.protocol != "m3u8_native")
        .collect()
}

fn protocol_rank(stream: &YtStream) -> u8 {
    if stream.protocol == "https" || stream.protocol == "http" {
        2
    } else if stream.protocol.is_empty() {
        1
    } else {
        0
    }
}

fn has_video(stream: &YtStream) -> bool {
    !stream.vcodec.is_empty() && stream.vcodec != "none"
}

fn has_audio(stream: &YtStream) -> bool {
    !stream.acodec.is_empty() && stream.acodec != "none"
}

fn audio_ok(stream: &YtStream) -> bool {
    stream.acodec.starts_with("mp4a") || stream.ext == "m4a"
}

fn video_ok(stream: &YtStream) -> bool {
    stream.vcodec.starts_with("avc1")
        || stream.vcodec.starts_with("hvc1")
        || stream.vcodec.starts_with("hev1")
        || stream.vcodec.starts_with("av01")
}

fn pick_progressive(streams: &[(String, YtStream)]) -> Option<YtPick> {
    streams
        .iter()
        .filter(|(_, stream)| {
            has_video(stream)
                && has_audio(stream)
                && video_ok(stream)
                && audio_ok(stream)
                && !stream.url.is_empty()
        })
        .filter(|(_, stream)| stream.height == 0 || stream.height <= 720)
        .max_by(|a, b| a.1.height.cmp(&b.1.height).then(a.1.tbr.total_cmp(&b.1.tbr)).then(protocol_rank(&a.1).cmp(&protocol_rank(&b.1))))
        .map(|(_, stream)| YtPick {
            url: stream.url.clone(),
            width: stream.width,
            height: stream.height,
            ext: stream.ext.clone(),
        })
}

fn pick_video(streams: &[(String, YtStream)]) -> Option<YtPick> {
    streams
        .iter()
        .filter(|(_, stream)| has_video(stream) && !has_audio(stream) && video_ok(stream))
        .filter(|(_, stream)| stream.height == 0 || stream.height <= 720)
        .max_by(|a, b| {
            a.1.vcodec.starts_with("avc1").cmp(&b.1.vcodec.starts_with("avc1"))
                .then(a.1.height.cmp(&b.1.height))
                .then(protocol_rank(&a.1).cmp(&protocol_rank(&b.1)))
        })
        .map(|(_, stream)| YtPick {
            url: stream.url.clone(),
            width: stream.width,
            height: stream.height,
            ext: stream.ext.clone(),
        })
}

fn pick_audio(streams: &[(String, YtStream)]) -> Option<YtPick> {
    streams
        .iter()
        .filter(|(_, stream)| has_audio(stream) && !has_video(stream))
        .filter(|(_, stream)| audio_ok(stream))
        .max_by(|a, b| a.1.abr.total_cmp(&b.1.abr).then(protocol_rank(&a.1).cmp(&protocol_rank(&b.1))))
        .map(|(_, stream)| YtPick {
            url: stream.url.clone(),
            width: 0,
            height: 0,
            ext: stream.ext.clone(),
        })
}

fn fill_media(info: &mut MediaInfo, duration: f64, width: u32, height: u32) {
    if !info.duration.is_finite() && duration.is_finite() {
        info.duration = duration;
    }
    if info.width == 0 {
        info.width = width;
    }
    if info.height == 0 {
        info.height = height;
    }
}

fn ytdlp_binary() -> Result<PathBuf, String> {
    trace("locating yt-dlp");
    if let Ok(path) = std::env::var("DJGROK_YTDL") {
        if !path.is_empty() {
            trace(format_args!("using DJGROK_YTDL binary={path}"));
            return Ok(PathBuf::from(path));
        }
    }
    if let Some(path) = which("yt-dlp") {
        trace(format_args!("found yt-dlp on PATH: {}", path.display()));
        return Ok(path);
    }
    let cache = std::env::temp_dir().join("sweet-yt-dlp");
    std::fs::create_dir_all(&cache).map_err(|err| err.to_string())?;
    let name = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" };
    let path = cache.join(name);
    if path.is_file() {
        trace(format_args!("using cached yt-dlp binary {}", path.display()));
        return Ok(path);
    }
    trace(format_args!("downloading standalone yt-dlp binary from {}", ytdlp_release_url()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    runtime
        .block_on(download_file(ytdlp_release_url(), &path))
        .map_err(|err| format!("could not download yt-dlp: {err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&path)
            .map_err(|err| err.to_string())?
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).map_err(|err| err.to_string())?;
    }
    Ok(path)
}

fn ytdlp_release_url() -> &'static str {
    if cfg!(windows) {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
    } else if cfg!(target_os = "macos") {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos"
    } else if cfg!(target_arch = "aarch64") {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux_aarch64"
    } else {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux"
    }
}

fn which(name: &str) -> Option<PathBuf> {
    let output = Command::new("which").arg(name).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let path = text.trim();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

