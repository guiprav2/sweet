import { Document, MutationObserver } from "../index.js";

const document = new Document();
const seen = [];
const observer = new MutationObserver((records) => {
  for (const record of records) seen.push(record.addedNodes.map((node) => node.tagName).join(","));
});
observer.observe(document.body, { childList: true, subtree: true });

const img = document.createElement("img");
if (img.tagName !== "IMG") throw new Error("tag " + img.tagName);
img.className = "w-8 h-4 object-contain";
document.body.appendChild(img);

const png = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR4nGP4z8AAQv8ZYAwAQ84H+VjtZqAAAAAASUVORK5CYII=",
  "base64",
);
img.src = "data:image/png;base64," + png.toString("base64");
await img.decode();
if (img.naturalWidth !== 2 || img.naturalHeight !== 2) {
  throw new Error("size " + img.naturalWidth + "x" + img.naturalHeight);
}

await new Promise((resolve) => setTimeout(resolve, 0));
if (!seen.includes("IMG")) throw new Error("observer missed the image: " + seen.join("|"));

const frame = document.renderPng(8, 4);
if (frame[0] !== 0x89 || frame[1] !== 0x50) throw new Error("render is not a png");
console.log("sweet image ok");
