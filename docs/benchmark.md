# Benchmark

Measured on 2026-10-17 with `djvu2pdf` 0.1.0 at default settings (`--bg rate:1 --fg rate:2`,
automatic dictionary merging), on a 10-core Apple Silicon Mac.

## Test sets

* **Scanned math books:** 9 books with 126–980 pages each, 4,276 pages in total; mostly
  bitonal scans with OCR text, some with colour covers. Not redistributable, so not in
  this repository.
* **djvu-rs corpus:** the 32 DjVu files in `tests/fixtures` and `tests/corpus` of
  [djvu-rs](https://github.com/matyushkin/djvu-rs): scans, colour books, maps, born-digital
  documents, rotated pages, palettes, legacy single-image files.
* **Thesis:** a born-digital LaTeX dissertation, 151 pages.

## Size

| Test set | Files | DjVu | PDF | Total | Median per file | Range |
|---|---|---|---|---|---|---|
| Scanned math books | 9 | 51.7 MB | 53.8 MB | +3.9 % | +7.4 % | −1.8 % to +11.1 % |
| djvu-rs corpus | 32 | 40.8 MB | 41.8 MB | +2.5 % | +7.6 % | see below |
| Thesis | 1 | 0.56 MB | 0.60 MB | +6.3 % | | |

Per book:

| Book | DjVu | PDF / DjVu |
|---|---|---|
| Berezanskii, *Expansions in Eigenfunctions of Selfadjoint Operators* | 22.92 MB | 0.982 |
| Mumford, *Algebraic Geometry I* | 2.40 MB | 1.031 |
| Macaulay, *The Algebraic Theory of Modular Systems* | 1.78 MB | 1.032 |
| Folland, *Harmonic Analysis in Phase Space* | 2.01 MB | 1.063 |
| Simon, *Trace Ideals and Their Applications* | 1.26 MB | 1.074 |
| Conn, Gould, Toint, *Trust-Region Methods* | 8.71 MB | 1.087 |
| Wilkinson, *The Algebraic Eigenvalue Problem* | 5.24 MB | 1.101 |
| Hodge, Pedoe, *Methods of Algebraic Geometry II* | 3.90 MB | 1.104 |
| Sommese, Wampler, *Numerical Solution of Systems of Polynomials* | 3.53 MB | 1.111 |

In the djvu-rs corpus, files over 100 KB range from 0.72× (czech) to 1.35× (navm_fgbz).
Files of a few hundred bytes to a few KB grow up to 5× in relative terms, because a PDF
has a few hundred bytes of fixed structure.

## Fidelity

* **Masks:** every JBIG2 mask of all 42 files was decoded with hayro-jbig2 and compared
  with djvu-rs's JB2 rendering: 0 differing pages
  (`cargo run --release --example mask_streams -- --verify FILE OUT`).
* **Rendering** (sampled pages, MuPDF against `ddjvu`): bitonal, rotated and palette pages
  are pixel-identical. Colour pages reach 31–61 dB PSNR, equal to or better than the
  Python/C++ prototype of the same design at the same size.
* **Text, bookmarks, links:** identical to the prototype's output, except that control
  characters in OCR text are now dropped.
* **Validity:** every output passes `qpdf --check`.

## Speed

All 9 books (4,276 pages) convert in 12 s with 10 threads. The web version (one thread,
WebAssembly in Chrome) converts the 162-page Simon book in 1.4 s and the 821-page
Berezanskii book in 39 s.

## Other converters

From an earlier benchmark on extracts of the same three test sets (size estimated for the
full sets; textbooks / djvu-rs corpus / thesis):

| Converter | Size vs DjVu |
|---|---|
| dpsprep `-O3` | 3.2× / 2.7× / 5.4× |
| djvu-rs PDF export, MRC mode | 4.7× / 6.0× / 9.2× |
| `ddjvu -format=pdf` | 4.9× / 18× / 8.5× |
| djvu-rs PDF export, default | 7.8× / 3.4× / 16× |

In that benchmark `ddjvu` and the djvu-rs exporters rendered all sampled bitonal pages
pixel-exact; dpsprep changed 3 of 38.
