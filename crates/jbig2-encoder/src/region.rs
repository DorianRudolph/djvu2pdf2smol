//! Generic region (T.88 6.2) and generic refinement region (6.3) coding.
//!
//! Only the context *pixel sets* have to match the decoder; any fixed bijection from
//! pixel patterns to context indices gives an identical adaptive model.

use crate::bitmap::Bitmap;
use crate::mq::{Cx, Mq};

/// Adaptive-template pixels used with GBTEMPLATE 0 (the T.88 defaults), as signed bytes
/// in segment order A1x, A1y, A2x, A2y, A3x, A3y, A4x, A4y.
pub const GENERIC_AT: [i8; 8] = [3, -1, -3, -1, 2, -2, -2, -2];
/// Adaptive-template pixels used with GRTEMPLATE 0: A1 (current image), A2 (reference).
pub const REFINE_AT: [i8; 4] = [-1, -1, -1, -1];

/// Number of contexts for [`encode_generic`] (16-bit context).
pub const GENERIC_CONTEXTS: usize = 1 << 16;
/// Number of contexts for [`encode_refinement`] (13-bit context).
pub const REFINE_CONTEXTS: usize = 1 << 13;

/// Generic region, GBTEMPLATE 0, default AT pixels, TPGDON 0.
pub fn encode_generic(mq: &mut Mq, cx: &mut [Cx], b: &Bitmap) {
    debug_assert_eq!(cx.len(), GENERIC_CONTEXTS);
    for y in 0..b.height() as i32 {
        for x in 0..b.width() as i32 {
            let c = (b.get(x - 1, y) as usize)
                | (b.get(x - 2, y) as usize) << 1
                | (b.get(x - 3, y) as usize) << 2
                | (b.get(x - 4, y) as usize) << 3
                | (b.get(x + 3, y - 1) as usize) << 4 // A1
                | (b.get(x + 2, y - 1) as usize) << 5
                | (b.get(x + 1, y - 1) as usize) << 6
                | (b.get(x, y - 1) as usize) << 7
                | (b.get(x - 1, y - 1) as usize) << 8
                | (b.get(x - 2, y - 1) as usize) << 9
                | (b.get(x - 3, y - 1) as usize) << 10 // A2
                | (b.get(x + 2, y - 2) as usize) << 11 // A3
                | (b.get(x + 1, y - 2) as usize) << 12
                | (b.get(x, y - 2) as usize) << 13
                | (b.get(x - 1, y - 2) as usize) << 14
                | (b.get(x - 2, y - 2) as usize) << 15; // A4
            mq.encode(&mut cx[c], b.get(x, y) != 0);
        }
    }
}

/// Generic refinement region, GRTEMPLATE 0 with AT (-1,-1) (-1,-1), TPGRON 0.
/// The reference pixel for (x, y) is `reference(x - dx, y - dy)`.
pub fn encode_refinement(
    mq: &mut Mq,
    cx: &mut [Cx],
    b: &Bitmap,
    reference: &Bitmap,
    dx: i32,
    dy: i32,
) {
    debug_assert_eq!(cx.len(), REFINE_CONTEXTS);
    for y in 0..b.height() as i32 {
        for x in 0..b.width() as i32 {
            let (rx, ry) = (x - dx, y - dy);
            let c = (b.get(x - 1, y) as usize)
                | (b.get(x + 1, y - 1) as usize) << 1
                | (b.get(x, y - 1) as usize) << 2
                | (b.get(x - 1, y - 1) as usize) << 3
                | (reference.get(rx + 1, ry + 1) as usize) << 4
                | (reference.get(rx, ry + 1) as usize) << 5
                | (reference.get(rx - 1, ry + 1) as usize) << 6
                | (reference.get(rx + 1, ry) as usize) << 7
                | (reference.get(rx, ry) as usize) << 8
                | (reference.get(rx - 1, ry) as usize) << 9
                | (reference.get(rx + 1, ry - 1) as usize) << 10
                | (reference.get(rx, ry - 1) as usize) << 11
                | (reference.get(rx - 1, ry - 1) as usize) << 12;
            mq.encode(&mut cx[c], b.get(x, y) != 0);
        }
    }
}

/// Offset (dx, dy) placing `parent` under `child` for refinement: centre alignment
/// (as JB2 does) refined by the best match within ±1 pixel; ties keep the centre.
///
/// pdfium's fast refinement path (template 0, default AT pixels, dx = 0, equal widths)
/// resets dy to 0 when |dy| ≥ the parent's height, so such offsets are never chosen;
/// the window always has a usable one with dx ≠ 0.
pub fn best_offset(child: &Bitmap, parent: &Bitmap) -> (i32, i32) {
    let cx = (child.width() as i32 - parent.width() as i32) / 2;
    let cy = (child.height() as i32 - parent.height() as i32) / 2;
    let mismatches = |dx: i32, dy: i32| -> u64 {
        let mut err = 0;
        for y in 0..child.height() as i32 {
            for x in 0..child.width() as i32 {
                err += (child.get(x, y) != parent.get(x - dx, y - dy)) as u64;
            }
        }
        err
    };
    let pdfium_safe = |dx: i32, dy: i32| {
        dx != 0 || child.width() != parent.width() || dy.unsigned_abs() < parent.height()
    };
    let mut best = pdfium_safe(cx, cy).then(|| (mismatches(cx, cy), cx, cy));
    for dy in cy - 1..=cy + 1 {
        for dx in cx - 1..=cx + 1 {
            if (dx, dy) == (cx, cy) || !pdfium_safe(dx, dy) {
                continue;
            }
            let e = mismatches(dx, dy);
            if best.is_none_or(|b| e < b.0) {
                best = Some((e, dx, dy));
            }
        }
    }
    let (_, dx, dy) = best.expect("offsets with dx != 0 are always usable");
    (dx, dy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_offset_avoids_pdfium_dy_reset() {
        // a 16x1 bar refined into a 16x2 glyph whose lower row matches it: the best
        // match is (0, 1), which pdfium decodes as (0, 0)
        let parent = Bitmap::from_fn(16, 1, |_, _| true);
        let child = Bitmap::from_fn(16, 2, |x, y| {
            y == 1 && x != 2 && x != 4 || y == 0 && (2..5).contains(&x)
        });
        let (dx, dy) = best_offset(&child, &parent);
        assert!(
            dx != 0 || dy.unsigned_abs() < parent.height(),
            "({dx}, {dy})"
        );
        // unaffected when the reference is tall enough
        let parent = Bitmap::from_fn(16, 2, |_, y| y == 1);
        assert_eq!(best_offset(&child, &parent), (0, 0));
    }
}
