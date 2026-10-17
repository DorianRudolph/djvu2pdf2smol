import * as pdfjs from "./pdfjs/pdf.min.mjs";

pdfjs.GlobalWorkerOptions.workerSrc = "./pdfjs/pdf.worker.min.mjs";

const $ = (id) => document.getElementById(id);
const drop = $("drop"), input = $("file"), status = $("status"), bar = $("bar");
const result = $("result"), download = $("download"), preview = $("preview"), error = $("error");

let worker = null, pdfDoc = null, observer = null, url = null;

function formatSize(n) {
  return n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`;
}

function reset() {
  worker?.terminate();
  worker = null;
  observer?.disconnect();
  observer = null;
  pdfDoc?.destroy();
  pdfDoc = null;
  if (url) URL.revokeObjectURL(url);
  url = null;
  preview.replaceChildren();
  result.hidden = error.hidden = true;
}

function fail(message) {
  status.hidden = true;
  error.textContent = `Conversion failed: ${message}`;
  error.hidden = false;
}

async function start(file) {
  reset();
  drop.classList.add("compact");
  $("name").textContent = file.name;
  $("count").textContent = "Reading…";
  bar.removeAttribute("value"); // indeterminate until the page count is known
  status.hidden = false;

  const data = await file.arrayBuffer();
  worker = new Worker("convert-worker.js");
  worker.onmessage = ({ data: msg }) => {
    if (msg.type === "progress") {
      bar.max = msg.total;
      bar.value = msg.done;
      $("count").textContent = `${msg.done} / ${msg.total} pages`;
    } else if (msg.type === "done") {
      worker.terminate();
      worker = null;
      finish(file, msg.pdf, msg.stats);
    } else if (msg.type === "error") {
      fail(msg.message);
    }
  };
  worker.onerror = (e) => fail(e.message || "the converter stopped unexpectedly");
  worker.postMessage({ data }, [data]);
}

function finish(file, pdf, stats) {
  status.hidden = true;
  const name = file.name.replace(/\.(djvu?|djv)$/i, "") + ".pdf";
  url = URL.createObjectURL(new Blob([pdf], { type: "application/pdf" }));
  download.href = url;
  download.download = name;
  $("summary").textContent = `${stats.pages} page${stats.pages === 1 ? "" : "s"} · ${formatSize(pdf.byteLength)}`;
  result.hidden = false;
  showPreview(pdf.slice(0)); // pdf.js takes ownership of its copy
}

async function showPreview(data) {
  const doc = await pdfjs.getDocument({ data, isEvalSupported: false }).promise;
  if (url === null) return doc.destroy(); // a newer file was dropped meanwhile
  pdfDoc = doc;
  const pages = [];
  for (let i = 1; i <= doc.numPages; i++) pages.push(doc.getPage(i));
  const slots = [];
  for (const [i, p] of (await Promise.all(pages)).entries()) {
    const vp = p.getViewport({ scale: 1 });
    const slot = document.createElement("div");
    slot.className = "page";
    slot.style.aspectRatio = `${vp.width} / ${vp.height}`;
    slot.dataset.index = i;
    slot.page = p;
    slots.push(slot);
  }
  if (pdfDoc !== doc) return;
  preview.replaceChildren(...slots);
  observer = new IntersectionObserver(onVisibility, { rootMargin: "150% 0px" });
  slots.forEach((s) => observer.observe(s));
}

function onVisibility(entries) {
  for (const { target: slot, isIntersecting } of entries) {
    if (isIntersecting) render(slot);
    else release(slot);
  }
}

async function render(slot) {
  if (slot.canvas) return;
  const canvas = document.createElement("canvas");
  slot.canvas = canvas;
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const base = slot.page.getViewport({ scale: 1 });
  const viewport = slot.page.getViewport({ scale: (slot.clientWidth * dpr) / base.width });
  canvas.width = Math.floor(viewport.width);
  canvas.height = Math.floor(viewport.height);
  slot.task = slot.page.render({ canvasContext: canvas.getContext("2d"), viewport });
  try {
    await slot.task.promise;
    if (slot.canvas === canvas) slot.replaceChildren(canvas);
  } catch {
    // cancelled: the page scrolled out of range
  }
}

function release(slot) {
  slot.task?.cancel();
  slot.task = null;
  if (slot.canvas) {
    slot.canvas.width = slot.canvas.height = 0; // free the bitmap now
    slot.canvas = null;
  }
  slot.replaceChildren();
}

function accept(files) {
  const file = files?.[0];
  if (file) start(file).catch((e) => fail(e.message || String(e)));
}

$("choose").addEventListener("click", () => input.click());
input.addEventListener("change", () => {
  accept(input.files);
  input.value = "";
});
document.addEventListener("dragover", (e) => {
  e.preventDefault();
  drop.classList.add("over");
});
document.addEventListener("dragleave", (e) => {
  if (!e.relatedTarget) drop.classList.remove("over");
});
document.addEventListener("drop", (e) => {
  e.preventDefault();
  drop.classList.remove("over");
  accept(e.dataTransfer?.files);
});
