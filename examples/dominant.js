import { Document, MutationObserver, dominant } from "../index.js";

const document = new Document();
const d = dominant(document);

const names = ["Flex", "Grid"];
let title = "Hello";
let open = true;
const root = d.el(
  "div",
  { class: () => (open ? "p-4 bg-zinc-900 text-white" : "p-4 bg-red-500 text-white") },
  d.el("div", { class: "text-xl" }, d.text(() => title)),
  d.if(() => open, d.el("div", { class: "text-sm" }, "open")),
  d.el("div", { class: "flex gap-2" }, d.map(() => names, "|", (name) => d.el("span", {}, name))),
);
document.body.appendChild(root);

await new Promise((resolve) => setTimeout(resolve, 0));

let text = root.textContent;
if (!text.includes("Hello") || !text.includes("open") || !text.includes("Flex|Grid")) {
  throw new Error("first paint " + JSON.stringify(text));
}
if (text.includes("if anchor")) throw new Error("comment leaked into text");
if (!root.className.includes("bg-zinc-900")) throw new Error("class " + root.className);

title = "Updated";
open = false;
names.push("Type");
d.updateSync();
text = root.textContent;
if (text.includes("open") || !text.includes("Updated") || !text.includes("Flex|Grid|Type")) {
  throw new Error("update " + JSON.stringify(text));
}
if (!root.className.includes("bg-red-500")) throw new Error("class stayed " + root.className);

const attrs = [];
const classes = new MutationObserver((records) => {
  for (const record of records) attrs.push(record.attributeName + "=" + record.oldValue);
});
classes.observe(root, { attributes: true, attributeOldValue: true });
root.className = "p-4 bg-blue-500";

const chars = [];
const label = root.childNodes[0].childNodes[0];
const data = new MutationObserver((records) => {
  for (const record of records) chars.push(record.oldValue);
});
data.observe(label, { characterData: true, characterDataOldValue: true });
const previous = label.textContent;
label.textContent = "Typed";

await new Promise((resolve) => setTimeout(resolve, 0));
if (!attrs.some((item) => item.startsWith("class="))) throw new Error("attributes " + attrs.join("|"));
if (!chars.includes(previous)) throw new Error("characterData " + chars.join("|"));

let threw = false;
try {
  d.html`div`;
} catch (error) {
  threw = String(error.message).includes("no HTML");
}
if (!threw) throw new Error("d.html should throw");

console.log("sweet dominant ok");
