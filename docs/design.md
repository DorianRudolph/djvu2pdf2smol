# Design

## Layer mapping

A DjVu page is a stack of layers. Each one is re-encoded into its closest PDF counterpart
instead of rendering the page into a single image.

| DjVu | PDF | Lossless? |
|---|---|---|
| JB2 mask (`Sjbz`, shared `Djbz` dictionaries) | JBIG2 `/ImageMask` stencil; one `/JBIG2Globals` stream per DjVu dictionary, or one merged stream | yes, bit-exact |
| BG44 background (IW44, usually 1/3 resolution) | JPEG 2000 image at the layer's own resolution, scaled over the page | no (re-encoded at the same byte budget) |
| FG44 foreground colour | small JPEG 2000 image with the JBIG2 mask as its `/Mask` | no (as above) |
| FGbz palette | one stencil per palette colour | yes |
| TXTz hidden text | invisible text (render mode 3) in a glyphless font with ToUnicode maps | yes (words and their boxes) |
| NAVM outline | PDF outline | yes |
| ANTz map areas with URLs | `/Link` annotations (pages or URIs) | yes |
| INFO rotation, METz metadata | `/Rotate`, document info | yes |

Page geometry follows djvulibre: layers are anchored at the bottom-left corner, and a layer
of `w × h` samples covers `w·red × h·red` page pixels, with `red` computed as in
`DjVuImage::compute_red`. When an FG44 grid extends past the page, the mask is padded to
the same grid so that `/Mask` and image line up.

## JB2 → JBIG2 without re-classification

`djvu-jb2::decode_structure` (djvu-rs fork) returns what the JB2 decoder computes anyway:
the shapes (cropped bitmaps, each optionally a refinement of an earlier shape) and the
blits. `jbig2-encoder` writes exactly that structure:

* shapes in the shared dictionary become symbols in `/JBIG2Globals`;
* page-local shapes become symbols in page dictionaries, or are coded inline as text-region
  refinements when they are used once and nothing refines from them;
* page-local shapes identical to a global one reuse that symbol;
* blits become text-region instances.

Because nothing is matched or approximated, the result decodes to exactly the DjVu mask.

When a document has several shape dictionaries whose union (exact duplicates removed) has
at most 80 % of their total shapes, they are merged into one `/JBIG2Globals`. This happens
with born-digital files that repeat a font's glyphs in every dictionary.

### Decoder compatibility

Rules found by testing jbig2dec, pdf.js, Apple CoreGraphics, poppler and MuPDF:

* no end-of-page or end-of-file segments (PDF 32000-1 §7.4.7 forbids them in embedded
  streams; CoreGraphics rejects them);
* every segment refers to at most 4 others: the long form of the referred-segment field
  has a retention-flag size that decoders read differently;
* the number of symbols in scope is never a power of two: CoreGraphics sizes symbol IDs
  as ⌊log₂ n⌋ + 1 bits instead of ⌈log₂ n⌉, so one unused 1×1 symbol is added when
  needed;
* refinement uses template 0 with the default adaptive pixels, and generic regions use
  template 0 with the T.88 default adaptive pixels.

## Colour layers

IW44 layers are decoded by djvu-rs and encoded with the openjp2 fork (a Rust port of
OpenJPEG): irreversible 9/7 wavelet, one quality layer, rate control aiming at
`factor × bytes of the DjVu layer`. Grey layers are coded with one component. A layer whose
samples all lie within ±2 levels of its mean becomes a solid fill (or nothing, if white).

## Text layer

DjVu text zones are flattened into lines of words. Each line sets the font size to the line
height; each word is placed with a relative `Td` and scaled horizontally (`Tz`) to its box,
so selection and search highlight the right area. Characters are assigned single-byte
codes in a few simple TrueType fonts (printable ASCII keeps its own code) that share one
embedded 800-byte glyphless font and have their own ToUnicode maps. Control characters
in the DjVu text (OCR debris) are dropped.

## Pipeline

`convert_with_progress` in `crates/djvu2pdf/src/convert.rs`:

1. **Shared preparation** (sequential): parse the document, find the distinct shape
   dictionaries and encode the `/JBIG2Globals`, read all text layers and build the fonts,
   reserve object numbers for the catalog, page tree and pages.
2. **Pages:** with `jobs > 1`, worker threads take page numbers from an atomic counter
   and convert each page into a `PageOut` (byte buffers and lists, nothing written yet).
   With `jobs == 1` (also the wasm path) pages are converted inline.
3. **In-order writer:** finished pages are written in page order; image and content
   streams go straight to the output, and small dictionaries are packed into compressed
   object streams. The file ends with a cross-reference stream.

Object numbers are assigned by the writer, so the output is byte-identical for any number
of threads, and the web build produces the same bytes as the CLI.

## Repository layout

```
crates/
  jbig2-encoder/   JBIG2 encoder for PDF-embeddable streams; input = shapes + placements
  djvu2pdf/        converter library and CLI
    src/mask.rs      JB2 structure -> JBIG2, dictionary merging
    src/jpx.rs       JPEG 2000 (JP2) encoding in memory
    src/text.rs      invisible text layer and fonts
    src/pdffile.rs   streaming writer: object streams, xref stream
    src/convert.rs   pages, document, parallel pipeline
  djvu2pdf-wasm/   WebAssembly entry points
web/               static site (worker, pdf.js viewer)
vendor/            submodules (djvu-rs, openjp2) and a patched test-only hayro-jbig2
docs/              this file, benchmark, forks
```

## WebAssembly interface

`djvu2pdf-wasm` uses a plain C ABI rather than a bindings generator. One conversion per
instance:

1. `ptr = input_alloc(len)`, then copy the DjVu bytes to `memory[ptr..ptr + len]`;
2. `status = convert(ptr, len)`: 0 = ok, 1 = error. It calls the imported
   `djvu2pdf.progress(done, total)` after each page;
3. the PDF, or the error message if `status` is 1, is at `result_ptr()`/`result_len()`;
   a JSON summary is at `stats_ptr()`/`stats_len()`. After a panic (a trap) the message
   is at `panic_ptr()`/`panic_len()`.

`web/convert-worker.js` runs this in a Web Worker, one worker per conversion.
