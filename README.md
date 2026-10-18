# djvu2pdf2smol

Convert DjVu documents to PDF files about the same size as the DjVu, with the text pixels
unchanged. Written in pure Rust: a command-line tool and a web page that converts in the
browser.

Most converters flatten every DjVu page into one full-resolution image, so the PDF ends up
about 3–18× larger than the DjVu. This one re-encodes each DjVu layer into its PDF counterpart:

* **Text and line art** (the JB2 mask) become JBIG2, transcoded shape by shape: bit-exact,
  no re-compression artefacts.
* **Colour backgrounds and foregrounds** (IW44) become JPEG 2000 at their native
  resolution, with the same byte budget as in the DjVu.
* **Hidden text, bookmarks and links** are carried over, so the PDF stays searchable and
  navigable.

## Results

Total PDF size against total DjVu size, default settings:

| Test set | Files | DjVu → PDF | Size |
|---|---|---|---|
| Scanned math books (126–980 pages) | 9 | 51.7 → 53.8 MB | **+3.9 %** |
| djvu-rs test corpus (scans, colour books, maps) | 32 | 40.8 → 41.8 MB | **+2.5 %** |
| Born-digital thesis (LaTeX) | 1 | 0.56 → 0.60 MB | **+6.3 %** |

The best other converter tested (dpsprep `-O3`) produced 2.7–5.4× the DjVu size. Details
are in [docs/benchmark.md](docs/benchmark.md).

## Usage

Install the CLI:

```sh
cargo install --git https://github.com/DorianRudolph/djvu2pdf2smol djvu2pdf
```

```sh
djvu2pdf book.djvu -o book.pdf
djvu2pdf books/ -o pdfs/        # every .djvu file in a directory
djvu2pdf --help
```

By default, colour backgrounds get 1× and foregrounds 2× the bytes the DjVu spent on them
(`--bg rate:F|lossless`, `--fg ...`), and all CPU cores are used (`-j N`).

Pages whose layers use G4/MMR (`Smmr`) or JPEG (`BGjp`/`FGjp`) encodings are rejected with
an error. These encodings are rare; none of the test files used them.

## Web version

`web/` is a static page: choose or drop a DjVu file, it converts in a background worker,
and the result opens in the pdf.js viewer with a download button. Nothing is uploaded, and
all files are served from the site itself.

```sh
rustup target add wasm32-unknown-unknown
web/build.sh                          # builds web/dist (downloads the pinned pdf.js release)
python3 -m http.server -d web/dist    # or any static host
web/deploy-pages.sh                   # publish to the `pages` branch (GitHub Pages)
```

## Building from source

```sh
git clone --recursive https://github.com/DorianRudolph/djvu2pdf2smol
cd djvu2pdf2smol
cargo build --release                 # target/release/djvu2pdf
```

How it works: [docs/design.md](docs/design.md). Changes in the forked dependencies:
[docs/forks.md](docs/forks.md). Development notes: [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. The submodules keep their own licenses
(djvu-rs: MIT; openjp2: BSD-2-Clause).
