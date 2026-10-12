import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Document, Window, dominant } from "../index.js";

const photoPath = join(dirname(fileURLToPath(import.meta.url)), "ANh8R.jpg");

const doc = new Document();
const d = dominant(doc);
doc.body.className = "flex flex-row h-screen bg-zinc-100 text-zinc-900";

const win = new Window(doc, { title: "Sweet", width: 960, height: 640 });
win.on("close", () => {
  console.log("window closed");
});

const side = doc.createElement("aside");
side.className = "w-48 h-screen bg-zinc-900 text-zinc-100 p-6 flex flex-col gap-4";
const brand = doc.createElement("div");
brand.className = "text-lg font-semibold";
brand.textContent = "Sweet";
const note = doc.createElement("div");
note.className = "text-sm text-zinc-400";
note.textContent = "Node owns the DOM. This window only paints it.";
side.appendChild(brand);
side.appendChild(note);

const main = doc.createElement("main");
main.className = "flex-1 p-8 flex flex-col gap-6";
let headline = "Tailwind, without CSS";
const title = d.el("div", { class: "text-3xl font-semibold" }, d.text(() => headline));
const grid = doc.createElement("div");
grid.className = "grid grid-cols-3 gap-4";
for (const name of ["Flex", "Grid", "Type"]) {
  const card = doc.createElement("div");
  card.className = "bg-white rounded-lg p-4 border border-zinc-200";
  const heading = doc.createElement("div");
  heading.className = "font-medium";
  heading.textContent = name;
  const body = doc.createElement("div");
  body.className = "text-sm text-zinc-500 mt-2";
  body.textContent = "A utility class string is the only style channel.";
  card.appendChild(heading);
  card.appendChild(body);
  grid.appendChild(card);
}
const photo = doc.createElement("img");
photo.className = "w-full h-64 object-cover rounded-lg";
photo.setAttribute("src", photoPath);
grid.appendChild(photo);
const button = doc.createElement("div");
button.className = "bg-zinc-900 text-white rounded-md px-4 py-2 cursor-pointer self-start";
button.textContent = "Say hello";
button.addEventListener("click", () => {
  console.log("click");
  button.textContent = "Hello from Node";
  button.className = "bg-blue-500 text-white rounded-md px-4 py-2 cursor-pointer self-start";
  headline = "Tailwind, without CSS (with Dominant!)";
  d.updateSync();
});

main.appendChild(title);
main.appendChild(grid);
main.appendChild(button);
doc.body.appendChild(side);
doc.body.appendChild(main);

await win.closed;
