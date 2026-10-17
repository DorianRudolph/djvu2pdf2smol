import * as pdfjs from "./pdfjs/pdf.min.mjs";

pdfjs.GlobalWorkerOptions.workerSrc = "./pdfjs/pdf.worker.min.mjs";
globalThis.pdfjsLib = pdfjs; // pdf_viewer.mjs reads the core library from here
const { EventBus, PDFLinkService, PDFViewer } = await import("./pdfjs/pdf_viewer.mjs");

const $ = (id) => document.getElementById(id);
const drop = $("drop"), input = $("file"), status = $("status"), bar = $("bar");
const result = $("result"), download = $("download"), preview = $("preview"), error = $("error");

// one viewer for the page's lifetime; documents are swapped in and out
const eventBus = new EventBus();
const linkService = new PDFLinkService({ eventBus });
const viewer = new PDFViewer({ container: $("viewerContainer"), viewer: $("viewer"), eventBus, linkService });
linkService.setViewer(viewer);
eventBus.on("pagesinit", () => (viewer.currentScaleValue = "page-width"));
new ResizeObserver(() => {
  if (viewer.pdfDocument) viewer.currentScaleValue = "page-width";
}).observe($("viewerContainer"));

let worker = null, pdfDoc = null, url = null, generation = 0;

function formatSize(n) {
  return n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`;
}

function reset() {
  generation++;
  worker?.terminate();
  worker = null;
  viewer.setDocument(null);
  linkService.setDocument(null);
  pdfDoc?.destroy();
  pdfDoc = null;
  if (url) URL.revokeObjectURL(url);
  url = null;
  preview.hidden = result.hidden = error.hidden = true;
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

async function finish(file, pdf, stats) {
  status.hidden = true;
  const name = file.name.replace(/\.(djvu?|djv)$/i, "") + ".pdf";
  url = URL.createObjectURL(new Blob([pdf], { type: "application/pdf" }));
  download.href = url;
  download.download = name;
  $("summary").textContent = `${stats.pages} page${stats.pages === 1 ? "" : "s"} · ${formatSize(pdf.byteLength)}`;
  result.hidden = false;

  const gen = generation;
  const doc = await pdfjs.getDocument({ data: pdf.slice(0), isEvalSupported: false }).promise;
  if (gen !== generation) return doc.destroy(); // a newer file was chosen meanwhile
  pdfDoc = doc;
  preview.hidden = false;
  viewer.setDocument(doc);
  linkService.setDocument(doc);
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
