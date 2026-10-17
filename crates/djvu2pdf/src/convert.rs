//! Document conversion: DjVu → layered PDF.
//!
//! Shared work (JB2 dictionaries → `/JBIG2Globals`, text fonts) is done first; then
//! pages are converted independently on worker threads and written in order as they
//! finish, so output streams to the writer with bounded memory.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

use djvu_jb2::Jb2Dict;
use djvu_rs::Rotation;
use djvu_rs::annotation::Shape as AreaShape;
use djvu_rs::djvu_document::{DjVuBookmark, DjVuDocument, DjVuPage, ForegroundPalette};
use djvu_rs::iw44::Iw44Image;
use jbig2_encoder::EncodeOptions;
use pdf_writer::{Filter, Name, Ref, Str, TextStr};

use crate::jpx;
use crate::mask::{MergeDicts, PageGlobals, PageMask, SharedDicts};
use crate::pdffile::{PdfFile, chunk, deflate};
use crate::text::{self, GLYPHLESS_TTF, Line, TextFonts};

/// How a colour layer (BG44/FG44) is coded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayerCoding {
    /// JPEG 2000 at `factor` × the bytes the DjVu spent on the layer.
    Rate(f32),
    /// Lossless JPEG 2000 (reversible 5/3 wavelet) of the decoded layer.
    Lossless,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub background: LayerCoding,
    pub foreground: LayerCoding,
    pub text: bool,
    pub outline: bool,
    pub links: bool,
    pub merge_dicts: MergeDicts,
    pub jbig2: EncodeOptions,
    /// Worker threads (at least 1).
    pub jobs: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            background: LayerCoding::Rate(1.0),
            foreground: LayerCoding::Rate(2.0),
            text: true,
            outline: true,
            links: true,
            merge_dicts: MergeDicts::Auto,
            jbig2: EncodeOptions::default(),
            jobs: 1,
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Parse(String),
    Page(usize, String),
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(e) => write!(f, "cannot parse DjVu: {e}"),
            Error::Page(n, e) => write!(f, "page {}: {e}", n + 1),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Conversion statistics.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub pages: usize,
    /// Pages with a colour background layer.
    pub compound_pages: usize,
    pub text_pages: usize,
    pub links: usize,
    pub bookmarks: usize,
    pub jbig2_dictionaries: usize,
    pub jbig2_merged: bool,
    pub bytes: u64,
}

// ------------------------------------------------------------------ page output

enum XObject {
    Mask {
        data: Vec<u8>,
        w: u32,
        h: u32,
        globals: Option<usize>,
    },
    Image {
        data: Vec<u8>,
        w: u32,
        h: u32,
        mask: Option<Box<XObject>>,
    },
}

enum Target {
    Page(usize),
    Uri(String),
}

struct PageOut {
    /// Page size in DjVu pixels (unrotated) and resolution.
    width: u32,
    height: u32,
    dpi: u32,
    rotate: i32,
    content: Vec<u8>,
    xobjects: Vec<(String, XObject)>,
    fonts: BTreeSet<usize>,
    links: Vec<([i32; 4], Target)>,
    compound: bool,
}

fn rgb_ops(rgb: [u8; 3]) -> String {
    let c = |v: u8| {
        let s = format!("{:.4}", v as f64 / 255.0);
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s.is_empty() {
            "0".to_string()
        } else {
            s.to_string()
        }
    };
    format!("{} {} {} rg", c(rgb[0]), c(rgb[1]), c(rgb[2]))
}

/// Subsampling factor of a layer of `rw` x `rh` covering a `w` x `h` page (as
/// djvulibre's `DjVuImage::compute_red`).
fn compute_red(w: u32, h: u32, rw: u32, rh: u32) -> u32 {
    (1..16)
        .find(|&r| w.div_ceil(r) == rw && h.div_ceil(r) == rh)
        .unwrap_or(16)
}

/// A decoded colour layer: interleaved samples (1 = gray, 3 = RGB).
struct Layer {
    w: u32,
    h: u32,
    comps: u32,
    samples: Vec<u8>,
}

impl Layer {
    fn decode(chunks: &[&[u8]]) -> Result<Option<Self>, String> {
        if chunks.is_empty() {
            return Ok(None);
        }
        let mut img = Iw44Image::new();
        for c in chunks {
            img.decode_chunk(c).map_err(|e| e.to_string())?;
        }
        if img.is_color() {
            let p = img.to_rgb().map_err(|e| e.to_string())?;
            let samples = p
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|&[r, g, b, _]| [r, g, b])
                .collect();
            Ok(Some(Self {
                w: p.width,
                h: p.height,
                comps: 3,
                samples,
            }))
        } else {
            let p = img.to_gray8().map_err(|e| e.to_string())?;
            Ok(Some(Self {
                w: p.width,
                h: p.height,
                comps: 1,
                samples: p.data,
            }))
        }
    }

    /// The mean colour if every sample is within 2 levels of it.
    fn uniform(&self) -> Option<[u8; 3]> {
        let c = self.comps as usize;
        let n = (self.w * self.h) as u64;
        if n == 0 {
            return Some([255; 3]);
        }
        let mut sum = [0u64; 3];
        for px in self.samples.chunks_exact(c) {
            for k in 0..c {
                sum[k] += px[k] as u64;
            }
        }
        let mean: Vec<i32> = (0..c)
            .map(|k| ((sum[k] as f64) / n as f64).round() as i32)
            .collect();
        let ok = self
            .samples
            .chunks_exact(c)
            .all(|px| (0..c).all(|k| (px[k] as i32 - mean[k]).abs() <= 2));
        ok.then(|| {
            if c == 3 {
                [mean[0], mean[1], mean[2]].map(|v| v as u8)
            } else {
                [mean[0] as u8; 3]
            }
        })
    }

    fn encode(&self, coding: LayerCoding, djvu_bytes: usize) -> Result<Vec<u8>, String> {
        let target = match coding {
            LayerCoding::Rate(f) => Some(((f as f64 * djvu_bytes as f64) as usize).max(256)),
            LayerCoding::Lossless => None,
        };
        jpx::encode(&self.samples, self.w, self.h, self.comps, target)
            .ok_or_else(|| "JPEG 2000 encoding failed".into())
    }
}

struct Shared<'a> {
    doc: &'a DjVuDocument,
    options: &'a Options,
    dicts: SharedDicts,
    /// Per page: index of its JB2 dictionary in `dict_refs`.
    page_dict: Vec<Option<usize>>,
    dict_refs: Vec<&'a Jb2Dict>,
    lines: Vec<Vec<Line>>,
    fonts: Option<TextFonts>,
    page_ids: HashMap<String, usize>,
}

impl Shared<'_> {
    fn page(&self, n: usize) -> Result<PageOut, String> {
        let page = self.doc.page(n).map_err(|e| e.to_string())?;
        // rare layer encodings we do not convert: fail rather than drop the layer
        let unsupported: Vec<&str> = ["Smmr", "BGjp", "FGjp", "BG2k", "FG2k"]
            .into_iter()
            .filter(|id| page.find_chunk(id.as_bytes().try_into().unwrap()).is_some())
            .collect();
        if !unsupported.is_empty() {
            return Err(format!(
                "unsupported layer encoding ({})",
                unsupported.join(", ")
            ));
        }
        let (w, h) = (page.width() as u32, page.height() as u32);
        let dpi = match page.dpi() {
            0 => 300,
            d => d as u32,
        };
        let rotate = match page.rotation() {
            Rotation::None => 0,
            Rotation::Ccw90 => 270,
            Rotation::Rot180 => 180,
            Rotation::Cw90 => 90,
        };
        let k = 72.0 / dpi as f64;
        let mut out = PageOut {
            width: w,
            height: h,
            dpi,
            rotate,
            content: Vec::new(),
            xobjects: Vec::new(),
            fonts: BTreeSet::new(),
            links: Vec::new(),
            compound: false,
        };
        let mut ops = vec![format!("q {} 0 0 {} 0 0 cm", fmt_num(k), fmt_num(k))];

        // background
        let bg_chunks = page.bg44_chunks();
        if let Some(bg) = Layer::decode(&bg_chunks)? {
            out.compound = true;
            let red = compute_red(w, h, bg.w, bg.h);
            match bg.uniform() {
                Some([255, 255, 255]) => {}
                Some(rgb) => ops.push(format!("{} 0 0 {w} {h} re f", rgb_ops(rgb))),
                None => {
                    let bytes: usize = bg_chunks.iter().map(|c| c.len()).sum();
                    let data = bg.encode(self.options.background, bytes)?;
                    out.xobjects.push((
                        "B".into(),
                        XObject::Image {
                            data,
                            w: bg.w,
                            h: bg.h,
                            mask: None,
                        },
                    ));
                    ops.push(format!(
                        "q {} 0 0 {} 0 0 cm /B Do Q",
                        bg.w * red,
                        bg.h * red
                    ));
                }
            }
        }

        // foreground: JB2 mask painted with FG44 colours, palette colours or black
        if let Some(sjbz) = page.find_chunk(b"Sjbz") {
            self.foreground(page, sjbz, n, &mut out, &mut ops)?;
        }

        out.content = ops.join("\n").into_bytes();
        // text (single-byte font codes, not necessarily ASCII)
        if let Some(fonts) = &self.fonts
            && !self.lines[n].is_empty()
        {
            out.content.push(b'\n');
            out.content
                .extend(text::text_ops(&self.lines[n], fonts, &mut out.fonts));
        }
        out.content.extend_from_slice(b"\nQ");

        if self.options.links {
            out.links = self.links(page, n);
        }
        Ok(out)
    }

    fn foreground(
        &self,
        page: &DjVuPage,
        sjbz: &[u8],
        n: usize,
        out: &mut PageOut,
        ops: &mut Vec<String>,
    ) -> Result<(), String> {
        let (w, h) = (out.width, out.height);
        let dict = self.page_dict[n].map(|k| self.dict_refs[k]);
        let globals: Option<PageGlobals> = self.dicts.page_globals(self.page_dict[n]);
        let gstream = globals.map(|g| g.stream);
        let mask = PageMask::new(sjbz, dict, globals).map_err(|e| e.to_string())?;
        let opts = &self.options.jbig2;
        let encode_mask = |mw: u32, mh: u32, select: &dyn Fn(usize) -> bool| XObject::Mask {
            data: mask.encode(mw, mh, out.dpi, select, opts).data,
            w: mw,
            h: mh,
            globals: gstream,
        };

        let fg_chunks = page.fg44_chunks();
        if let Some(fg) = Layer::decode(&fg_chunks)? {
            // FG44 samples cover red x red page pixels; pad the mask to the same grid
            let red = compute_red(w, h, fg.w, fg.h);
            let (mw, mh) = (w.max(fg.w * red), h.max(fg.h * red));
            let m = encode_mask(mw, mh, &|_| true);
            match fg.uniform() {
                Some(rgb) => {
                    out.xobjects.push(("M".into(), m));
                    ops.push(format!("q {} {mw} 0 0 {mh} 0 0 cm /M Do Q", rgb_ops(rgb)));
                }
                None => {
                    let bytes: usize = fg_chunks.iter().map(|c| c.len()).sum();
                    let data = fg.encode(self.options.foreground, bytes)?;
                    let img = XObject::Image {
                        data,
                        w: fg.w,
                        h: fg.h,
                        mask: Some(Box::new(m)),
                    };
                    out.xobjects.push(("F".into(), img));
                    ops.push(format!("q {mw} 0 0 {mh} 0 0 cm /F Do Q"));
                }
            }
            return Ok(());
        }

        let ForegroundPalette { colors, indices } = page
            .foreground_palette()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        if colors.len() > 1 && indices.len() == mask.blit_count() {
            // one stencil per palette colour, each with only that colour's blits
            for (c, &rgb) in colors.iter().enumerate() {
                if !indices.iter().any(|&i| i as usize == c) {
                    continue;
                }
                let m = encode_mask(w, h, &|b| indices[b] as usize == c);
                out.xobjects.push((format!("M{c}"), m));
                ops.push(format!("q {} {w} 0 0 {h} 0 0 cm /M{c} Do Q", rgb_ops(rgb)));
            }
        } else {
            let rgb = colors.first().copied().unwrap_or([0, 0, 0]);
            out.xobjects
                .push(("M".into(), encode_mask(w, h, &|_| true)));
            ops.push(format!("q {} {w} 0 0 {h} 0 0 cm /M Do Q", rgb_ops(rgb)));
        }
        Ok(())
    }

    /// Resolve a DjVu URL to a 0-based page: `#12` (page number), `#+1`/`#-1`
    /// (relative to `from`), or `#<page id>`.
    fn target_page(&self, url: &str, from: usize) -> Option<usize> {
        let t = url.strip_prefix('#')?;
        let n = self.doc.page_count() as i64;
        let p = if t.starts_with('+') || t.starts_with('-') {
            from as i64 + t.parse::<i64>().ok()?
        } else if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
            t.parse::<i64>().ok()? - 1
        } else {
            *self.page_ids.get(t)? as i64
        };
        (0..n).contains(&p).then_some(p as usize)
    }

    fn links(&self, page: &DjVuPage, n: usize) -> Vec<([i32; 4], Target)> {
        let Ok(areas) = page.hyperlinks() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for a in areas {
            let r = match &a.shape {
                AreaShape::Rect(r) | AreaShape::Oval(r) | AreaShape::Text(r) => [
                    r.x as i32,
                    r.y as i32,
                    (r.x + r.width) as i32,
                    (r.y + r.height) as i32,
                ],
                AreaShape::Poly(pts) if pts.len() >= 2 => {
                    let xs = pts.iter().map(|p| p.0 as i32);
                    let ys = pts.iter().map(|p| p.1 as i32);
                    [
                        xs.clone().min().unwrap(),
                        ys.clone().min().unwrap(),
                        xs.max().unwrap(),
                        ys.max().unwrap(),
                    ]
                }
                _ => continue,
            };
            let url = a.url.trim();
            let target = if url.starts_with('#') {
                match self.target_page(url, n) {
                    Some(p) => Target::Page(p),
                    None => continue,
                }
            } else if ["http:", "https:", "ftp:", "mailto:"]
                .iter()
                .any(|s| url.to_ascii_lowercase().starts_with(s))
            {
                Target::Uri(url.to_string())
            } else {
                continue;
            };
            out.push((r, target));
        }
        out
    }
}

fn fmt_num(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

// ------------------------------------------------------------------ document

/// Convert a DjVu document (bundled or single-page) to PDF, written to `out`.
pub fn convert<W: Write>(data: &[u8], out: W, options: &Options) -> Result<(W, Stats), Error> {
    let doc = DjVuDocument::parse(data).map_err(|e| Error::Parse(e.to_string()))?;
    let npages = doc.page_count();

    // shared JB2 dictionaries in first-use order (by identity of their Djbz bytes);
    // inline page Djbz chunks are decoded here and kept alive in `inline`
    let mut inline: Vec<(usize, Jb2Dict)> = Vec::new();
    for n in 0..npages {
        let page = doc.page(n).map_err(|e| Error::Page(n, e.to_string()))?;
        if let Some(djbz) = page.find_chunk(b"Djbz") {
            let d = djvu_jb2::decode_dict(djbz, None).map_err(|e| Error::Page(n, e.to_string()))?;
            inline.push((n, d));
        }
    }
    let mut keys: Vec<*const u8> = Vec::new();
    let mut dict_refs: Vec<&Jb2Dict> = Vec::new();
    let mut page_dict = Vec::with_capacity(npages);
    let mut inline_iter = inline.iter().peekable();
    for n in 0..npages {
        let page = doc.page(n).map_err(|e| Error::Page(n, e.to_string()))?;
        let entry: Option<(*const u8, &Jb2Dict)> = match inline_iter.peek() {
            Some((m, d)) if *m == n => {
                inline_iter.next();
                Some((d as *const Jb2Dict as *const u8, d))
            }
            _ => page.shared_jb2_dict().map(|(raw, d)| (raw.as_ptr(), d)),
        };
        page_dict.push(entry.map(|(key, d)| {
            keys.iter().position(|&k| k == key).unwrap_or_else(|| {
                keys.push(key);
                dict_refs.push(d);
                dict_refs.len() - 1
            })
        }));
    }
    let dicts = SharedDicts::new(&dict_refs, options.merge_dicts, &options.jbig2);

    // text layers and the fonts covering them
    let mut lines = Vec::with_capacity(npages);
    let mut chars = BTreeSet::new();
    for n in 0..npages {
        let page = doc.page(n).map_err(|e| Error::Page(n, e.to_string()))?;
        let l = match (options.text, page.text_layer_shared()) {
            (true, Ok(Some(layer))) => text::lines(&layer, page.height() as u32),
            _ => Vec::new(),
        };
        text::chars(&l, &mut chars);
        lines.push(l);
    }
    let fonts = (!chars.is_empty()).then(|| TextFonts::new(&chars));

    let page_ids: HashMap<String, usize> = doc
        .component_directory()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.kind == 'P')
        .enumerate()
        .map(|(i, e)| (e.id, i))
        .collect();

    let shared = Shared {
        doc: &doc,
        options,
        dicts,
        page_dict,
        dict_refs,
        lines,
        fonts,
        page_ids,
    };

    let mut pdf = PdfFile::new(out)?;
    let catalog = pdf.alloc();
    let pages_root = pdf.alloc();
    let page_refs: Vec<Ref> = (0..npages).map(|_| pdf.alloc()).collect();

    // shared streams: JBIG2 globals and text fonts
    let mut globals_refs = Vec::new();
    for g in shared.dicts.streams() {
        let id = pdf.alloc();
        let mut c = chunk();
        c.stream(id, &g.data);
        pdf.write_chunk(id, &c)?;
        globals_refs.push(id);
    }
    let font_refs = match &shared.fonts {
        Some(f) => write_fonts(&mut pdf, f)?,
        None => Vec::new(),
    };

    let mut stats = Stats {
        pages: npages,
        jbig2_dictionaries: shared.dicts.streams().len(),
        jbig2_merged: shared.dicts.merged(),
        ..Default::default()
    };

    // pages: converted on worker threads, written in order
    let jobs = options.jobs.max(1).min(npages.max(1));
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let mut result: Result<(), Error> = Ok(());
    std::thread::scope(|s| {
        let (tx, rx) = mpsc::channel::<(usize, Result<PageOut, String>)>();
        for _ in 0..jobs {
            let tx = tx.clone();
            let (shared, next, failed) = (&shared, &next, &failed);
            s.spawn(move || {
                loop {
                    let n = next.fetch_add(1, Ordering::Relaxed);
                    if n >= npages || failed.load(Ordering::Relaxed) {
                        break;
                    }
                    let r =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| shared.page(n)))
                            .unwrap_or_else(|_| Err("internal error (panic)".into()));
                    if tx.send((n, r)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        let mut ready: BTreeMap<usize, PageOut> = BTreeMap::new();
        let mut want = 0;
        for (n, r) in rx {
            match r {
                Ok(p) => {
                    ready.insert(n, p);
                }
                Err(e) => {
                    failed.store(true, Ordering::Relaxed);
                    result = Err(Error::Page(n, e));
                    break;
                }
            }
            while let Some(p) = ready.remove(&want) {
                stats.compound_pages += p.compound as usize;
                stats.text_pages += !p.fonts.is_empty() as usize;
                stats.links += p.links.len();
                if let Err(e) = write_page(
                    &mut pdf,
                    &p,
                    page_refs[want],
                    pages_root,
                    &page_refs,
                    &globals_refs,
                    &font_refs,
                ) {
                    failed.store(true, Ordering::Relaxed);
                    result = Err(e.into());
                    return;
                }
                want += 1;
            }
        }
    });
    result?;

    pdf.object(pages_root, |o| {
        let mut d = o.dict();
        d.pair(Name(b"Type"), Name(b"Pages"));
        d.pair(Name(b"Count"), npages as i32);
        d.insert(Name(b"Kids"))
            .array()
            .items(page_refs.iter().copied());
    })?;

    let outline = if options.outline && !doc.bookmarks().is_empty() {
        let (root, count) = write_outline(&mut pdf, &shared, doc.bookmarks(), &page_refs)?;
        stats.bookmarks = count;
        Some(root)
    } else {
        None
    };
    pdf.object(catalog, |o| {
        let mut d = o.dict();
        d.pair(Name(b"Type"), Name(b"Catalog"));
        d.pair(Name(b"Pages"), pages_root);
        if let Some(root) = outline {
            d.pair(Name(b"Outlines"), root);
            d.pair(Name(b"PageMode"), Name(b"UseOutlines"));
        }
    })?;

    let info = pdf.alloc();
    let meta = doc.metadata().ok().flatten();
    pdf.object(info, |o| {
        let mut d = o.dict();
        d.pair(
            Name(b"Producer"),
            TextStr(&format!("djvu2pdf {}", crate::VERSION)),
        );
        if let Some(m) = &meta {
            for (key, value) in [
                (&b"Title"[..], &m.title),
                (b"Author", &m.author),
                (b"Subject", &m.subject),
                (b"Keywords", &m.keywords),
            ] {
                if let Some(v) = value {
                    d.pair(Name(key), TextStr(v));
                }
            }
        }
    })?;
    let out = pdf.finish(catalog, Some(info))?;
    Ok((out, stats))
}

fn write_fonts<W: Write>(pdf: &mut PdfFile<W>, fonts: &TextFonts) -> std::io::Result<Vec<Ref>> {
    let ttf = pdf.alloc();
    let mut c = chunk();
    let data = deflate(GLYPHLESS_TTF);
    c.stream(ttf, &data)
        .filter(Filter::FlateDecode)
        .pair(Name(b"Length1"), GLYPHLESS_TTF.len() as i32);
    pdf.write_chunk(ttf, &c)?;
    let descriptor = pdf.alloc();
    pdf.object(descriptor, |o| {
        let mut d = o.dict();
        d.pair(Name(b"Type"), Name(b"FontDescriptor"));
        d.pair(Name(b"FontName"), Name(b"GlyphLess"));
        d.pair(Name(b"Flags"), 4);
        d.insert(Name(b"FontBBox"))
            .array()
            .items([0, -200, 500, 800]);
        d.pair(Name(b"ItalicAngle"), 0);
        d.pair(Name(b"Ascent"), 800);
        d.pair(Name(b"Descent"), -200);
        d.pair(Name(b"CapHeight"), 800);
        d.pair(Name(b"StemV"), 80);
        d.pair(Name(b"MissingWidth"), 500);
        d.pair(Name(b"FontFile2"), ttf);
    })?;
    let widths = pdf.alloc();
    pdf.object(widths, |o| {
        o.array().items(std::iter::repeat_n(500, 256));
    })?;
    let mut refs = Vec::new();
    for fi in 0..fonts.fonts.len() {
        let cmap = pdf.alloc();
        let data = deflate(&fonts.to_unicode(fi));
        let mut c = chunk();
        c.stream(cmap, &data).filter(Filter::FlateDecode);
        pdf.write_chunk(cmap, &c)?;
        let font = pdf.alloc();
        pdf.object(font, |o| {
            let mut d = o.dict();
            d.pair(Name(b"Type"), Name(b"Font"));
            d.pair(Name(b"Subtype"), Name(b"TrueType"));
            d.pair(Name(b"BaseFont"), Name(b"GlyphLess"));
            d.pair(Name(b"FirstChar"), 0);
            d.pair(Name(b"LastChar"), 255);
            d.pair(Name(b"Widths"), widths);
            d.pair(Name(b"FontDescriptor"), descriptor);
            d.pair(Name(b"ToUnicode"), cmap);
        })?;
        refs.push(font);
    }
    Ok(refs)
}

fn write_xobject<W: Write>(
    pdf: &mut PdfFile<W>,
    x: &XObject,
    globals: &[Ref],
) -> std::io::Result<Ref> {
    match x {
        XObject::Mask {
            data,
            w,
            h,
            globals: g,
        } => {
            let id = pdf.alloc();
            let mut c = chunk();
            let mut img = c.image_xobject(id, data);
            img.filter(Filter::Jbig2Decode);
            img.width(*w as i32)
                .height(*h as i32)
                .image_mask(true)
                .bits_per_component(1);
            if let Some(g) = g {
                img.decode_parms().jbig2_globals(globals[*g]);
            }
            drop(img);
            pdf.write_chunk(id, &c)?;
            Ok(id)
        }
        XObject::Image { data, w, h, mask } => {
            let mask = mask
                .as_ref()
                .map(|m| write_xobject(pdf, m, globals))
                .transpose()?;
            let id = pdf.alloc();
            let mut c = chunk();
            let mut img = c.image_xobject(id, data);
            img.filter(Filter::JpxDecode);
            img.width(*w as i32).height(*h as i32);
            if let Some(m) = mask {
                img.stencil_mask(m);
            }
            drop(img);
            pdf.write_chunk(id, &c)?;
            Ok(id)
        }
    }
}

fn write_page<W: Write>(
    pdf: &mut PdfFile<W>,
    p: &PageOut,
    id: Ref,
    parent: Ref,
    page_refs: &[Ref],
    globals: &[Ref],
    fonts: &[Ref],
) -> std::io::Result<()> {
    let mut xobjects = Vec::new();
    for (name, x) in &p.xobjects {
        xobjects.push((name.as_str(), write_xobject(pdf, x, globals)?));
    }
    let content = pdf.alloc();
    let data = deflate(&p.content);
    let mut c = chunk();
    c.stream(content, &data).filter(Filter::FlateDecode);
    pdf.write_chunk(content, &c)?;

    let k = 72.0 / p.dpi as f32;
    let mut annots = Vec::new();
    for (r, target) in &p.links {
        let a = pdf.alloc();
        pdf.object(a, |o| {
            let mut d = o.dict();
            d.pair(Name(b"Type"), Name(b"Annot"));
            d.pair(Name(b"Subtype"), Name(b"Link"));
            d.insert(Name(b"Rect"))
                .array()
                .items(r.map(|v| v as f32 * k));
            d.insert(Name(b"Border")).array().items([0, 0, 0]);
            match target {
                Target::Page(n) => {
                    let mut dest = d.insert(Name(b"Dest")).array();
                    dest.item(page_refs[*n]);
                    dest.item(Name(b"Fit"));
                }
                Target::Uri(u) => {
                    let mut action = d.insert(Name(b"A")).dict();
                    action.pair(Name(b"S"), Name(b"URI"));
                    action.pair(Name(b"URI"), Str(u.as_bytes()));
                }
            }
        })?;
        annots.push(a);
    }

    pdf.object(id, |o| {
        let mut d = o.dict();
        d.pair(Name(b"Type"), Name(b"Page"));
        d.pair(Name(b"Parent"), parent);
        d.insert(Name(b"MediaBox")).array().items([
            0.0,
            0.0,
            p.width as f32 * k,
            p.height as f32 * k,
        ]);
        if p.rotate != 0 {
            d.pair(Name(b"Rotate"), p.rotate);
        }
        d.pair(Name(b"Contents"), content);
        let mut res = d.insert(Name(b"Resources")).dict();
        if !xobjects.is_empty() {
            let mut x = res.insert(Name(b"XObject")).dict();
            for (name, r) in &xobjects {
                x.pair(Name(name.as_bytes()), *r);
            }
        }
        if !p.fonts.is_empty() {
            let mut f = res.insert(Name(b"Font")).dict();
            for &fi in &p.fonts {
                f.pair(Name(format!("T{fi}").as_bytes()), fonts[fi]);
            }
        }
        drop(res);
        if !annots.is_empty() {
            d.insert(Name(b"Annots"))
                .array()
                .items(annots.iter().copied());
        }
    })
}

/// Write the outline; returns its root and the number of items.
fn write_outline<W: Write>(
    pdf: &mut PdfFile<W>,
    shared: &Shared,
    items: &[DjVuBookmark],
    page_refs: &[Ref],
) -> std::io::Result<(Ref, usize)> {
    struct Item {
        id: Ref,
        title: String,
        page: usize,
        children: Vec<Item>,
    }
    fn build<W: Write>(pdf: &mut PdfFile<W>, shared: &Shared, items: &[DjVuBookmark]) -> Vec<Item> {
        items
            .iter()
            .map(|b| Item {
                id: pdf.alloc(),
                title: b.title.clone(),
                page: shared.target_page(&b.url, 0).unwrap_or(0),
                children: build(pdf, shared, &b.children),
            })
            .collect()
    }
    fn count(items: &[Item]) -> usize {
        items.iter().map(|i| 1 + count(&i.children)).sum()
    }
    fn write<W: Write>(
        pdf: &mut PdfFile<W>,
        items: &[Item],
        parent: Ref,
        page_refs: &[Ref],
    ) -> std::io::Result<()> {
        for (i, it) in items.iter().enumerate() {
            pdf.object(it.id, |o| {
                let mut d = o.dict();
                d.pair(Name(b"Title"), TextStr(&it.title));
                d.pair(Name(b"Parent"), parent);
                if i > 0 {
                    d.pair(Name(b"Prev"), items[i - 1].id);
                }
                if let Some(n) = items.get(i + 1) {
                    d.pair(Name(b"Next"), n.id);
                }
                if let (Some(first), Some(last)) = (it.children.first(), it.children.last()) {
                    d.pair(Name(b"First"), first.id);
                    d.pair(Name(b"Last"), last.id);
                    d.pair(Name(b"Count"), -(it.children.len() as i32)); // collapsed
                }
                let mut dest = d.insert(Name(b"Dest")).array();
                dest.item(page_refs[it.page]);
                dest.item(Name(b"Fit"));
            })?;
            write(pdf, &it.children, it.id, page_refs)?;
        }
        Ok(())
    }
    let root = pdf.alloc();
    let tree = build(pdf, shared, items);
    write(pdf, &tree, root, page_refs)?;
    pdf.object(root, |o| {
        let mut d = o.dict();
        d.pair(Name(b"Type"), Name(b"Outlines"));
        if let (Some(first), Some(last)) = (tree.first(), tree.last()) {
            d.pair(Name(b"First"), first.id);
            d.pair(Name(b"Last"), last.id);
        }
        d.pair(Name(b"Count"), tree.len() as i32);
    })?;
    Ok((root, count(&tree)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_matches_djvulibre() {
        assert_eq!(compute_red(3132, 5040, 1044, 1680), 3);
        assert_eq!(compute_red(100, 100, 100, 100), 1);
        assert_eq!(compute_red(100, 100, 9, 9), 12);
    }

    #[test]
    fn numbers_are_compact() {
        assert_eq!(fmt_num(0.24), "0.24");
        assert_eq!(fmt_num(72.0 / 600.0), "0.12");
        assert_eq!(rgb_ops([255, 0, 128]), "1 0 0.502 rg");
    }
}
