# hayro-jbig2 0.3.1, patched (dev-dependency only)

Unmodified copy of the crates.io package `hayro-jbig2` 0.3.1
(https://github.com/LaurenzV/hayro, MIT OR Apache-2.0) except for one fix in
`src/decode/generic_refinement.rs` (see `PATCH.diff`):

`RefinementContextGatherer::maybe_reload_buffers` zeroed the reference-row words while
`x - GRREFERENCEDX < 0`, and did not reload them once `x - GRREFERENCEDX` reached 0
(`ref_cur_x` was already 0). With a positive reference x offset — a refinement wider
than its reference, e.g. a 23 px symbol refined from a 21 px one — the decoder used
white reference pixels for the first word of every row and lost sync. The fix keeps the
words that `start_row` loaded at reference column 0.

Used only to verify our encoder in tests (`[patch.crates-io]` in the workspace
`Cargo.toml`). Drop this directory once the fix is released upstream.
