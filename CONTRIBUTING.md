# Development

```sh
git clone --recursive https://github.com/DorianRudolph/djvu2pdf2smol
cargo test --release
cargo clippy --all-targets -- -D warnings
cargo build --release --target wasm32-unknown-unknown -p djvu2pdf-wasm
```

* **Test data:** do not commit DjVu or PDF documents unless they are freely
  redistributable. Tests use the fixtures in `vendor/djvu-rs/tests/fixtures` or generate
  their input.
* **Determinism:** output must not depend on the number of threads.
  `output_does_not_depend_on_threads` checks this, and the web build relies on it.
* **Masks:** after changing the encoder or the mask code, check that masks still decode
  bit-exactly:
  `cargo run --release --example mask_streams -- --verify FILE.djvu OUTDIR`.
* **Licenses page:** `web/build.sh` generates `licenses.html` with cargo-about from the
  crates compiled into the wasm module (`web/about.toml`, `web/about.hbs`). When a new
  dependency's notice comes out as a bare template (a license file the detector does not
  recognize, or none), add a `clarify` entry pointing at the crate's real license file.
* **Design notes:** [docs/design.md](docs/design.md).
