//! JBIG2 encoder producing PDF-embeddable streams (ITU-T T.88, PDF 32000-1 §7.4.7).
//!
//! Input is *structure*, not pixels: shapes (bitmaps, optionally refined from a parent
//! shape) and their placements. Encoding that structure as-is — no symbol
//! classification — makes the output lossless by construction; this is how a DjVu JB2
//! layer becomes JBIG2 bit-exactly.
//!
//! ```
//! use jbig2_encoder::{Bitmap, EncodeOptions, Page, Placement, Shape, encode_globals, encode_page};
//!
//! let glyph = Bitmap::from_fn(5, 7, |x, y| x == 0 || y == 6);
//! let globals = encode_globals(&[Shape { bitmap: glyph, parent: None }], &EncodeOptions::default());
//! let page = Page::new(100, 50, 300);
//! let placements = [Placement { shape: 0, x: 10, y: 20 }, Placement { shape: 0, x: 18, y: 20 }];
//! let stream = encode_page(&page, Some(&globals), &[], &placements, &EncodeOptions::default());
//! // `globals.data` goes into the /JBIG2Globals stream, `stream` into the image XObject.
//! # assert!(!stream.data.is_empty());
//! ```
//!
//! Decoder compatibility (verified against jbig2dec, pdf.js, Apple CoreGraphics and
//! poppler with the C++ prototype):
//! * no end-of-page / end-of-file segments (forbidden in PDF embedded streams);
//! * every segment refers to at most 4 others (the long form's retention field is
//!   parsed inconsistently between decoders);
//! * symbol counts in scope are never a power of two: Apple's decoder sizes symbol IDs
//!   as floor(log2 n)+1 bits instead of ceil(log2 n), so one unused padding symbol is
//!   added when needed.

mod bitmap;
mod dict;
pub mod mq;
mod region;
mod segment;
mod text;

use std::collections::HashMap;

pub use bitmap::Bitmap;
pub use text::TextParams;

use dict::{DictSymbol, Reference, symbol_dictionary};
use region::best_offset;
use segment::{
    IMMEDIATE_TEXT_REGION, PAGE_INFORMATION, SYMBOL_DICTIONARY, page_information, segment,
};
use text::{Instance, text_region};

/// A shape: a bitmap and the shape it can be coded as a refinement of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shape {
    pub bitmap: Bitmap,
    /// Shape index (in the same index space as [`Placement::shape`]) to refine from;
    /// must be smaller than this shape's own index. `None` = coded directly.
    pub parent: Option<usize>,
}

/// One placement of a shape. Coordinates are relative to the page region
/// ([`Page::region`]), top-left origin, y downwards; (x, y) is the shape's top-left pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    /// Shape index: `0..globals.len()` = global shapes, then the page's local shapes.
    pub shape: usize,
    pub x: i32,
    pub y: i32,
}

/// Encoder options.
#[derive(Clone, Debug)]
pub struct EncodeOptions {
    /// Use refinement coding for shapes with a parent (otherwise all symbols are coded
    /// directly). Default: true.
    pub refine: bool,
    /// Code single-use local refinements inline in the text region instead of adding
    /// them to the page dictionary. Default: true.
    pub inline_refine: bool,
    /// Try strip sizes / S offsets / reference corners per page and keep the smallest
    /// (≈0.1 % smaller, slower). Default: false.
    pub search: bool,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            refine: true,
            inline_refine: true,
            search: false,
        }
    }
}

/// Page geometry.
#[derive(Clone, Debug)]
pub struct Page {
    pub width: u32,
    pub height: u32,
    /// Resolution in pixels per metre (0 = unknown).
    pub resolution_ppm: u32,
    /// Region holding the placements: (x, y, width, height) on the page. Defaults to
    /// the whole page; a smaller region leaves the rest white (e.g. padding a mask to a
    /// coarser colour layer's grid).
    pub region: (u32, u32, u32, u32),
}

impl Page {
    /// Whole-page region at `dpi` dots per inch.
    pub fn new(width: u32, height: u32, dpi: u32) -> Self {
        Self {
            width,
            height,
            resolution_ppm: (dpi as f64 / 0.0254).round() as u32,
            region: (0, 0, width, height),
        }
    }
}

/// An encoded shared dictionary, to be stored as a PDF `/JBIG2Globals` stream and
/// passed to [`encode_page`] for every page that uses it.
#[derive(Clone, Debug)]
pub struct Globals {
    /// The `/JBIG2Globals` stream contents.
    pub data: Vec<u8>,
    shapes: Vec<Bitmap>,
    /// Symbol id of each global shape (None for empty bitmaps).
    symbol_id: Vec<Option<u32>>,
    /// Bitmaps in symbol-id order (including padding symbols).
    table: Vec<Bitmap>,
    segments: Vec<u32>,
    /// Exact-content index for aliasing local copies of global shapes.
    by_content: HashMap<Bitmap, u32>,
    refinements: bool,
}

impl Globals {
    /// Number of global shapes (the index space offset for local shapes).
    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }
}

/// Statistics of one encoded page.
#[derive(Clone, Debug, Default)]
pub struct PageStats {
    /// Local shapes coded in the page's dictionaries.
    pub dictionary_shapes: usize,
    /// Local shapes coded inline as text-region refinements.
    pub inline_shapes: usize,
    /// Local shapes replaced by an identical global shape.
    pub aliased_shapes: usize,
    pub text_params: TextParams,
}

/// An encoded page stream.
#[derive(Clone, Debug)]
pub struct EncodedPage {
    /// The image XObject's `/JBIG2Decode` stream contents.
    pub data: Vec<u8>,
    pub stats: PageStats,
}

/// Symbol table of one stream: bitmaps in symbol-id (SBSYMS) order.
struct SymbolTable<'a> {
    bitmaps: Vec<&'a Bitmap>,
}

/// Unused 1x1 symbol that keeps symbol counts off powers of two.
static PAD: std::sync::LazyLock<Bitmap> = std::sync::LazyLock::new(|| Bitmap::new(1, 1));

fn is_pow2(n: usize) -> bool {
    n >= 1 && n & (n - 1) == 0
}

/// Encode `ids` (indices into `shapes`) as at most two dictionary segments — a generic
/// one and a refinement one (layered so references point backwards) — so every segment
/// refers to at most 4 others. `symbol_id[s]` is set for every encoded shape.
#[allow(clippy::too_many_arguments)]
fn encode_dictionaries<'a>(
    out: &mut Vec<u8>,
    next_segment: &mut u32,
    page: u8,
    ids: &[usize],
    shapes: &[&'a Bitmap],
    parent: &[Option<usize>],
    symbol_id: &mut [Option<u32>],
    table: &mut SymbolTable<'a>,
    dict_segments: &mut Vec<u32>,
    refine: bool,
) {
    // layer: None = generic, Some(l) = refinement depth
    let mut layer: Vec<Option<u32>> = vec![None; shapes.len()];
    let mut here = vec![false; shapes.len()];
    for &s in ids {
        layer[s] = match parent[s].filter(|_| refine) {
            None => None,
            Some(p) if symbol_id[p].is_some() => Some(0), // parent from an earlier dictionary
            Some(p) if here[p] => Some(layer[p].map_or(0, |l| l + 1)),
            Some(_) => None, // parent not encodable (empty): code directly
        };
        here[s] = true;
    }
    let n_ref = ids.iter().filter(|&&s| layer[s].is_some()).count();
    let pad = is_pow2(table.bitmaps.len() + ids.len()) && (n_ref > 0 || page != 0);

    for pass in 0..2 {
        let refagg = pass == 1;
        let mut syms: Vec<DictSymbol<'a>> = Vec::new();
        let mut who: Vec<Option<usize>> = Vec::new();
        let mut index_in_dict: HashMap<usize, usize> = HashMap::new();
        if pass == 0 && pad {
            syms.push(DictSymbol {
                bitmap: &PAD,
                layer: 0,
                reference: None,
            });
            who.push(None);
        }
        for &s in ids {
            if layer[s].is_some() != refagg {
                continue;
            }
            let reference = if refagg {
                let p = parent[s].expect("refined shape has a parent");
                let r = match symbol_id[p] {
                    Some(id) => Reference::Input(id as usize),
                    None => Reference::New(index_in_dict[&p]),
                };
                let (dx, dy) = best_offset(shapes[s], shapes[p]);
                Some((r, dx, dy))
            } else {
                None
            };
            index_in_dict.insert(s, syms.len());
            syms.push(DictSymbol {
                bitmap: shapes[s],
                layer: layer[s].unwrap_or(0),
                reference,
            });
            who.push(Some(s));
        }
        if syms.is_empty() {
            continue;
        }
        let inputs: Vec<&Bitmap> = if refagg {
            table.bitmaps.clone()
        } else {
            Vec::new()
        };
        let (data, order) = symbol_dictionary(&syms, &inputs, refagg);
        let refs: Vec<u32> = if refagg {
            dict_segments.clone()
        } else {
            Vec::new()
        };
        segment(out, *next_segment, SYMBOL_DICTIONARY, &refs, page, &data);
        dict_segments.push(*next_segment);
        *next_segment += 1;
        for k in order {
            match who[k] {
                Some(s) => {
                    symbol_id[s] = Some(table.bitmaps.len() as u32);
                    table.bitmaps.push(shapes[s]);
                }
                None => table.bitmaps.push(&*PAD),
            }
        }
    }
}

/// Encode shared shapes as a `/JBIG2Globals` stream (segments with page association 0).
/// `shapes[i].parent` refers to `shapes` itself.
pub fn encode_globals(shapes: &[Shape], options: &EncodeOptions) -> Globals {
    let bitmaps: Vec<&Bitmap> = shapes.iter().map(|s| &s.bitmap).collect();
    let parent: Vec<Option<usize>> = shapes.iter().map(|s| s.parent).collect();
    let ids: Vec<usize> = (0..shapes.len())
        .filter(|&i| !shapes[i].bitmap.is_empty())
        .collect();
    let mut symbol_id = vec![None; shapes.len()];
    let mut table = SymbolTable {
        bitmaps: Vec::new(),
    };
    let mut segments = Vec::new();
    let mut data = Vec::new();
    let mut next = 0;
    encode_dictionaries(
        &mut data,
        &mut next,
        0,
        &ids,
        &bitmaps,
        &parent,
        &mut symbol_id,
        &mut table,
        &mut segments,
        options.refine,
    );
    let table: Vec<Bitmap> = table.bitmaps.into_iter().cloned().collect();
    let mut by_content = HashMap::new();
    for (i, s) in shapes.iter().enumerate() {
        if let Some(id) = symbol_id[i] {
            by_content.entry(s.bitmap.clone()).or_insert(id);
        }
    }
    Globals {
        data,
        shapes: shapes.iter().map(|s| s.bitmap.clone()).collect(),
        symbol_id,
        table,
        segments,
        by_content,
        refinements: options.refine && shapes.iter().any(|s| s.parent.is_some()),
    }
}

/// Encode one page: page information, local dictionaries and one text region.
///
/// `local` shapes have indices `globals.len() + i`; their parents and all placements
/// use that combined index space.
pub fn encode_page(
    page: &Page,
    globals: Option<&Globals>,
    local: &[Shape],
    placements: &[Placement],
    options: &EncodeOptions,
) -> EncodedPage {
    let n_global = globals.map_or(0, |g| g.len());
    let n = n_global + local.len();
    let mut bitmaps: Vec<&Bitmap> = Vec::with_capacity(n);
    let mut parent: Vec<Option<usize>> = vec![None; n];
    let mut symbol_id: Vec<Option<u32>> = vec![None; n];
    if let Some(g) = globals {
        bitmaps.extend(g.shapes.iter());
        symbol_id[..n_global].copy_from_slice(&g.symbol_id);
    }
    for (i, s) in local.iter().enumerate() {
        bitmaps.push(&s.bitmap);
        parent[n_global + i] = s.parent;
        if let Some(p) = s.parent {
            assert!(p < n_global + i, "parent must precede its shape");
        }
    }
    let mut table = SymbolTable {
        bitmaps: globals.map_or_else(Vec::new, |g| g.table.iter().collect()),
    };

    let mut uses = vec![0usize; n];
    for p in placements {
        uses[p.shape] += 1;
    }
    let mut is_parent = vec![false; n];
    for s in local.iter() {
        if let Some(p) = s.parent {
            is_parent[p] = true;
        }
    }

    let mut stats = PageStats::default();
    // local shapes: alias exact copies of global shapes, inline single-use refinements,
    // put the rest into the page dictionaries
    let mut inline = vec![false; n];
    let mut dict_ids = Vec::new();
    for i in n_global..n {
        let bm = bitmaps[i];
        if bm.is_empty() || (uses[i] == 0 && !is_parent[i]) {
            continue;
        }
        if let Some(&id) = globals.and_then(|g| g.by_content.get(bm)) {
            symbol_id[i] = Some(id);
            stats.aliased_shapes += 1;
            continue;
        }
        let usable_parent = parent[i].is_some_and(|p| !bitmaps[p].is_empty());
        if options.refine && options.inline_refine && uses[i] == 1 && !is_parent[i] && usable_parent
        {
            inline[i] = true;
            stats.inline_shapes += 1;
        } else {
            dict_ids.push(i);
        }
    }
    stats.dictionary_shapes = dict_ids.len();

    let mut out = Vec::new();
    let refinements = options.refine
        && (globals.is_some_and(|g| g.refinements) || local.iter().any(|s| s.parent.is_some()));
    let mut next = globals.map_or(0, |g| g.segments.len() as u32);
    segment(
        &mut out,
        next,
        PAGE_INFORMATION,
        &[],
        1,
        &page_information(page.width, page.height, page.resolution_ppm, refinements),
    );
    next += 1;
    let mut dict_segments = globals.map_or_else(Vec::new, |g| g.segments.clone());
    encode_dictionaries(
        &mut out,
        &mut next,
        1,
        &dict_ids,
        &bitmaps,
        &parent,
        &mut symbol_id,
        &mut table,
        &mut dict_segments,
        options.refine,
    );

    let mut instances = Vec::with_capacity(placements.len());
    for p in placements {
        let s = p.shape;
        let bm = bitmaps[s];
        if bm.is_empty() {
            continue;
        }
        let bottom = p.y + bm.height() as i32 - 1;
        let refined = if inline[s] {
            let par = parent[s].expect("inline shape has a parent");
            let reference = bitmaps[par];
            let (dx, dy) = best_offset(bm, reference);
            Some((par, (bm, reference, dx, dy)))
        } else {
            None
        };
        let id = match refined {
            Some((par, _)) => symbol_id[par],
            None => symbol_id[s],
        };
        let Some(id) = id else { continue };
        instances.push(Instance {
            id,
            x: p.x,
            bottom,
            width: bm.width() as i32,
            height: bm.height() as i32,
            refined: refined.map(|(_, r)| r),
        });
    }

    let (rx, ry, rw, rh) = page.region;
    let n_symbols = table.bitmaps.len();
    let candidates: Vec<TextParams> = if options.search {
        let mut v = Vec::new();
        for corner in 0..=1 {
            for log_strips in 0..=3 {
                for ds_offset in -2..=4 {
                    v.push(TextParams {
                        log_strips,
                        ds_offset,
                        corner,
                    });
                }
            }
        }
        v
    } else {
        (0..=3)
            .map(|log_strips| TextParams {
                log_strips,
                ds_offset: 0,
                corner: 0,
            })
            .collect()
    };
    let best = candidates
        .into_iter()
        .min_by_key(|&p| text_region(rw, rh, rx, ry, &instances, n_symbols, p, false).len())
        .unwrap_or_default();
    stats.text_params = best;
    let region = text_region(rw, rh, rx, ry, &instances, n_symbols, best, true);
    segment(
        &mut out,
        next,
        IMMEDIATE_TEXT_REGION,
        &dict_segments,
        1,
        &region,
    );
    EncodedPage { data: out, stats }
}
