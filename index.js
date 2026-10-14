import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { bind, MutationObserver, rustDocument } from "./dom.js";

const require = createRequire(import.meta.url);
const root = dirname(fileURLToPath(import.meta.url));
const nativeFile = process.env.SWEET_NATIVE_FILE || (process.env.SWEET_NATIVE_BUILD === "debug"
  ? join(root, "native", "debug", "sweet.node")
  : join(root, "native", "sweet.node"));
if (nativeFile.endsWith(".so")) {
  require.extensions[".so"] = require.extensions[".node"];
}
const native = require(nativeFile);

const openWindows = new Set();
const bound = bind(native);

export const Document = bound.Document;
export const Node = bound.Node;
export { MutationObserver };
export { dominant } from "./dominant.js";

export class Window {
  constructor(document, options) {
    const rust = document?.[rustDocument];
    if (!rust) throw new TypeError("Window expects a sweet Document");
    this.native = new native.Window(rust, options);
    const pending = this.native.closed().finally(() => {
      openWindows.delete(pending);
    });
    openWindows.add(pending);
    this.closed = pending;
  }

  on(event, callback) {
    this.native.on(event, callback);
  }

  close() {
    this.native.close();
  }

  minimize() {
    this.native.minimize();
  }

  toggleMaximize() {
    this.native.toggleMaximize();
  }

  get resizable() {
    return this.native.resizable;
  }

  set resizable(value) {
    this.native.resizable = Boolean(value);
  }
}
