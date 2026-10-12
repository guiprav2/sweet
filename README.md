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

`Document` and `Node` talk to the tree directly. `Window` shows the current tree and sends `click`, `mousedown`, `mouseup`, `mousemove`, `resize`, and `close`. A resize event carries the logical width and height as `x` and `y`. A pending `closed` promise keeps Node alive until the window goes away. A `close` handler runs before that promise resolves, so the handler must not wait on `closed`. Pointer events for a turn are delivered before that turn's frame is painted, and a burst of DOM writes shares one frame.

Pass `borderless: true` to hide the native frame. `dragAnywhere: true` lets the user move the window from any area without a `click` listener; clickable elements and text fields continue to receive input normally.

`doc.renderPng(width, height)` and `doc.layoutBoxes(width, height)` paint without opening a window.

`new Document()` is the document. `createElement`, `appendChild`, `classList`, `addEventListener`, and `MutationObserver` follow the DOM. `document.createElement('img')` makes an image element, and setting `src` to a data URL, file path, or `http` URL decodes the bytes in this process. `object-fill`, `object-contain`, and `object-cover` choose how the pixels fit the box. There is still no HTML parser, and `style` still throws. `node.on(type, fn)` is the same registration as `addEventListener`. An `input` or `textarea` is still a sweet element: Tailwind paints its box, and an egui text editor inside that box handles the caret, selection, IME, and shortcuts. It does not open its own window.

`document.createElement('video')` streams local files, direct URLs, and YouTube URLs through `ffmpeg`; audio is played by `mpv`. Its decoded frames use the same raster path as images, so Tailwind sizing, object fit, clipping, opacity, and 3D transforms compose normally. The element supports `src`, `play()`, `pause()`, `currentTime`, `duration`, `volume`, `muted`, `autoplay`, `loop`, and the usual playback events. `ffmpeg`, `ffprobe`, `mpv`, and `yt-dlp` must be on `PATH`.

## Build DJ Grok for macOS

Build on the Mac that will produce the app. Install Xcode Command Line Tools, Node.js, Rust (the pinned version is in `rust-toolchain.toml`), and the media commands:

```bash
xcode-select --install
brew install node ffmpeg mpv yt-dlp
npm run bundle:macos
```

The unsigned `dist/DJ Grok.app` is built for the Mac's current CPU architecture (Apple Silicon or Intel). The bundle contains Node, Sweet's native addon, DJ Grok, fonts, and cover art. `ffmpeg`, `ffprobe`, `mpv`, and `yt-dlp` are runtime requirements installed on the Mac; the launcher adds standard Homebrew paths so Finder can find them. DJ Grok saves its library and API key under `~/Library/Application Support/DJ Grok`.

To compile only the Sweet addon and Node binding, run `npm run build`. Neither command uses an Apple Developer account or signing certificate.

`MutationObserver` reports `childList`, `attributes`, and `characterData`. `subtree` covers descendants. `dominant(document)` is the Dominant binding layer on that document: `d.el`, `d.text`, `d.if`, `d.map`, and `d.updateSync`. Call it before building the tree. `d.html` throws.

## Utilities

Display and flex: `block`, `flex`, `inline-flex`, `grid`, `inline-grid`, `hidden`, direction, wrap, `flex-1`, grow, shrink, `items-*`, `justify-*`, `content-*`, `self-*`, `place-*`, gap.

Grid: `grid-cols-N`, `grid-rows-N`, `col-span-*`, `row-span-*`, `col-start-*`, `row-start-*`.

Box: padding, margin (including negative and `auto`), inset, width, height, min and max size, fractions, `full`, `screen`, arbitrary `[Npx]`, `[Nrem]`, and `[N%]`.

Paint: palette colors and `[#hex]` for `bg`, `text`, and `border`; border widths; uniform `rounded-*`; opacity; `z-*`; `cursor-pointer`; `truncate`; whitespace; `aspect-square` and `aspect-video`; `pointer-events-none`.

Variants: `hover` (the element and its ancestors), `focus` (the clicked element), `dark` (`doc.dark = true`), `sm` `md` `lg` `xl` `2xl`, and `!`.

## Limits

Text nodes are block or flex items. There is no inline flow and no scroll. `overflow-hidden` clips. `fixed` is absolute against the parent, because Taffy has no viewport-fixed layer. Radius is uniform. One window per document. `box-sizing` is border-box. Heading tags do not get a user-agent font size.

Fonts are Lato from the system, then DejaVu. `SWEET_FONT` overrides the regular face. `SWEET_FONT_DIR` loads `.ttf` files from a directory and recognizes weights in filenames such as `Regular`, `Medium`, and `SemiBold`.
