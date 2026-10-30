const $ = (id) => document.getElementById(id);
const drop = $("drop"), input = $("file"), status = $("status"), bar = $("bar");
const result = $("result"), download = $("download"), preview = $("preview"), error = $("error");

let worker = null, url = null, generation = 0;

// the pdf.js viewer app inside the iframe, once it has initialized
const viewerReady = new Promise((resolve) => {
  const init = () => {
    const win = preview.contentWindow;
    // files dropped on the viewer are DjVu files for us, not PDFs for pdf.js
    win.addEventListener("dragover", (e) => {
      e.preventDefault();
      e.stopImmediatePropagation();
    }, true);
    win.addEventListener("drop", (e) => {
      e.preventDefault();
      e.stopImmediatePropagation();
      accept(e.dataTransfer?.files);
    }, true);
    // opening PDFs is not what this page is for
    const style = win.document.createElement("style");
    style.textContent = "#secondaryOpenFile { display: none !important; }";
    win.document.head.append(style);
    win.PDFViewerApplication.initializedPromise.then(() => resolve(win.PDFViewerApplication));
  };
  if (preview.contentDocument?.readyState === "complete" && preview.contentWindow.PDFViewerApplication) init();
  else preview.addEventListener("load", init, { once: true });
});

function formatSize(n) {
  return n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`;
}

function reset() {
  generation++;
  worker?.terminate();
  worker = null;
  viewerReady.then((app) => app.close());
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
  $("outname").textContent = name;
  $("summary").textContent = `${stats.pages} page${stats.pages === 1 ? "" : "s"} · ${formatSize(pdf.byteLength)}`;
  result.hidden = false;

  const gen = generation;
  const app = await viewerReady;
  if (gen !== generation) return; // a newer file was chosen meanwhile
  preview.hidden = false;
  // narrow screens: start without the sidebar (the PDF asks for its outline); 0 = none,
  // -1 = as the document requests
  preview.contentWindow.PDFViewerApplicationOptions.set("sidebarViewOnLoad", preview.clientWidth < 700 ? 0 : -1);
  // originalUrl gives the viewer's own download button the right file name
  await app.open({ url, originalUrl: name });
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
