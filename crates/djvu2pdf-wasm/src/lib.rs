//! WebAssembly entry points (target `wasm32-unknown-unknown`, no imports required).
//! Conversion runs entirely in the browser; documents never leave the user's machine.
//!
//! Status: scaffolding.

/// Version string, as a smoke test that the module links.
#[unsafe(no_mangle)]
pub extern "C" fn djvu2pdf_version_len() -> usize {
    djvu2pdf::VERSION.len()
}
