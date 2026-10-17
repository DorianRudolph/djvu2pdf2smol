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

The CLI works. On the test corpora (9 scanned books, the djvu-rs fixtures, a born-digital
thesis) every page converts and every JBIG2 mask decodes bit-exactly. Text, bookmarks and
links match the Python/C++ prototype, and the books' sizes are within ±0.4 % of it. All 4,276 pages of
the 9 books convert in about 13 s on 10 cores.

* [x] workspace, forks as submodules, wasm build of OpenJPEG
* [x] `jbig2-encoder`: generic + refinement dictionaries, text regions, inline refinement,
      global aliasing, decoder-compatibility rules
* [x] JB2 structure API in the djvu-rs fork
* [x] converter + CLI: masks, backgrounds, foregrounds, palettes, text, outline, links,
      page-parallel with in-order streaming output
* [ ] indirect (multi-file) documents
* [x] web front end (`web/`): single-threaded wasm in a Web Worker, pdf.js preview,
      everything served from the same origin (no CDN)

Not supported: G4/MMR masks (`Smmr`) and JPEG-coded layers (`BGjp`/`FGjp`). These rare
encodings did not appear in any test corpus or in a sample of Wikimedia Commons DjVu files;
such pages are rejected with an error rather than converted with the layer missing.

## Usage

```sh
djvu2pdf book.djvu -o book.pdf
djvu2pdf books/ -o pdfs/              # every *.djvu in a directory
djvu2pdf --help
```

Defaults: bit-exact masks, background JPEG 2000 at 1× and foreground at 2× the bytes DjVu
spent on that layer (`--bg rate:F|lossless`, `--fg ...`), all cores (`-j N`).

## Layout

```
crates/
  jbig2-encoder/   JBIG2 encoder for PDF-embeddable streams (symbol dictionaries,
                   refinement, text regions); input = shapes + placements, no DjVu knowledge
  djvu2pdf/        converter library + `djvu2pdf` CLI
                   (mask.rs: JB2 -> JBIG2, jpx.rs: JPEG 2000, text.rs: text layer,
                   pdffile.rs: object/xref-stream writer, convert.rs: pages + document)
  djvu2pdf-wasm/   WebAssembly entry points (plain C ABI, no bindings generator)
web/               static site: drop/choose a file, convert in a worker, preview, download
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
* **openjp2** also builds without warnings when `file-io` is off.
* **djvu-rs** (`jb2-structure-api`): `djvu_jb2::decode_structure` gives the shapes (cropped
  bitmaps with refinement parents) and blits of a JB2 image, and `Jb2Dict::shape` does the
  same for dictionaries. It also adds `DjVuPage::shared_jb2_dict` (the INCL'd Djbz) and
  `DjVuPage::foreground_palette` (FGbz), and makes `Iw44Image::is_color` public.
* **vendor/hayro-jbig2** (not a fork; a dev-dependency only): a patched copy of
  hayro-jbig2 0.3.1. It fixes refinement decoding with a positive reference x offset and
  is used to verify the encoder in tests. See `vendor/hayro-jbig2/PATCHED.md`.

Both are meant to go upstream.

## Building

```sh
git clone --recursive https://github.com/DorianRudolph/djvu2pdf2smol
cargo build --release
cargo test
rustup target add wasm32-unknown-unknown
web/build.sh                          # wasm + vendored pdf.js -> web/dist (needs npm)
python3 -m http.server -d web/dist    # any static host works
```

The web build produces the same PDF bytes as the CLI with `-j 1`.

## Test data

This repository contains **no corpus documents**. Tests use the freely distributable fixtures
inside `vendor/djvu-rs/tests/` or generate their input.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. The submodules keep their own licenses
(djvu-rs: MIT; openjp2: BSD-2-Clause).
