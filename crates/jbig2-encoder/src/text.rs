//! Immediate text region segments (T.88 6.4 / 7.4.3), arithmetic coding only.

use crate::bitmap::Bitmap;
use crate::mq::{IaidCtx, IntCtx, Mq, code_len};
use crate::region::{REFINE_AT, REFINE_CONTEXTS, encode_refinement};
use crate::segment::put32;

/// One symbol instance.
pub(crate) struct Instance<'a> {
    pub id: u32,
    /// Left column (S) and bottom row (top-down) of the placed bitmap.
    pub x: i32,
    pub bottom: i32,
    pub width: i32,
    pub height: i32,
    /// Inline refinement (RI = 1) of symbol `id`: the refined bitmap, its reference,
    /// and the reference offset (dx, dy) in refinement-region terms.
    pub refined: Option<(&'a Bitmap, &'a Bitmap, i32, i32)>,
}

/// Strip size (2^logstrips), S offset and reference corner (0 = bottom-left, 1 = top-left).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextParams {
    pub log_strips: u8,
    pub ds_offset: i8,
    pub corner: u8,
}

/// Encode a text region segment's data. The region (`width` x `height`) is placed at
/// (`x`, `y`) on the page; instance coordinates are relative to the region.
#[allow(clippy::too_many_arguments)]
pub(crate) fn text_region(
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    instances: &[Instance],
    num_symbols: usize,
    p: TextParams,
    bitmaps: bool,
) -> Vec<u8> {
    let refine = instances.iter().any(|i| i.refined.is_some());
    let mut d = Vec::new();
    for v in [width, height, x, y] {
        put32(&mut d, v);
    }
    d.push(0); // region combination operator: OR
    let strips = 1i32 << p.log_strips;
    let flags: u16 = (refine as u16) << 1 // SBREFINE (SBHUFF 0, SBRTEMPLATE 0)
        | (p.log_strips as u16) << 2
        | (p.corner as u16) << 4 // REFCORNER, TRANSPOSED 0, SBCOMBOP OR, SBDEFPIXEL 0
        | ((p.ds_offset as u16) & 0x1F) << 10;
    d.extend_from_slice(&flags.to_be_bytes());
    if refine {
        d.extend(REFINE_AT.iter().map(|&a| a as u8));
    }
    put32(&mut d, instances.len() as u32);

    let t_of = |i: &Instance| {
        if p.corner == 0 {
            i.bottom
        } else {
            i.bottom - i.height + 1
        }
    };
    let strip_of = |i: &Instance| t_of(i).div_euclid(strips) * strips;
    let mut sorted: Vec<&Instance> = instances.iter().collect();
    sorted.sort_by_key(|i| (strip_of(i), i.x)); // stable: equal keys keep their order

    let mut mq = Mq::new();
    let (mut iadt, mut iafs, mut iads, mut iait) = (
        IntCtx::default(),
        IntCtx::default(),
        IntCtx::default(),
        IntCtx::default(),
    );
    let (mut iari, mut iardw, mut iardh, mut iardx, mut iardy) = (
        IntCtx::default(),
        IntCtx::default(),
        IntCtx::default(),
        IntCtx::default(),
        IntCtx::default(),
    );
    let mut iaid = IaidCtx::new(code_len(num_symbols as u64));
    let mut gr = vec![0; REFINE_CONTEXTS];

    iadt.encode(&mut mq, Some(0)); // initial STRIPT = 0
    let (mut strip_t, mut first_s) = (0i32, 0i32);
    let mut k = 0;
    while k < sorted.len() {
        let base = strip_of(sorted[k]);
        iadt.encode(&mut mq, Some((base - strip_t) / strips));
        strip_t = base;
        let mut cur_s = 0i32;
        let mut first = true;
        while k < sorted.len() && strip_of(sorted[k]) == base {
            let s = sorted[k];
            if first {
                iafs.encode(&mut mq, Some(s.x - first_s));
                first_s = s.x;
                first = false;
            } else {
                iads.encode(&mut mq, Some(s.x - cur_s - p.ds_offset as i32));
            }
            if strips > 1 {
                iait.encode(&mut mq, Some(t_of(s) - strip_t));
            }
            iaid.encode(&mut mq, s.id);
            if refine {
                iari.encode(&mut mq, Some(s.refined.is_some() as i32));
                if let Some((bm, reference, dx, dy)) = s.refined {
                    let rdw = bm.width() as i32 - reference.width() as i32;
                    let rdh = bm.height() as i32 - reference.height() as i32;
                    iardw.encode(&mut mq, Some(rdw));
                    iardh.encode(&mut mq, Some(rdh));
                    // GRREFERENCEDX = floor(RDW/2) + RDX (arithmetic shift, as decoders do)
                    iardx.encode(&mut mq, Some(dx - (rdw >> 1)));
                    iardy.encode(&mut mq, Some(dy - (rdh >> 1)));
                    if bitmaps {
                        encode_refinement(&mut mq, &mut gr, bm, reference, dx, dy);
                    }
                }
            }
            cur_s = s.x + s.width - 1;
            k += 1;
        }
        iads.encode(&mut mq, None); // OOB ends the strip (decoders read it after the last instance too)
    }
    d.extend(mq.finish());
    d
}
