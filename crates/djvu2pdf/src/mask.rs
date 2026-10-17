//! JB2 mask → JBIG2, bit-exactly: the JB2 shapes, refinement parents and blits are
//! re-encoded as JBIG2 symbols, refinements and text-region instances.

use std::collections::HashMap;

use djvu_jb2::{Jb2Dict, Jb2Error, decode_structure};
use jbig2_encoder::{
    Bitmap, EncodeOptions, EncodedPage, Globals, Page, Placement, Shape, encode_globals,
    encode_page,
};

fn to_bitmap(b: &djvu_rs::Bitmap) -> Bitmap {
    Bitmap::from_packed(b.width, b.height, b.row_stride(), &b.data)
}

/// Whether to merge a document's shared JB2 dictionaries into one `/JBIG2Globals`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MergeDicts {
    /// Merge when the union (exact duplicates removed) has at most 80 % of the shapes of
    /// all dictionaries together. Born-digital files often repeat one font's glyphs in
    /// every dictionary; scans rarely share exact bitmaps.
    #[default]
    Auto,
    Always,
    Never,
}

/// The `/JBIG2Globals` streams for a document's shared JB2 dictionaries (Djbz).
pub struct SharedDicts {
    streams: Vec<Globals>,
    /// Per input dictionary: its stream and dictionary shape → global shape index.
    assignment: Vec<(usize, Vec<Option<usize>>)>,
    merged: bool,
}

/// The globals a page is encoded against.
#[derive(Clone, Copy)]
pub struct PageGlobals<'a> {
    /// Index into [`SharedDicts::streams`].
    pub stream: usize,
    globals: &'a Globals,
    /// The page's dictionary shape → global shape; `None` = no dictionary.
    map: Option<&'a [Option<usize>]>,
}

impl SharedDicts {
    /// Encode `dicts` (each distinct shared dictionary once, in first-use order).
    pub fn new(dicts: &[&Jb2Dict], merge: MergeDicts, options: &EncodeOptions) -> Self {
        let per_dict: Vec<Vec<Shape>> = dicts
            .iter()
            .map(|d| {
                (0..d.len())
                    .map(|i| {
                        let s = d.shape(i);
                        Shape {
                            bitmap: to_bitmap(&s.bitmap),
                            parent: s.parent,
                        }
                    })
                    .collect()
            })
            .collect();
        let total: usize = per_dict.iter().map(Vec::len).sum();

        // union with exact duplicates merged; parents precede their children
        let mut union: Vec<Shape> = Vec::new();
        let mut index: HashMap<Bitmap, usize> = HashMap::new();
        let mut maps = Vec::with_capacity(dicts.len());
        for shapes in &per_dict {
            let mut map: Vec<Option<usize>> = Vec::with_capacity(shapes.len());
            for s in shapes {
                if s.bitmap.is_empty() {
                    map.push(None);
                    continue;
                }
                let u = *index.entry(s.bitmap.clone()).or_insert_with(|| {
                    let parent = s.parent.and_then(|p| map[p]);
                    union.push(Shape {
                        bitmap: s.bitmap.clone(),
                        parent,
                    });
                    union.len() - 1
                });
                map.push(Some(u));
            }
            maps.push(map);
        }
        let merged = match merge {
            MergeDicts::Always => !dicts.is_empty(),
            MergeDicts::Never => false,
            MergeDicts::Auto => dicts.len() > 1 && union.len() as f64 <= 0.8 * total as f64,
        };

        if merged {
            Self {
                streams: vec![encode_globals(&union, options)],
                assignment: maps.into_iter().map(|m| (0, m)).collect(),
                merged,
            }
        } else {
            Self {
                assignment: per_dict
                    .iter()
                    .enumerate()
                    .map(|(k, shapes)| (k, (0..shapes.len()).map(Some).collect()))
                    .collect(),
                streams: per_dict
                    .iter()
                    .map(|shapes| encode_globals(shapes, options))
                    .collect(),
                merged,
            }
        }
    }

    /// Whether all dictionaries were merged into one stream (used by every page).
    pub fn merged(&self) -> bool {
        self.merged
    }

    /// The encoded `/JBIG2Globals` streams.
    pub fn streams(&self) -> &[Globals] {
        &self.streams
    }

    /// Globals for a page that uses dictionary `dict` (index into the `dicts` passed to
    /// [`SharedDicts::new`]), or none. With merged dictionaries every page uses the
    /// merged stream, so its glyphs can alias shared ones.
    pub fn page_globals(&self, dict: Option<usize>) -> Option<PageGlobals<'_>> {
        match dict {
            Some(k) => {
                let (stream, map) = &self.assignment[k];
                Some(PageGlobals {
                    stream: *stream,
                    globals: &self.streams[*stream],
                    map: Some(map),
                })
            }
            None if self.merged => Some(PageGlobals {
                stream: 0,
                globals: &self.streams[0],
                map: None,
            }),
            None => None,
        }
    }
}

/// One page's JB2 mask, decoded into shapes and placements ready for JBIG2 encoding.
pub struct PageMask<'a> {
    globals: Option<PageGlobals<'a>>,
    width: u32,
    height: u32,
    local: Vec<Shape>,
    /// Per JB2 blit, in blit order (the FGbz colour index order); `None` = empty shape.
    placements: Vec<Option<Placement>>,
}

impl<'a> PageMask<'a> {
    /// Decode a page's Sjbz. `dict` is the page's shared dictionary, `globals` its
    /// encoding from [`SharedDicts::page_globals`].
    pub fn new(
        sjbz: &[u8],
        dict: Option<&Jb2Dict>,
        globals: Option<PageGlobals<'a>>,
    ) -> Result<Self, Jb2Error> {
        let structure = decode_structure(sjbz, dict)?;
        let inherited = structure.inherited;
        let n_global = globals.map_or(0, |g| g.globals.len());
        let map = globals.and_then(|g| g.map).unwrap_or(&[]);
        // page shape index → combined (global, then local) index; None = empty
        let remap = |s: usize| -> Option<usize> {
            match s.checked_sub(inherited) {
                Some(i) => Some(n_global + i),
                None => map.get(s).copied().flatten(),
            }
        };
        let local: Vec<Shape> = structure
            .shapes
            .iter()
            .map(|s| Shape {
                bitmap: to_bitmap(&s.bitmap),
                parent: s.parent.and_then(remap),
            })
            .collect();
        let height_of = |shape: usize| -> i32 {
            match shape.checked_sub(n_global) {
                Some(i) => local[i].bitmap.height() as i32,
                None => globals.map_or(0, |g| g.globals.shape(shape).height() as i32),
            }
        };
        let page_height = structure.height as i32;
        let placements = structure
            .blits
            .iter()
            .map(|b| {
                let shape = remap(b.shape)?;
                Some(Placement {
                    shape,
                    x: b.left,
                    y: page_height - b.bottom - height_of(shape),
                })
            })
            .collect();
        Ok(Self {
            globals,
            width: structure.width,
            height: structure.height,
            local,
            placements,
        })
    }

    /// Mask width in pixels (the page width).
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Mask height in pixels (the page height).
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Number of JB2 blits.
    pub fn blit_count(&self) -> usize {
        self.placements.len()
    }

    /// Encode the blits selected by `select(blit index)` as a JBIG2 page of
    /// `canvas_width` x `canvas_height` pixels (at least the mask size), the mask
    /// sitting in its bottom-left corner — e.g. padded to cover a coarser colour layer.
    pub fn encode(
        &self,
        canvas_width: u32,
        canvas_height: u32,
        dpi: u32,
        select: impl Fn(usize) -> bool,
        options: &EncodeOptions,
    ) -> EncodedPage {
        let (w, h) = (canvas_width.max(self.width), canvas_height.max(self.height));
        let mut page = Page::new(w, h, dpi);
        page.region = (0, h - self.height, self.width, self.height);
        let placements: Vec<Placement> = self
            .placements
            .iter()
            .enumerate()
            .filter(|&(i, _)| select(i))
            .filter_map(|(_, p)| *p)
            .collect();
        encode_page(
            &page,
            self.globals.map(|g| g.globals),
            &self.local,
            &placements,
            options,
        )
    }
}

/// Encode one page's JB2 mask (Sjbz) as a whole-page JBIG2 stream. `dict` is the page's
/// shared dictionary, `globals` its encoding from [`SharedDicts::page_globals`].
pub fn encode_mask(
    sjbz: &[u8],
    dict: Option<&Jb2Dict>,
    globals: Option<PageGlobals>,
    dpi: u32,
    options: &EncodeOptions,
) -> Result<EncodedPage, Jb2Error> {
    let mask = PageMask::new(sjbz, dict, globals)?;
    Ok(mask.encode(mask.width, mask.height, dpi, |_| true, options))
}
