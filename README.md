# sweet

A retained DOM and a Tailwind layout painter for trusted desktop UI. Node.js is the JavaScript engine. It mutates the tree from outside. The renderer does not parse HTML, does not parse stylesheets, and does not run JavaScript.

## Why this engine

Servo’s DOM objects are JavaScript reflectors. Layout reads them through the script crate, and the embed API is an HTML webview. Taking HTML, CSS, and JavaScript out of Servo means deleting the crate the rest of the browser sits on.

The piece Servo actually uses for flex, grid, and block is [Taffy](https://docs.rs/taffy). sweet maps a closed set of Tailwind utilities onto Taffy’s style fields and paints the boxes itself. There is no Stylo and no CSS parser. `className` is the only styling channel. `setAttribute("style", ...)` throws.

The window is a thread in the Node process. It is not a child process and it is not a sandbox. This is for UI you trust.

## Use

```bash
npm run build
node examples/hello.js
```

DJ Grok defaults to the optimized release binding to give animation the full native rendering performance:

```bash
npm run build
node djgrok/title.js
```

The debug build is copied to `native/debug/sweet.node`, separate from the release binding.
Use `npm run build:debug` and `SWEET_NATIVE_BUILD=debug node djgrok/title.js` when debugging native code.
For rendering benchmarks, use `cargo run --release -p sweet-core --example frame_cost`;
debug builds are unoptimized and are not representative of rendering performance.

Carousel destinations are submitted once per track change. Native transitions run on the
window thread independently of JavaScript, and same-size video frames refresh their
animated layer without rebuilding scene layout. DOM/video redraws share the 60 Hz
frame schedule. `SWEET_PROF=1 node examples/carousel-stress.js` deliberately blocks
JavaScript for two seconds during a transition; optionally set `SWEET_STRESS_VIDEO`
to a local video file to include decoding. The `frame_cost` benchmark includes video
updates and reports p95, maximum, and counts over the 16.67 ms budget. These measure
CPU rendering; OS scheduling and display presentation can still cause missed frames.

```js
import { Document, Window } from "./index.js";

const doc = new Document();
doc.body.className = "flex flex-row h-screen bg-zinc-100";

const card = doc.createElement("div");
card.className = "m-8 p-6 bg-white rounded-lg text-xl";
card.textContent = "Hello";
card.on("click", (event) => {
  card.textContent = "Clicked";
});
doc.body.appendChild(card);

const win = new Window(doc, { title: "Sweet", width: 800, height: 600 });
await win.closed;
```

`Document` and `Node` talk to the tree directly. `Window` shows the current tree and sends `click`, `mousedown`, `mouseup`, `mousemove`, `scroll`, `resize`, and `close`. Wheel input scrolls the deepest eligible element and bubbles leftover movement to its ancestors. A resize event carries the logical width and height as `x` and `y`. A pending `closed` promise keeps Node alive until the window goes away. A `close` handler runs before that promise resolves, so the handler must not wait on `closed`. Pointer events for a turn are delivered before that turn's frame is painted, and a burst of DOM writes shares one frame.

Pass `borderless: true` to hide the native frame and request a transparent client surface. Windows are resizable by default; set `resizable: false` when creating a window or assign `win.resizable = false` later to disable resizing. Borderless windows show directional resize cursors at their edges. `opacity` sets borderless window opacity from `0.1` to `1` (default `1`). `dragAnywhere: true` lets the user move the window from any area without a `click` listener; clickable elements and text fields continue to receive input normally. `Window` also exposes `minimize()`, `toggleMaximize()`, and `close()`.

`doc.renderPng(width, height)` and `doc.layoutBoxes(width, height)` paint without opening a window.

`new Document()` is the document. `createElement`, `appendChild`, `classList`, `addEventListener`, and `MutationObserver` follow the DOM. `document.createElement('img')` makes an image element, and setting `src` to a data URL, file path, or `http` URL decodes the bytes in this process. `object-fill`, `object-contain`, and `object-cover` choose how the pixels fit the box. There is still no HTML parser, and `style` still throws. `node.on(type, fn)` is the same registration as `addEventListener`. An `input` or `textarea` is still a sweet element: Tailwind paints its box, and an egui text editor inside that box handles the caret, selection, IME, and shortcuts. It does not open its own window.

`document.createElement('video')` streams local files, direct URLs, and YouTube URLs in-process. `yscv-video` decodes H.264, HEVC, and AV1 from MP4/MKV; `symphonia` decodes the audio track and `rodio` plays it. Decoded frames use the same raster path as images, so Tailwind sizing, object fit, clipping, opacity, and 3D transforms compose normally. The element supports `src`, `play()`, `pause()`, `currentTime`, `duration`, `volume`, `muted`, `autoplay`, `loop`, and the usual playback events. Remote sources are downloaded to a temp file first, because the decoder seeks. YouTube resolution still uses a standalone `yt-dlp` binary: one already on `PATH`, `DJGROK_YTDL`, or a copy cached on first use. Playback itself does not need `ffmpeg`, `ffprobe`, or `mpv`.

## Compile the native binding on macOS

On the Mac, install Xcode Command Line Tools and Node.js. Rustup reads the pinned compiler version from `rust-toolchain.toml`.

```bash
xcode-select --install
brew install node
npm run build
```

This builds `native/sweet.node` for the Mac's current CPU architecture (Apple Silicon or Intel), with no Apple signing account or certificate. DJ Grok plays video without system `ffmpeg` or `mpv`. YouTube search still prefers `yt-dlp` on `PATH` (`brew install yt-dlp`); playback downloads a standalone `yt-dlp` binary on first use if it is missing.

The native code currently runs Winit's window loop on a worker thread, which macOS does not allow. The addon can be compiled this way, but DJ Grok needs a macOS main-thread event-loop integration before it can run there or be packaged as a usable Mac app.

`MutationObserver` reports `childList`, `attributes`, and `characterData`. `subtree` covers descendants. `dominant(document)` is the Dominant binding layer on that document: `d.el`, `d.text`, `d.if`, `d.map`, and `d.updateSync`. Call it before building the tree. `d.html` throws.

## Utilities

### Tailwind utilities

Display and flex: `block`, `flex`, `inline-flex`, `grid`, `inline-grid`, `hidden`, direction, wrap, `flex-1`, grow, shrink, `items-*`, `justify-*`, `content-*`, `self-*`, `place-*`, gap.

Grid: `grid-cols-N`, `grid-rows-N`, arbitrary track lists such as `grid-cols-[240px_1fr_20%]`, `col-span-*`, `row-span-*`, `col-start-*`, `row-start-*`. Arbitrary tracks accept lengths, percentages, viewport units, `min(...)`, `auto`, and fractional units.

Box: padding, margin (including negative and `auto`), inset, width, height, min and max size, fractions, `full`, `screen`, arbitrary `[Npx]`, `[Nrem]`, and `[N%]`.

Paint: palette colors and `[#hex]` for `bg`, `text`, and `border`; border widths; uniform `rounded-*`; opacity; `z-*`; `cursor-pointer`; `truncate`; whitespace; `aspect-square` and `aspect-video`; `pointer-events-none`.

Scrolling: `overflow-{auto,scroll,hidden,clip,visible}` and `overflow-x-*` / `overflow-y-*`; `scrollbar-width-{thin,auto,none}` or `scrollbar-w-{1,2,3}`; `scrollbar-gutter-stable`; `scrollbar-thumb-*`, `scrollbar-track-*`, and `scrollbar-radius-[Npx]`. Scrollbars are hidden by default; a width utility paints them. `scrollbar-color-*` aliases the thumb color.

Icons: use `nf` with any Nerd Fonts glyph class such as `nf-fa-github`, `nf-md-home`, or `nf-cod-account`. Sweet bundles Nerd Fonts SymbolsOnly, so icons do not depend on a system Nerd Font installation. Icon elements size to their glyph; Tailwind `text-*` utilities set their size and normal text color utilities set their color.

Variants: `hover` (the element and its ancestors), `focus` (the clicked element), `dark` (`doc.dark = true`), `sm` `md` `lg` `xl` `2xl`, and `!`. Responsive variants use viewport widths of 640, 768, 1024, 1280, and 1536 pixels; they override base utilities in increasing breakpoint order regardless of class-string order. Resizing relays out the existing DOM.

## Limits

Text nodes are block or flex items. Overflow utilities clip or scroll element content. Nodes expose `scrollLeft`, `scrollTop`, `scrollWidth`, `scrollHeight`, `clientWidth`, and `clientHeight`, plus `scroll()`, `scrollTo()`, and `scrollBy()`. These methods update the retained view immediately; smooth scrolling behavior is not implemented. `fixed` is absolute against the parent, because Taffy has no viewport-fixed layer. Radius is uniform. One window per document. `box-sizing` is border-box. Heading tags do not get a user-agent font size.

Fonts are Lato from the system, then DejaVu. `SWEET_FONT` overrides the regular face. `SWEET_FONT_DIR` loads `.ttf` files from a directory and recognizes weights in filenames such as `Regular`, `Medium`, and `SemiBold`.
