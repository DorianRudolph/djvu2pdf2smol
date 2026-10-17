# Forked and patched dependencies

The submodules in `vendor/` are forks with changes this project needs. All changes are
meant to go upstream.

## djvu-rs

<https://github.com/DorianRudolph/djvu-rs>, branch `jb2-structure-api`
(upstream: <https://github.com/matyushkin/djvu-rs>, MIT)

* `djvu_jb2::decode_structure`: the shapes (cropped bitmaps with refinement parents) and
  blits of a JB2 image, instead of its rendered bitmap; `Jb2Dict::shape` does the same for
  dictionaries. Compositing the blits reproduces `decode()` exactly.
* `DjVuPage::shared_jb2_dict`: the raw bytes and decoded dictionary of the `Djbz` a page
  INCLs, so pages can be grouped by dictionary.
* `DjVuPage::foreground_palette`: the FGbz colours and per-blit colour indices.
* `Iw44Image::is_color` is public (it was test-only).

## openjp2

<https://github.com/dorianrudolph/openjp2>, branch `wasm32-support`
(Rust port of OpenJPEG, BSD-2-Clause)

* On `wasm32-unknown-unknown`, which has no libc, the C allocator functions are
  implemented on Rust's allocator. Other targets are unchanged.
* The default codestream comment is allocated with `opj_malloc`. It was a `CString`
  released with C `free`: undefined behaviour, which aborts on wasm.
* The `fprintf` module is compiled without the `file-io` feature, and builds without
  `file-io` have no warnings.

Result: a JPEG 2000 encoder in wasm with no imports, byte-identical to the native build.

## hayro-jbig2 (test only)

`vendor/hayro-jbig2` is a patched copy of hayro-jbig2 0.3.1 (MIT OR Apache-2.0), used only
as a dev-dependency to verify the encoder. It fixes refinement decoding with a positive
reference x offset (a refinement wider than its reference), which made the decoder lose
sync. Details and the diff are in `vendor/hayro-jbig2/PATCHED.md`. Remove it once the fix
is released upstream.
