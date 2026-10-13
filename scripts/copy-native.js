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
const profile = process.argv[2] ?? "release";
if (!["debug", "release"].includes(profile)) throw new Error(`Unknown build profile: ${profile}`);
const source = join("target", profile, `${prefix}sweet.${extension}`);
const directory = profile === "debug" ? join("native", "debug") : "native";
const destination = join(directory, "sweet.node");
await mkdir(directory, { recursive: true });
await copyFile(source, destination);
console.log(`Copied ${source} to ${destination}`);
