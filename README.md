# djvu2pdf2smol

Convert DjVu documents to PDF **without the usual 5–15× size blow-up and without touching a
single text pixel** — in pure Rust, as a CLI and in the browser (WebAssembly).

Most converters flatten every DjVu page into one full-resolution raster. DjVu pages are
layered, and PDF has close equivalents for every layer, so this project re-encodes each layer
instead:

| DjVu layer | PDF | lossless? |
|---|---|---|
| JB2 text mask (+ shared Djbz dictionaries) | JBIG2 stencil, **transcoded** shape-by-shape (no re-classification), shared `/JBIG2Globals` | yes, bit-exact |
| BG44 background (subsampled colour) | JPEG 2000 at its native resolution, same byte budget | visually (re-encode) |
| FG44 foreground colour | small JPEG 2000 image, mask as `/Mask` | visually (re-encode) |
| FGbz palette | one stencil per colour | yes |
| TXTz hidden text | invisible text layer | yes (words + boxes) |
| NAVM outline, ANTz links | bookmarks, link annotations | yes |

A Python/C++ prototype of this design, benchmarked against `ddjvu`, dpsprep, OCRmyPDF,
djvups/ps2pdf and djvu-rs's PDF exporter, reached **≈1.0–1.08× the DjVu size** on scanned
books, photo-heavy scans and a born-digital thesis, with all masks bit-exact and pixel-exact
bitonal rendering in MuPDF, pdf.js and Apple Preview. The best existing tools produced
2.7–18× the DjVu size.

## Status

Early: the repository is being set up and the prototype ported.

* [x] workspace, forks as submodules, wasm build of OpenJPEG
* [~] `jbig2-encoder`: MQ / integer / symbol-ID coders done; regions, dictionaries, text
      regions, segment writer to do
* [ ] JB2 structure API in the djvu-rs fork (shapes with parents, blits, dictionaries)
* [ ] converter: masks, backgrounds, foregrounds, text, outline, links, verification
* [ ] wasm front end

## Layout

```
crates/
  jbig2-encoder/   JBIG2 encoder for PDF-embeddable streams (symbol dictionaries,
                   refinement, text regions); input = shapes + placements, no DjVu knowledge
  djvu2pdf/        converter library + `djvu2pdf` CLI
  djvu2pdf-wasm/   WebAssembly entry points
vendor/            git submodules (forks)
  djvu-rs/         https://github.com/DorianRudolph/djvu-rs  — DjVu decoding (MIT)
  openjp2/         https://github.com/dorianrudolph/openjp2 (branch wasm32-support)
                   — Rust port of OpenJPEG (BSD-2-Clause), crate in openjp2-rs/
```

### What the forks change

* **openjp2** (`wasm32-support`): implements the C allocator on Rust's allocator on
  `wasm32-unknown-unknown` (no libc there; other targets unchanged); allocates the default
  codestream comment with `opj_malloc` (it was a `CString` released with C `free` —
  undefined behaviour, aborts on wasm); compiles the `fprintf` module without `file-io`.
  Result: a JPEG 2000 encoder in wasm with zero imports, byte-identical to native.
* **djvu-rs**: planned — expose the JB2 structure (shape bitmaps with parent indices,
  blits, shared dictionaries) that `djvu-jb2` already computes while decoding.

Both are meant to go upstream.

## Building

```sh
git clone --recursive https://github.com/DorianRudolph/djvu2pdf2smol
cargo build --release
cargo test
rustup target add wasm32-unknown-unknown
cargo build --release -p djvu2pdf-wasm --target wasm32-unknown-unknown
```

## Test data

This repository contains **no corpus documents**. Tests use the freely distributable fixtures
inside `vendor/djvu-rs/tests/` or generate their input.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. The submodules keep their own licenses
(djvu-rs: MIT; openjp2: BSD-2-Clause).
