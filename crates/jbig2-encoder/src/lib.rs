//! JBIG2 encoder producing PDF-embeddable streams (ITU-T T.88, PDF 32000-1 §7.4.7).
//!
//! Input is *structure*, not pixels: a set of symbol bitmaps (optionally refined from a
//! parent symbol) and their placements. Encoding that structure losslessly — no symbol
//! classification — is what lets a DjVu JB2 layer become JBIG2 bit-exactly.
//!
//! Status: being ported from the C++ prototype (`jb2jbig2`). Done: the MQ arithmetic
//! coder and the integer / symbol-ID coders. To do: generic and refinement region coding,
//! symbol dictionaries, text regions, segment writer, and the decoder-compatibility rules
//! (no end-of-page segment, <= 4 referred segments, no power-of-two symbol counts).

pub mod mq;
