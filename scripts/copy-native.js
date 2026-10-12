import { copyFile, mkdir } from "node:fs/promises";
import { join } from "node:path";

const extensions = {
  darwin: "dylib",
  linux: "so",
  win32: "dll",
};
const extension = extensions[process.platform];
if (!extension) throw new Error(`Unsupported native build platform: ${process.platform}`);

const prefix = process.platform === "win32" ? "" : "lib";
const source = join("target", "release", `${prefix}sweet.${extension}`);
const destination = join("native", "sweet.node");
await mkdir("native", { recursive: true });
await copyFile(source, destination);
console.log(`Copied ${source} to ${destination}`);
