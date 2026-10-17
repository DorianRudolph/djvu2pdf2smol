//! DjVu → layered (MRC) PDF.
//!
//! Per page, each DjVu layer is re-encoded into its PDF counterpart instead of flattening
//! the page into one full-resolution raster:
//!
//! | DjVu | PDF |
//! |---|---|
//! | JB2 mask (Sjbz + shared Djbz) | JBIG2 `/ImageMask` stencil, transcoded bit-exactly (`jbig2-encoder`) |
//! | BG44 background | JPEG 2000 at its native subsampled resolution, same byte budget (`openjp2`) |
//! | FG44 foreground | low-resolution JPEG 2000 image with the mask as `/Mask` |
//! | FGbz palette | one stencil per palette colour |
//! | TXTz hidden text | invisible text (render mode 3) |
//! | NAVM / ANTz | bookmarks / link annotations |
//!
//! Status: scaffolding. The reference implementation is the Python/C++ prototype
//! described in README.md.

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
