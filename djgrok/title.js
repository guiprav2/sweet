import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Document, Window, dominant } from "../index.js";

const here = dirname(fileURLToPath(import.meta.url));
const doc = new Document();
const d = dominant(doc);
doc.body.className = "h-screen w-full bg-[#070709] text-white";

const state = {
  apiKey: "",
  keyError: "",
  keyBusy: false,
};

function saveKey() {
  if (state.keyBusy) return;
  const key = state.apiKey.trim();
  if (key.length < 20) {
    state.keyError = "Enter a Grok API key.";
    d.updateSync();
    return;
  }
  state.keyError = "";
  state.keyBusy = true;
  d.updateSync();
  console.log("title screen accepted a key of length", key.length);
}

const screen = d.el(
  "div",
  { class: "h-screen w-full grid place-items-center bg-[#070709] px-6" },
  d.el("div", { class: "flex w-full max-w-sm flex-col items-center" },
    d.el("img", {
      src: join(here, "logo.png"),
      class: "h-44 w-44",
    }),
    d.el("div", { class: "mt-7 text-3xl font-semibold" }, "DJ Grok"),
    d.el(
      "div",
      { class: "mt-2 text-center text-sm text-white/55" },
      "A Grok API key opens the booth. It stays in this player until you forget it.",
    ),
    d.el(
      "div",
      { class: "mt-7 w-full text-left text-[11px] font-medium text-white/40" },
      "GROK API KEY",
    ),
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

doc.body.appendChild(screen);

const win = new Window(doc, { title: "DJ Grok", width: 960, height: 640 });
win.on("close", () => {
  console.log("window closed");
});
await win.closed;
