import { readFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { ytdlBin } from "./djgrok/youtube.js";

const NATIVE_EVENTS = new Set([
  "click",
  "mousedown",
  "mouseup",
  "mousemove",
  "input",
  "change",
  "keydown",
]);

/** Handle to the layout engine. Not part of the DOM surface. */
export const rustDocument = Symbol("sweet.rustDocument");

/**
 * Document and Node match the DOM shape: createElement, appendChild, attributes,
 * events, images, and MutationObserver. There is no second document type.
 * `style` is rejected. Tailwind classes are the only styling channel.
 */

export class MutationObserver {
  constructor(callback) {
    this.callback = callback;
    this._roots = [];
    this._session = null;
    this._queued = [];
  }

  observe(target, options = {}) {
    if (!options || (!options.childList && !options.attributes && !options.characterData)) {
      throw new TypeError("MutationObserver needs childList, attributes, or characterData");
    }
    const session = target._session;
    if (!session) throw new TypeError("observe target is not a sweet node");
    this._session = session;
    this._roots.push({
      target,
      subtree: !!options.subtree,
      childList: !!options.childList,
      attributes: !!options.attributes,
      characterData: !!options.characterData,
      attributeOldValue: !!options.attributeOldValue,
      characterDataOldValue: !!options.characterDataOldValue,
      attributeFilter: options.attributeFilter ? new Set(options.attributeFilter) : null,
    });
    session.observers.add(this);
  }

  disconnect() {
    this._roots = [];
    this._session?.observers.delete(this);
    this._session = null;
  }

  takeRecords() {
    const records = this._queued;
    this._queued = [];
    return records;
  }
}

class Session {
  constructor(rust, document) {
    document._session = this;
    document[rustDocument] = rust;
    this.rust = rust;
    this.nodes = new Map();
    this.observers = new Set();
    this.pending = [];
    this.scheduled = false;
    this.document = document;
    this.bodyId = rust.body.nodeId;
    this.body = this.wrap(rust.body);
  }

  wrap(native) {
    const id = native.nodeId;
    let node = this.nodes.get(id);
    if (!node) {
      node = new Node(native, this);
      this.nodes.set(id, node);
    }
    return node;
  }

  record(record) {
    this.pending.push(record);
    if (this.scheduled) return;
    this.scheduled = true;
    queueMicrotask(() => {
      this.scheduled = false;
      const records = this.pending;
      this.pending = [];
      for (const observer of this.observers) {
        const matched = [];
        for (const record of records) {
          const root = observerRoot(observer, record);
          if (!root) continue;
          matched.push(presentRecord(root, record));
        }
        if (matched.length === 0) continue;
        observer._queued.push(...matched);
        try {
          observer.callback(observer.takeRecords(), observer);
        } catch (error) {
          console.error(error);
        }
      }
    });
  }
}

function observerRoot(observer, record) {
  for (const root of observer._roots) {
    const hit = root.target === record.target || (root.subtree && root.target.contains(record.target));
    if (!hit) continue;
    if (record.type === "childList" && root.childList) return root;
    if (record.type === "attributes" && root.attributes) {
      if (root.attributeFilter && !root.attributeFilter.has(record.attributeName)) continue;
      return root;
    }
    if (record.type === "characterData" && root.characterData) return root;
  }
  return null;
}

function presentRecord(root, record) {
  if (record.type === "attributes" && !root.attributeOldValue) return { ...record, oldValue: null };
  if (record.type === "characterData" && !root.characterDataOldValue) {
    return { ...record, oldValue: null };
  }
  return record;
}

class SweetDocument {
  constructor() {
    this.nodeType = 9;
    this.nodeName = "#document";
  }

  get body() {
    return this._session.body;
  }

  get dark() {
    return this._session.rust.dark;
  }

  set dark(value) {
    this._session.rust.dark = !!value;
  }

  get childNodes() {
    return [this.body];
  }

  createElement(tag) {
    return this._session.wrap(this._session.rust.createElement(tag));
  }

  createTextNode(text) {
    return this._session.wrap(this._session.rust.createText(text == null ? "" : String(text)));
  }

  createComment(data) {
    return this._session.wrap(this._session.rust.createComment(data == null ? "" : String(data)));
  }

  createElementNS(namespace, tag) {
    const element = this.createElement(tag);
    element._namespace = namespace;
    return element;
  }

  contains(node) {
    let current = node;
    while (current) {
      if (current === this) return true;
      current = current.parentNode;
    }
    return false;
  }

  renderPng(width, height) {
    return this._session.rust.renderPng(width, height);
  }

  layoutBoxes(width, height) {
    return this._session.rust.layoutBoxes(width, height);
  }
}

class Node {
  #rust;

  constructor(rust, session) {
    this.#rust = rust;
    this._session = session;
    this._listeners = new Map();
    this._nativeEvents = new Set();
    this.complete = false;
    this.naturalWidth = 0;
    this.naturalHeight = 0;
    this._imageToken = 0;
    this._attrs = new Map();
    this._namespace = null;
    this._mediaToken = 0;
    this._media = null;
    this._mediaPromise = null;
    this._currentTime = 0;
    this._duration = NaN;
    this._paused = true;
    this._ended = false;
    this._volume = 1;
    this._muted = false;
    this._videoWidth = 0;
    this._videoHeight = 0;
    this._mediaClock = null;
  }

  get namespaceURI() {
    if (this.nodeType !== 1) return null;
    return this._namespace || "http://www.w3.org/1999/xhtml";
  }

  get nodeType() {
    return this.#rust.nodeType;
  }

  get nodeId() {
    return this.#rust.nodeId;
  }

  get nodeName() {
    return this.tagName;
  }

  get tagName() {
    const name = this.#rust.tagName;
    return this.nodeType === 1 ? name.toUpperCase() : name;
  }

  get ownerDocument() {
    return this._session.document;
  }

  get parentNode() {
    const parent = this.#rust.parent();
    if (parent) return this._session.wrap(parent);
    if (this.#rust.nodeId === this._session.bodyId) return this._session.document;
    return null;
  }

  get childNodes() {
    return this.#rust.children().map((child) => this._session.wrap(child));
  }

  get nextSibling() {
    return this.#sibling(1);
  }

  get previousSibling() {
    return this.#sibling(-1);
  }

  #sibling(delta) {
    const parent = this.parentNode;
    if (!parent || parent.nodeType === 9) return null;
    const children = parent.childNodes;
    const index = children.indexOf(this);
    if (index < 0) return null;
    return children[index + delta] || null;
  }

  contains(node) {
    let current = node;
    while (current) {
      if (current === this) return true;
      current = current.parentNode;
    }
    return false;
  }

  get className() {
    return this.#rust.className;
  }

  set className(value) {
    const previous = this.#rust.className;
    const next = value == null ? "" : String(value);
    this.#rust.className = next;
    this._attrs.set("class", next);
    if (previous !== next) this.#attributeRecord("class", previous);
  }

  get classList() {
    return new ClassList(this);
  }

  get textContent() {
    return this.#rust.textContent;
  }

  set textContent(value) {
    const next = value == null ? "" : String(value);
    if (this.nodeType === 3 || this.nodeType === 8) {
      const previous = this.#rust.textContent;
      this.#rust.textContent = next;
      if (previous !== next) {
        this._session.record({
          type: "characterData",
          target: this,
          oldValue: previous,
          addedNodes: [],
          removedNodes: [],
          previousSibling: null,
          nextSibling: null,
          attributeName: null,
        });
      }
      return;
    }
    for (const child of this.childNodes.slice()) this.removeChild(child);
    if (next !== "") this.appendChild(this.ownerDocument.createTextNode(next));
  }

  get id() {
    return this.#rust.id;
  }

  set id(value) {
    const previous = this.#rust.id;
    const next = value == null ? "" : String(value);
    this.#rust.id = next;
    this._attrs.set("id", next);
    if (previous !== next) this.#attributeRecord("id", previous);
  }

  get value() {
    if (this.#isField()) return this.#rust.value;
    return this._value;
  }

  set value(value) {
    const text = value == null ? "" : String(value);
    if (this.#isField()) {
      this.#rust.value = text;
      return;
    }
    this._value = text;
  }

  get placeholder() {
    return this.getAttribute("placeholder") || "";
  }

  set placeholder(value) {
    this.setAttribute("placeholder", value == null ? "" : String(value));
  }

  get type() {
    return this.getAttribute("type") || (this.tagName === "INPUT" ? "text" : "");
  }

  set type(value) {
    this.setAttribute("type", value == null ? "" : String(value));
  }

  get disabled() {
    return !!this.#rust.disabled;
  }

  set disabled(value) {
    this.#rust.disabled = !!value;
  }

  #isField() {
    return this.tagName === "INPUT" || this.tagName === "TEXTAREA";
  }

  get src() {
    return this.getAttribute("src") || "";
  }

  set src(value) {
    this.setAttribute("src", value == null ? "" : String(value));
  }

  get currentSrc() {
    return this._media?.source || this.src;
  }

  get paused() {
    return this.tagName === "VIDEO" ? this._paused : true;
  }

  get ended() {
    return this.tagName === "VIDEO" ? this._ended : false;
  }

  get duration() {
    return this.tagName === "VIDEO" ? this._duration : NaN;
  }

  get currentTime() {
    if (this.tagName !== "VIDEO") return 0;
    if (!this._paused && this._media?.startedAt) {
      const elapsed = (performance.now() - this._media.startedAt) / 1000;
      return Math.max(0, this._media.startTime + elapsed);
    }
    return this._currentTime;
  }

  set currentTime(value) {
    if (this.tagName !== "VIDEO") return;
    const next = Math.max(0, Number(value) || 0);
    this._currentTime = Number.isFinite(this._duration) ? Math.min(next, this._duration) : next;
    this._ended = false;
    if (!this._paused) this.#restartMedia();
  }

  get volume() {
    return this._volume;
  }

  set volume(value) {
    this._volume = Math.max(0, Math.min(1, Number(value) || 0));
    this.#setMediaVolume();
  }

  get muted() {
    return this._muted;
  }

  set muted(value) {
    this._muted = !!value;
    this.#setMediaVolume();
  }

  get videoWidth() {
    return this._videoWidth;
  }

  get videoHeight() {
    return this._videoHeight;
  }

  get readyState() {
    if (this.tagName !== "VIDEO") return 0;
    return this._videoWidth > 0 ? 4 : this._media ? 1 : 0;
  }

  get autoplay() {
    return this.hasAttribute("autoplay");
  }

  set autoplay(value) {
    this.#toggleAttribute("autoplay", value);
  }

  get loop() {
    return this.hasAttribute("loop");
  }

  set loop(value) {
    this.#toggleAttribute("loop", value);
  }

  get controls() {
    return this.hasAttribute("controls");
  }

  set controls(value) {
    this.#toggleAttribute("controls", value);
  }

  #toggleAttribute(name, value) {
    if (value) this.setAttribute(name, "");
    else this.removeAttribute(name);
  }

  hasAttribute(name) {
    return this.getAttribute(name) != null;
  }

  setAttribute(name, value) {
    const text = value == null ? "" : String(value);
    const previous = this.getAttribute(name);
    this.#rust.setAttribute(name, text);
    this._attrs.set(name, text);
    if (this.tagName === "IMG" && name === "src") this.#load(text);
    if (this.tagName === "VIDEO" && name === "src") this.#loadMedia(text);
    if (this.tagName === "VIDEO" && name === "muted") this.muted = true;
    if (previous !== text) this.#attributeRecord(name, previous);
  }

  getAttribute(name) {
    const value = this.#rust.getAttribute(name);
    return value == null ? null : value;
  }

  removeAttribute(name) {
    const previous = this.getAttribute(name);
    this.#rust.removeAttribute(name);
    this._attrs.delete(name);
    if (this.tagName === "IMG" && name === "src") this.#load("");
    if (this.tagName === "VIDEO" && name === "src") this.#loadMedia("");
    if (this.tagName === "VIDEO" && name === "muted") this.muted = false;
    if (previous != null) this.#attributeRecord(name, previous);
  }

  cloneNode(deep = false) {
    const document = this.ownerDocument;
    if (this.nodeType === 3) return document.createTextNode(this.textContent);
    if (this.nodeType === 8) return document.createComment(this.textContent);
    const copy = this._namespace
      ? document.createElementNS(this._namespace, this.#rust.tagName)
      : document.createElement(this.#rust.tagName);
    if (this.className) copy.className = this.className;
    if (this.id) copy.id = this.id;
    for (const [name, value] of this._attrs) {
      if (name === "class" || name === "id") continue;
      copy.setAttribute(name, value);
    }
    if (deep) {
      for (const child of this.childNodes) copy.appendChild(child.cloneNode(true));
    }
    return copy;
  }

  #attributeRecord(name, oldValue) {
    this._session.record({
      type: "attributes",
      target: this,
      attributeName: name,
      oldValue,
      addedNodes: [],
      removedNodes: [],
      previousSibling: null,
      nextSibling: null,
    });
  }

  appendChild(child) {
    return this.insertBefore(child, null);
  }

  insertBefore(child, before) {
    if (!child || child._session !== this._session) {
      throw new Error("node belongs to another document");
    }
    const oldParent = child.parentNode;
    const oldNext = child.nextSibling;
    this.#rust.insertBefore(child.#rust, before ? before.#rust : undefined);
    if (oldParent && oldParent.nodeType !== 9) {
      this._session.record({
        type: "childList",
        target: oldParent,
        addedNodes: [],
        removedNodes: [child],
        previousSibling: null,
        nextSibling: oldNext,
      });
    }
    this._session.record({
      type: "childList",
      target: this,
      addedNodes: [child],
      removedNodes: [],
      previousSibling: child.previousSibling,
      nextSibling: child.nextSibling,
    });
    return child;
  }

  removeChild(child) {
    const next = child.nextSibling;
    const previous = child.previousSibling;
    this.#rust.removeChild(child.#rust);
    this._session.record({
      type: "childList",
      target: this,
      addedNodes: [],
      removedNodes: [child],
      previousSibling: previous,
      nextSibling: next,
    });
    return child;
  }

  remove() {
    const parent = this.parentNode;
    if (parent && parent.nodeType !== 9) parent.removeChild(this);
  }

  on(type, callback) {
    this.addEventListener(type, callback);
  }

  addEventListener(type, callback, options) {
    if (typeof callback !== "function") return;
    const once = !!(options && options.once);
    let list = this._listeners.get(type);
    if (!list) {
      list = [];
      this._listeners.set(type, list);
    }
    list.push({ callback, once });
    if (NATIVE_EVENTS.has(type) && !this._nativeEvents.has(type)) {
      this._nativeEvents.add(type);
      this.#rust.on(type, (event) => this.#dispatch(type, event));
    }
  }

  removeEventListener(type, callback) {
    const list = this._listeners.get(type);
    if (!list) return;
    const index = list.findIndex((entry) => entry.callback === callback);
    if (index >= 0) list.splice(index, 1);
  }

  dispatchEvent(event) {
    this.#dispatch(event.type, event);
    return true;
  }

  #dispatch(type, detail) {
    const list = this._listeners.get(type);
    if (!list || list.length === 0) return;
    const event = {
      type,
      target: this,
      currentTarget: this,
      bubbles: false,
      ...detail,
    };
    for (const entry of list.slice()) {
      if (entry.once) this.removeEventListener(type, entry.callback);
      entry.callback.call(this, event);
    }
  }

  decode() {
    if (this.tagName !== "IMG") return Promise.reject(new Error("decode is for img elements"));
    if (this.complete && this.naturalWidth > 0) return Promise.resolve();
    return new Promise((resolve, reject) => {
      this.addEventListener("load", () => resolve(), { once: true });
      this.addEventListener("error", () => reject(new Error("image failed to load")), { once: true });
    });
  }

  load() {
    if (this.tagName !== "VIDEO") return;
    this.#loadMedia(this.src);
  }

  async play() {
    if (this.tagName !== "VIDEO") return;
    if (!this.src) throw new Error("video has no source");
    if (!this._paused && this._media?.video) return;
    this._paused = false;
    this._ended = false;
    try {
      const media = await this.#resolveMedia(this.src);
      if (this._paused || media.token !== this._mediaToken) return;
      this.#spawnMedia(media);
      this.#dispatch("play", {});
      this.#dispatch("playing", {});
    } catch (error) {
      this._paused = true;
      this.#dispatch("error", { error });
      throw error;
    }
  }

  pause() {
    if (this.tagName !== "VIDEO" || this._paused) return;
    this._currentTime = this.currentTime;
    this._paused = true;
    this.#stopMedia(false);
    this.#dispatch("pause", {});
  }

  #loadMedia(source) {
    const wasPlaying = !this._paused;
    this._mediaToken += 1;
    this.#stopMedia(true);
    this._media = null;
    this._mediaPromise = null;
    this._currentTime = 0;
    this._duration = NaN;
    this._ended = false;
    this._videoWidth = 0;
    this._videoHeight = 0;
    this.#rust.clearImage();
    if (!source) {
      this._paused = true;
      this.#dispatch("emptied", {});
      return;
    }
    this.#dispatch("loadstart", {});
    if (wasPlaying || this.autoplay) {
      this._paused = false;
      this.play().catch(() => {});
    }
  }

  #resolveMedia(source) {
    if (this._media?.source === source) return Promise.resolve(this._media);
    if (this._mediaPromise) return this._mediaPromise;
    const token = this._mediaToken;
    this._mediaPromise = resolveMedia(source).then((resolved) => {
      if (token !== this._mediaToken) throw new Error("video source changed");
      this._media = { ...resolved, source, token, video: null, audio: null, startedAt: 0, startTime: 0 };
      this._duration = resolved.duration;
      this._videoWidth = resolved.width || 0;
      this._videoHeight = resolved.height || 0;
      this.#dispatch("loadedmetadata", {});
      this.#dispatch("durationchange", {});
      return this._media;
    }).finally(() => {
      this._mediaPromise = null;
    });
    return this._mediaPromise;
  }

  #restartMedia() {
    this.#stopMedia(false);
    const media = this._media;
    if (media) this.#spawnMedia(media);
    else this.play().catch(() => {});
  }

  #spawnMedia(media) {
    this.#stopMedia(false);
    const width = 480;
    const height = 270;
    const frameBytes = width * height * 4;
    const start = Math.max(0, this._currentTime || 0);
    const seek = start > 0 ? ["-ss", String(start)] : [];
    const vf = `scale=${width}:${height}:force_original_aspect_ratio=decrease,pad=${width}:${height}:(ow-iw)/2:(oh-ih)/2,fps=12`;
    const video = spawn("ffmpeg", [
      "-nostdin", "-loglevel", "error", "-re", ...seek,
      ...(media.videoHeaders ? ["-headers", media.videoHeaders] : []),
      "-i", media.videoUrl,
      "-an", "-vf", vf, "-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1",
    ], { stdio: ["ignore", "pipe", "pipe"] });
    const audio = media.audioUrl && !this._muted
      ? spawn("mpv", [
          "--no-video", "--no-terminal", "--really-quiet", "--force-window=no",
          `--start=${start}`, `--volume=${Math.round(this._volume * 100)}`, media.audioUrl,
        ], { stdio: "ignore" })
      : null;
    media.video = video;
    media.audio = audio;
    media.startedAt = performance.now();
    media.startTime = start;
    this._mediaClock = setInterval(() => {
      if (this._paused || media.token !== this._mediaToken || media.video !== video) return;
      this._currentTime = this.currentTime;
      this.#dispatch("timeupdate", {});
    }, 250);
    let pending = Buffer.alloc(0);
    let first = true;
    video.stdout.on("data", (chunk) => {
      if (media.token !== this._mediaToken || media.video !== video) return;
      pending = pending.length ? Buffer.concat([pending, chunk]) : chunk;
      while (pending.length >= frameBytes) {
        const frame = pending.subarray(0, frameBytes);
        pending = pending.subarray(frameBytes);
        this.#rust.setImageRgba(width, height, frame);
        this._videoWidth = width;
        this._videoHeight = height;
        if (first) {
          first = false;
          this.#dispatch("loadeddata", {});
          this.#dispatch("canplay", {});
        }
      }
    });
    let decoderError = "";
    video.stderr.on("data", (chunk) => {
      if (decoderError.length < 4000) decoderError += chunk.toString();
    });
    video.once("error", (error) => {
      if (media.token === this._mediaToken && media.video === video) this.#dispatch("error", { error });
    });
    video.once("close", (code) => {
      if (media.token !== this._mediaToken || media.video !== video) return;
      if (this._mediaClock) {
        clearInterval(this._mediaClock);
        this._mediaClock = null;
      }
      media.video = null;
      if (media.audio) media.audio.kill("SIGKILL");
      media.audio = null;
      if (this._paused) return;
      if (code !== 0) {
        this._currentTime = this.currentTime;
        this._paused = true;
        const detail = decoderError.trim();
        this.#dispatch("error", { error: new Error(detail || `video decoder exited with ${code}`) });
        return;
      }
      this._currentTime = Number.isFinite(this._duration) ? this._duration : this.currentTime;
      this._paused = true;
      this._ended = true;
      if (code === 0) {
        this.#dispatch("timeupdate", {});
        this.#dispatch("ended", {});
        if (this.loop) {
          this._currentTime = 0;
          this.play().catch(() => {});
        }
      }
    });
  }

  #stopMedia(clearFrame) {
    if (this._mediaClock) {
      clearInterval(this._mediaClock);
      this._mediaClock = null;
    }
    const media = this._media;
    if (media?.video) {
      media.video.removeAllListeners("close");
      media.video.kill("SIGKILL");
      media.video = null;
    }
    if (media?.audio) {
      media.audio.kill("SIGKILL");
      media.audio = null;
    }
    if (clearFrame) this.#rust.clearImage();
  }

  #setMediaVolume() {
    if (!this._paused) this.#restartMedia();
  }

  #load(url) {
    const token = ++this._imageToken;
    this.complete = false;
    if (!url) {
      this.naturalWidth = 0;
      this.naturalHeight = 0;
      this.#rust.clearImage();
      this.complete = true;
      return;
    }
    readImage(url).then(
      (bytes) => {
        if (token !== this._imageToken) return;
        try {
          const size = this.#rust.setImageEncoded(bytes);
          this.naturalWidth = size.width;
          this.naturalHeight = size.height;
          this.complete = true;
          this.#dispatch("load", {});
        } catch (error) {
          this.complete = true;
          this.#dispatch("error", { error });
        }
      },
      (error) => {
        if (token !== this._imageToken) return;
        this.complete = true;
        this.#dispatch("error", { error });
      },
    );
  }
}

class ClassList {
  constructor(node) {
    this._node = node;
  }

  #tokens() {
    return this._node.className.split(/\s+/).filter(Boolean);
  }

  #write(tokens) {
    this._node.className = tokens.join(" ");
  }

  add(...names) {
    const tokens = this.#tokens();
    for (const name of names) {
      if (name && !tokens.includes(name)) tokens.push(name);
    }
    this.#write(tokens);
  }

  remove(...names) {
    const drop = new Set(names);
    this.#write(this.#tokens().filter((token) => !drop.has(token)));
  }

  contains(name) {
    return this.#tokens().includes(name);
  }

  get value() {
    return this._node.className;
  }
}

async function readImage(url) {
  if (url.startsWith("data:")) {
    const comma = url.indexOf(",");
    if (comma < 0) throw new Error("bad image data url");
    const meta = url.slice(5, comma);
    const data = url.slice(comma + 1);
    if (/;base64/i.test(meta)) return Buffer.from(data, "base64");
    return Buffer.from(decodeURIComponent(data));
  }
  if (url.startsWith("http://") || url.startsWith("https://")) {
    const response = await fetch(url);
    if (!response.ok) throw new Error("image request failed");
    return Buffer.from(await response.arrayBuffer());
  }
  const path = url.startsWith("file://") ? fileURLToPath(url) : url;
  return readFile(path);
}

function runJson(command, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { stdio: ["ignore", "pipe", "pipe"] });
    const stdout = [];
    const stderr = [];
    const timer = setTimeout(() => child.kill("SIGKILL"), 45000);
    child.stdout.on("data", (chunk) => stdout.push(chunk));
    child.stderr.on("data", (chunk) => stderr.push(chunk));
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once("close", (code) => {
      clearTimeout(timer);
      if (code !== 0) {
        reject(new Error(Buffer.concat(stderr).toString().trim() || `${command} exited with ${code}`));
        return;
      }
      try {
        resolve(JSON.parse(Buffer.concat(stdout).toString()));
      } catch (error) {
        reject(error);
      }
    });
  });
}

function isYouTube(url) {
  try {
    const host = new URL(url).hostname.replace(/^www\./, "");
    return host === "youtube.com" || host.endsWith(".youtube.com") || host === "youtu.be";
  } catch {
    return false;
  }
}

async function resolveMedia(source) {
  if (isYouTube(source)) {
    const info = await runJson(await ytdlBin(), [
      "--no-playlist", "--no-warnings", "-f",
      "bv*[height<=720][vcodec^=avc1]+ba[ext=m4a]/bv*[height<=720]+ba/b[height<=720]/b",
      "--dump-single-json", source,
    ]);
    const formats = info.requested_formats || [];
    const video = formats.find((item) => item.vcodec && item.vcodec !== "none") || info;
    const audio = formats.find((item) => item.acodec && item.acodec !== "none") || video;
    if (!video.url) throw new Error("YouTube did not return a playable video stream");
    return {
      videoUrl: video.url,
      audioUrl: audio.url || video.url,
      videoHeaders: formatHeaders(video.http_headers || info.http_headers),
      duration: Number(info.duration) || NaN,
      width: Number(video.width || info.width) || 0,
      height: Number(video.height || info.height) || 0,
    };
  }
  let input = source;
  if (source.startsWith("file://")) input = fileURLToPath(source);
  const info = await runJson("ffprobe", [
    "-v", "error", "-show_entries", "format=duration:stream=codec_type,width,height",
    "-of", "json", input,
  ]);
  const stream = (info.streams || []).find((item) => item.codec_type === "video") || {};
  return {
    videoUrl: input,
    audioUrl: input,
    duration: Number(info.format?.duration) || NaN,
    width: Number(stream.width) || 0,
    height: Number(stream.height) || 0,
  };
}

function formatHeaders(headers) {
  if (!headers || typeof headers !== "object") return "";
  return Object.entries(headers).map(([name, value]) => `${name}: ${value}\r\n`).join("");
}

export function bind(addon) {
  class Document extends SweetDocument {
    constructor() {
      super();
      new Session(new addon.Document(), this);
    }
  }
  return { Document, Node };
}
