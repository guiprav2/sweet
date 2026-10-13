// SWEET_PROF=1 node examples/carousel-stress.js [local-video.webm]
// Deliberately block JavaScript while the native thread animates five covers.
import { performance } from "node:perf_hooks";
import { Document, Window } from "../index.js";

const doc = new Document();
doc.body.className = "relative h-screen w-full overflow-hidden rounded-2xl bg-[#070709]";
const stage = doc.createElement("div");
stage.className = "absolute left-[544px] top-[300px] w-[352px] h-[198px] perspective-[1600px]";
doc.body.appendChild(stage);
const cards = [];
const videos = [];
const videoPath = process.env.SWEET_STRESS_VIDEO || process.argv[2];
const pose = (offset) => {
  const i = Math.min(Math.abs(offset), 5);
  const sign = Math.sign(offset);
  return `absolute inset-0 transition-all duration-3000 ease-out z-[${40 - i * 6}] [transform:translateX(${sign * [0, 228, 292, 348, 396, 436][i]}px)_translateZ(${[90, -20, -64, -104, -140, -172][i]}px)_rotateY(${-sign * 50}deg)]`;
};
for (let i = 0; i < 5; i++) {
  const card = doc.createElement("div");
  card.className = pose(i - 2);
  const image = doc.createElement("img");
  image.className = "h-full w-full object-cover rounded-xl";
  image.src = new URL("../djgrok/covers/WNcsUNKlAKw.jpg", import.meta.url).pathname;
  card.appendChild(image);
  if (i === 2 && videoPath) {
    const video = doc.createElement("video");
    video.className = "absolute inset-0 h-full w-full object-cover";
    video.src = videoPath;
    video.muted = true;
    card.appendChild(video);
    videos.push(video);
    void video.play().catch(console.error);
  }
  stage.appendChild(card);
  cards.push(card);
}
const win = new Window(doc, { title: "Carousel thread stress", width: 1440, height: 900, borderless: true });
setTimeout(() => {
  cards.forEach((card, i) => { card.className = pose(i - 3); });
  setTimeout(() => {
    console.log("Blocking JavaScript for 2 seconds; native animation must continue.");
    const until = performance.now() + 2000;
    while (performance.now() < until) { /* simulate busy application work */ }
    console.log("JavaScript resumed.");
  }, 100);
}, 500);
win.on("close", () => videos.forEach((video) => video.removeAttribute("src")));
setTimeout(() => win.close(), 4500);
await win.closed;
