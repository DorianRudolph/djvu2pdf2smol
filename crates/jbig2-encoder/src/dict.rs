//! Symbol dictionary segments (T.88 6.5 / 7.4.2), arithmetic coding only.

use crate::bitmap::Bitmap;
use crate::mq::{IaidCtx, IntCtx, Mq, code_len};
use crate::region::{
    GENERIC_AT, GENERIC_CONTEXTS, REFINE_AT, REFINE_CONTEXTS, encode_generic, encode_refinement,
};
use crate::segment::{put16, put32};

/// Where a refinement symbol's reference comes from.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Reference {
    /// Index into the dictionary's input symbols (SDINSYMS).
    Input(usize),
    /// Index into the `symbols` being encoded in the same dictionary (must have a
    /// smaller `layer`, so it is decoded first).
    New(usize),
}

pub(crate) struct DictSymbol<'a> {
    pub bitmap: &'a Bitmap,
    /// Refinement depth inside this dictionary (orders the symbols).
    pub layer: u32,
    /// `None` = generic coding (only in dictionaries without SDREFAGG).
    pub reference: Option<(Reference, i32, i32)>,
}

/// Encode one symbol dictionary segment's data. Returns the data and the export order
/// (indices into `symbols`): exported symbol k is `symbols[order[k]]`.
///
/// With `refagg`, every symbol must have a reference (coded as a one-instance
/// refinement/aggregate); without, every symbol is generic-coded.
pub(crate) fn symbol_dictionary(
    symbols: &[DictSymbol],
    inputs: &[&Bitmap],
    refagg: bool,
) -> (Vec<u8>, Vec<usize>) {
    let mut order: Vec<usize> = (0..symbols.len()).collect();
    order.sort_by_key(|&i| {
        (
            symbols[i].layer,
            symbols[i].bitmap.height(),
            symbols[i].bitmap.width(),
        )
    });
    let mut position = vec![0usize; symbols.len()];
    for (k, &i) in order.iter().enumerate() {
        position[i] = k;
    }

    let mut d = Vec::new();
    // flags: SDHUFF 0, SDREFAGG, SDTEMPLATE 0, SDRTEMPLATE 0, contexts neither used nor retained
    put16(&mut d, if refagg { 2 } else { 0 });
    d.extend(GENERIC_AT.iter().map(|&a| a as u8));
    if refagg {
        d.extend(REFINE_AT.iter().map(|&a| a as u8));
    }
    put32(&mut d, symbols.len() as u32); // SDNUMEXSYMS
    put32(&mut d, symbols.len() as u32); // SDNUMNEWSYMS

    let mut mq = Mq::new();
    let (mut iadh, mut iadw, mut iaex, mut iaai, mut iardx, mut iardy) = (
        IntCtx::default(),
        IntCtx::default(),
        IntCtx::default(),
        IntCtx::default(),
        IntCtx::default(),
        IntCtx::default(),
    );
    let mut iaid = IaidCtx::new(code_len((inputs.len() + symbols.len()) as u64));
    let mut gb = vec![0; GENERIC_CONTEXTS];
    let mut gr = vec![0; REFINE_CONTEXTS];

    let mut class_height = 0i32;
    let mut k = 0;
    while k < order.len() {
        let first = &symbols[order[k]];
        let (h, layer) = (first.bitmap.height() as i32, first.layer);
        iadh.encode(&mut mq, Some(h - class_height));
        class_height = h;
        let mut width = 0i32;
        while k < order.len()
            && symbols[order[k]].bitmap.height() as i32 == h
            && symbols[order[k]].layer == layer
        {
            let s = &symbols[order[k]];
            iadw.encode(&mut mq, Some(s.bitmap.width() as i32 - width));
            width = s.bitmap.width() as i32;
            if refagg {
                let (reference, dx, dy) = s
                    .reference
                    .expect("refinement dictionary symbol needs a reference");
                let (id, ref_bitmap) = match reference {
                    Reference::Input(i) => (i, inputs[i]),
                    Reference::New(j) => {
                        assert!(
                            position[j] < k,
                            "reference must be decoded before its refinement"
                        );
                        (inputs.len() + position[j], symbols[j].bitmap)
                    }
                };
                iaai.encode(&mut mq, Some(1)); // REFAGGNINST = 1: a plain refinement
                iaid.encode(&mut mq, id as u32);
                iardx.encode(&mut mq, Some(dx));
                iardy.encode(&mut mq, Some(dy));
                encode_refinement(&mut mq, &mut gr, s.bitmap, ref_bitmap, dx, dy);
            } else {
                encode_generic(&mut mq, &mut gb, s.bitmap);
            }
            k += 1;
        }
        iadw.encode(&mut mq, None); // OOB ends the height class
    }
    // export flags: skip the input symbols, export all new ones
    iaex.encode(&mut mq, Some(inputs.len() as i32));
    iaex.encode(&mut mq, Some(symbols.len() as i32));
    d.extend(mq.finish());
    (d, order)
}
