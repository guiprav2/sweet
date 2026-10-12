import { dirname, fileURLToPath, join } from "../path.qjs.js";
import { Document, Window, dominant } from "../index.qjs.js";

const here = dirname(fileURLToPath(import.meta.url));
const logo = join(here, "logo.png");
const photo = join(here, "../examples/ANh8R.jpg");
const doc = new Document();
const d = dominant(doc);
doc.body.className = "h-screen w-full bg-[#070709] text-white";

const state = {
  apiKey: "",
  keyError: "",
  keyBusy: false,
  entered: false,
  index: 2,
};

// The window opens at 960, which is the original app's narrow cover-flow.
const viewWidth = 960;

const covers = [
  { title: "Velvet", tint: "bg-fuchsia-700" },
  { title: "Night Drive", src: logo },
  { title: "Glass Room", src: photo },
  { title: "Afterglow", tint: "bg-rose-500" },
  { title: "Static", tint: "bg-indigo-600" },
];

function pose(offset) {
  const abs = Math.abs(offset);
  if (abs > 5) return "pointer-events-none opacity-0";
  const narrow = viewWidth < 1024;
  const shift = (narrow ? [0, 112, 150, 178, 198, 214] : [0, 228, 292, 348, 396, 436])[abs];
  const depth = [90, -20, -64, -104, -140, -172][abs];
  const turn = offset < 0 ? 50 : offset > 0 ? -50 : 0;
  const size = [1, 0.98, 0.94, 0.9, 0.86, 0.82][abs];
  const x = offset < 0 ? -shift : shift;
  const opacity = ["opacity-100", "opacity-100", "opacity-95", "opacity-82", "opacity-68", "opacity-52"][abs];
  const layer = ["z-40", "z-[28]", "z-[22]", "z-[16]", "z-[12]", "z-[8]"][abs];
  return `${layer} ${opacity} [transform:translateX(${x}px)_translateZ(${depth}px)_rotateY(${turn}deg)_scale(${size})]`;
}

function saveKey() {
  if (state.keyBusy || state.entered) return;
  const key = state.apiKey.trim();
  if (key.length < 20) {
    state.keyError = "Enter a Grok API key.";
    d.updateSync();
    return;
  }
  state.keyError = "";
  state.keyBusy = false;
  state.entered = true;
  d.updateSync();
  console.log("title screen accepted a key of length", key.length);
}

function step(delta) {
  state.index = (state.index + delta + covers.length) % covers.length;
  d.updateSync();
}

function coverFace(cover) {
  if (cover.src) {
    return d.el("img", { src: cover.src, class: "h-full w-full object-cover" });
  }
  return d.el("div", { class: `h-full w-full ${cover.tint}` });
}

function coverReflection(cover) {
  if (cover.src) {
    return d.el("img", {
      src: cover.src,
      class: "h-full w-full object-cover object-bottom [transform:scaleY(-1)]",
    });
  }
  return d.el("div", { class: `h-full w-full ${cover.tint} [transform:scaleY(-1)]` });
}

function coverCard(cover, index) {
  return d.el(
    "div",
    {
      class: [
        "absolute inset-0 origin-center cursor-pointer transition-all duration-700 ease-out",
        () => pose(index - state.index),
      ],
      onclick: () => {
        state.index = index;
        d.updateSync();
      },
    },
    d.el(
      "div",
      {
        class: [
          "absolute inset-0 overflow-hidden rounded-xl bg-zinc-900",
          () => (index === state.index ? "ring-2 ring-white/85" : "ring-1 ring-white/10"),
        ],
      },
      coverFace(cover),
      d.el("div", {
        class: "pointer-events-none absolute inset-0 bg-gradient-to-tr from-black/30 via-transparent to-white/25",
      }),
    ),
    d.el(
      "div",
      { class: "pointer-events-none absolute inset-x-0 top-[103%] h-[42%] overflow-hidden" },
      coverReflection(cover),
      d.el("div", {
        class:
          "pointer-events-none absolute inset-0 bg-gradient-to-b from-[#070709]/45 via-[#070709]/82 to-[#070709]",
      }),
    ),
  );
}

function titleScreen() {
  return d.el(
    "div",
    { class: "relative h-screen w-full grid place-items-center overflow-hidden bg-[#070709] px-6" },
    d.el("div", {
      class:
        "pointer-events-none absolute left-1/2 top-[34%] h-72 w-72 -translate-x-1/2 -translate-y-1/2 rounded-full bg-rose-400/30 blur-3xl animate-glow",
    }),
    d.el(
      "div",
      { class: "relative z-10 flex w-full max-w-sm flex-col items-center" },
      d.el(
        "div",
        { class: "animate-float" },
        d.el(
          "div",
          { class: "relative h-44 w-44" },
          d.el("img", { src: logo, class: "h-full w-full animate-spin-slow" }),
          d.el(
            "div",
            { class: "pointer-events-none absolute inset-[8%] overflow-hidden rounded-full" },
            d.el("div", {
              class:
                "absolute inset-y-[-10%] w-1/3 bg-gradient-to-r from-transparent via-white/55 to-transparent animate-sheen",
            }),
          ),
        ),
      ),
      d.el("div", { class: "mt-7 text-3xl font-semibold" }, "DJ Grok"),
      d.el(
        "div",
        { class: "mt-2 text-center text-sm text-white/55" },
        "A Grok API key opens the booth. It stays in this player until you forget it.",
      ),
      d.el("div", { class: "mt-7 w-full text-left text-[11px] font-medium text-white/40" }, "GROK API KEY"),
      d.el("input", {
        type: "password",
        placeholder: "xai-…",
        class:
          "mt-2 w-full rounded-2xl border border-white/10 bg-white/5 px-4 py-3 text-sm text-white focus:border-rose-300/70",
        value: d.binding({
          get: () => state.apiKey,
          set: (value) => {
            state.apiKey = value;
          },
        }),
        onkeydown: (event) => {
          if (event.key === "Enter") saveKey();
        },
      }),
      d.if(
        () => !!state.keyError,
        d.el("div", { class: "mt-3 w-full text-sm text-rose-300" }, d.text(() => state.keyError)),
      ),
      d.el(
        "div",
        {
          class:
            "mt-5 rounded-full bg-white px-6 py-2.5 text-sm font-semibold text-black cursor-pointer disabled:opacity-50",
          disabled: () => state.keyBusy,
          onclick: () => saveKey(),
        },
        d.text(() => (state.keyBusy ? "Checking…" : "Start")),
      ),
    ),
  );
}

function coverScreen() {
  return d.el(
    "div",
    { class: "relative h-screen w-full grid place-items-center overflow-hidden bg-[#070709]" },
    d.el(
      "div",
      { class: "flex flex-col items-center" },
      d.el(
        "div",
        { class: "perspective-[1600px]" },
        d.el(
          "div",
          { class: "relative aspect-video w-[min(52vw,22rem)]" },
          ...covers.map((cover, index) => coverCard(cover, index)),
          d.el("div", {
            class:
              "pointer-events-none absolute left-1/2 top-[108%] h-8 w-3/4 -translate-x-1/2 rounded-full bg-black/70 blur-2xl",
          }),
        ),
      ),
      d.el("div", { class: "mt-28 text-lg font-semibold" }, d.text(() => covers[state.index].title)),
      d.el("div", { class: "mt-1 text-sm text-white/45" }, "Cover flow"),
      d.el(
        "div",
        { class: "mt-4 flex gap-3" },
        d.el(
          "div",
          {
            class: "cursor-pointer rounded-full bg-white/10 px-4 py-2 text-sm",
            onclick: () => step(-1),
          },
          "Prev",
        ),
        d.el(
          "div",
          {
            class: "cursor-pointer rounded-full bg-white px-4 py-2 text-sm font-semibold text-black",
            onclick: () => step(1),
          },
          "Next",
        ),
      ),
    ),
  );
}

doc.body.appendChild(
  d.el(
    "div",
    { class: "h-screen w-full" },
    d.if(() => !state.entered, titleScreen()),
    d.if(() => state.entered, coverScreen()),
  ),
);

const win = new Window(doc, { title: "DJ Grok", width: 960, height: 640 });
win.on("close", () => {
  console.log("window closed");
});
await win.closed;
