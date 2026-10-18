#!/bin/sh
# Build the static site into web/dist: the wasm converter plus a vendored copy of pdf.js
# (no CDN). Serve dist/ from any static host, e.g. `python3 -m http.server -d web/dist`.
set -eu
cd "$(dirname "$0")"
cargo build --release --target wasm32-unknown-unknown -p djvu2pdf-wasm
[ -d node_modules/pdfjs-dist ] || npm ci --no-audit --no-fund
rm -rf dist
mkdir -p dist/pdfjs
cp index.html legal.html style.css app.js convert-worker.js dist/
cp ../target/wasm32-unknown-unknown/release/djvu2pdf_wasm.wasm dist/djvu2pdf.wasm
pdfjs=node_modules/pdfjs-dist
cp "$pdfjs/legacy/build/pdf.min.mjs" "$pdfjs/legacy/build/pdf.worker.min.mjs" "$pdfjs/LICENSE" dist/pdfjs/
cp -R "$pdfjs/legacy/web/pdf_viewer.mjs" "$pdfjs/legacy/web/pdf_viewer.css" "$pdfjs/legacy/web/images" dist/pdfjs/
echo "built web/dist"
