import { copyFile, mkdir, rename } from "node:fs/promises";
import { join } from "node:path";

const extensions = {
  darwin: "dylib",
  linux: "so",
  android: "so",
  win32: "dll",
};
const extension = extensions[process.platform];
if (!extension) throw new Error(`Unsupported native build platform: ${process.platform}`);

const prefix = process.platform === "win32" ? "" : "lib";
const profile = process.argv[2] ?? "release";
if (!["debug", "release"].includes(profile)) throw new Error(`Unknown build profile: ${profile}`);
const targetDirectory = process.env.CARGO_TARGET_DIR || "target";
const source = join(targetDirectory, profile, `${prefix}sweet.${extension}`);
const directory = profile === "debug" ? join("native", "debug") : "native";
const destination = join(directory, "sweet.node");
const temporary = join(directory, `.sweet.node.${process.pid}.tmp`);
await mkdir(directory, { recursive: true });
await copyFile(source, temporary);
await rename(temporary, destination);
console.log(`Copied ${source} to ${destination}`);
