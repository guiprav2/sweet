import { Document, Window } from "../index.js";

const doc = new Document();
doc.body.className = "flex flex-row bg-white";
const left = doc.createElement("div");
const right = doc.createElement("div");
left.className = "w-1/2 h-10 bg-red-500";
right.className = "w-1/2 h-10 bg-blue-500";
doc.body.appendChild(left);
doc.body.appendChild(right);

const boxes = doc.layoutBoxes(200, 80);
const leftBox = boxes.find((box) => box.id === left.nodeId);
const rightBox = boxes.find((box) => box.id === right.nodeId);
if (!leftBox || Math.abs(leftBox.width - 100) > 1.5) {
  throw new Error(`left width ${leftBox && leftBox.width}`);
}
if (!rightBox || Math.abs(rightBox.x - 100) > 1.5) {
  throw new Error(`right x ${rightBox && rightBox.x}`);
}

const png = doc.renderPng(200, 80);
if (png[0] !== 0x89 || png[1] !== 0x50 || png[2] !== 0x4e || png[3] !== 0x47) {
  throw new Error("renderPng did not return a PNG");
}

const styled = doc.createElement("div");
try {
  styled.setAttribute("style", "color: red");
  throw new Error("style attribute was accepted");
} catch (error) {
  if (!String(error).includes("no CSS")) throw error;
}

const card = doc.createElement("div");
card.className = "m-8 p-6 bg-zinc-900 text-white text-2xl rounded-lg";
card.textContent = "Hello";
doc.body.appendChild(card);

const win = new Window(doc, { title: "sweet check", width: 640, height: 400 });
let sawClose = false;
win.on("close", () => {
  sawClose = true;
});
setTimeout(() => win.close(), 400);
await win.closed;
if (!sawClose) throw new Error("close event did not fire");
console.log("sweet check ok");
