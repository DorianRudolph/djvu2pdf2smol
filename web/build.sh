#!/bin/sh
# Build the static site into web/dist: the wasm converter plus the pdf.js viewer from the
# official release archive (pinned version and checksum; no CDN at runtime).
# Serve dist/ from any static host, e.g. `python3 -m http.server -d web/dist`.
set -eu
cd "$(dirname "$0")"

PDFJS_VERSION=4.10.38
PDFJS_SHA256=c85b52b94e081c0443333da0ba4270661b2061da573133984c6272fe3e27bb57
PDFJS_ZIP=.cache/pdfjs-$PDFJS_VERSION-legacy-dist.zip

cargo build --release --target wasm32-unknown-unknown -p djvu2pdf-wasm

if [ ! -f "$PDFJS_ZIP" ]; then
  mkdir -p .cache
  curl -fsSL -o "$PDFJS_ZIP.part" \
    "https://github.com/mozilla/pdf.js/releases/download/v$PDFJS_VERSION/pdfjs-$PDFJS_VERSION-legacy-dist.zip"
  mv "$PDFJS_ZIP.part" "$PDFJS_ZIP"
fi
echo "$PDFJS_SHA256  $PDFJS_ZIP" | shasum -a 256 -c - >/dev/null

rm -rf dist
mkdir -p dist
cp index.html legal.html style.css app.js convert-worker.js dist/
cp ../target/wasm32-unknown-unknown/release/djvu2pdf_wasm.wasm dist/djvu2pdf.wasm
# the viewer app without source maps, debugger, sample document, CJK cmaps and standard
# fonts (our PDFs embed their only font)
unzip -q "$PDFJS_ZIP" -d dist/pdfjs \
  -x '*.map' 'web/debugger.*' 'web/*.pdf' 'web/cmaps/*' 'web/standard_fonts/*'
echo "built web/dist"
