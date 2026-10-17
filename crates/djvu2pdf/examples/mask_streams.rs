//! Write the JBIG2 streams of a DjVu document's masks: the global streams (`g<k>.jb2`)
//! and one page stream per page (`p<n>.jb2`, 1-based) — the layout of the C++ prototype
//! (`jb2jbig2`), for comparison.
//!
//! Usage: mask_streams [--verify] [--merge-dicts=auto|always|never] in.djvu OUTDIR
//!
//! `--verify` decodes every page stream with hayro-jbig2 and compares it with
//! djvu-rs's JB2 rendering.

use djvu_rs::djvu_document::DjVuDocument;
use djvu2pdf::mask::{MergeDicts, SharedDicts, encode_mask};
use jbig2_encoder::EncodeOptions;

fn main() {
    let mut verify = false;
    let mut merge = MergeDicts::Auto;
    let mut positional = Vec::new();
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--verify" => verify = true,
            "--merge-dicts=auto" => merge = MergeDicts::Auto,
            "--merge-dicts=always" => merge = MergeDicts::Always,
            "--merge-dicts=never" => merge = MergeDicts::Never,
            _ => positional.push(a),
        }
    }
    let [input, outdir] = positional.as_slice() else {
        eprintln!(
            "usage: mask_streams [--verify] [--merge-dicts=auto|always|never] in.djvu OUTDIR"
        );
        std::process::exit(2);
    };
    let data = std::fs::read(input).expect("read input");
    let doc = DjVuDocument::parse(&data).expect("parse");
    std::fs::create_dir_all(outdir).expect("create OUTDIR");
    let options = EncodeOptions::default();
    let start = std::time::Instant::now();

    // distinct shared dictionaries in first-use order, and each page's dictionary
    // inline page dictionaries (Djbz in the page itself) are decoded here and kept alive
    let inline: Vec<Option<djvu_jb2::Jb2Dict>> = (0..doc.page_count())
        .map(|n| {
            let page = doc.page(n).expect("page");
            page.find_chunk(b"Djbz")
                .map(|d| djvu_jb2::decode_dict(d, None).expect("decode Djbz"))
        })
        .collect();
    // distinct dictionaries in first-use order, and each page's dictionary
    let mut keys: Vec<*const u8> = Vec::new();
    let mut dicts = Vec::new();
    let mut page_dict = Vec::new();
    for (n, own) in inline.iter().enumerate() {
        let page = doc.page(n).expect("page");
        let entry = match own {
            Some(d) => Some((d as *const djvu_jb2::Jb2Dict as *const u8, d)),
            None => page.shared_jb2_dict().map(|(raw, d)| (raw.as_ptr(), d)),
        };
        page_dict.push(entry.map(|(key, dict)| {
            keys.iter().position(|&k| k == key).unwrap_or_else(|| {
                keys.push(key);
                dicts.push(dict);
                dicts.len() - 1
            })
        }));
    }
    let shared = SharedDicts::new(&dicts, merge, &options);
    let mut global_bytes = 0;
    for (k, g) in shared.streams().iter().enumerate() {
        std::fs::write(format!("{outdir}/g{k}.jb2"), &g.data).unwrap();
        global_bytes += g.data.len();
    }

    let (mut page_bytes, mut sjbz_bytes, mut mismatches) = (0, 0, 0);
    for (n, &dict) in page_dict.iter().enumerate() {
        let page = doc.page(n).expect("page");
        let Some(sjbz) = page.find_chunk(b"Sjbz") else {
            continue;
        };
        sjbz_bytes += sjbz.len();
        let jb2_dict = dict.map(|k| dicts[k]);
        let globals = shared.page_globals(dict);
        let encoded =
            encode_mask(sjbz, jb2_dict, globals, page.dpi() as u32, &options).expect("decode JB2");
        page_bytes += encoded.data.len();
        std::fs::write(format!("{outdir}/p{}.jb2", n + 1), &encoded.data).unwrap();
        if verify {
            let expected = djvu_jb2::decode(sjbz, jb2_dict).expect("decode JB2");
            let globals = globals.map(|g| shared.streams()[g.stream].data.as_slice());
            if decode_jbig2(&encoded.data, globals) != expected {
                mismatches += 1;
                eprintln!("page {}: decoded JBIG2 differs from the JB2 mask", n + 1);
            }
        }
    }
    println!(
        "{} dicts -> {} streams{} {} B, pages {} B, total {} B; Sjbz {} B; {:.2}s",
        dicts.len(),
        shared.streams().len(),
        if shared.merged() { " (merged)" } else { "" },
        global_bytes,
        page_bytes,
        global_bytes + page_bytes,
        sjbz_bytes,
        start.elapsed().as_secs_f64()
    );
    if verify {
        println!("verify: {mismatches} page(s) differ");
        if mismatches > 0 {
            std::process::exit(1);
        }
    }
}

/// Decode a PDF-embedded JBIG2 stream into a djvu-rs bitmap.
fn decode_jbig2(page: &[u8], globals: Option<&[u8]>) -> djvu_rs::Bitmap {
    struct Collect {
        bitmap: djvu_rs::Bitmap,
        x: u32,
        y: u32,
    }
    impl hayro_jbig2::Decoder for Collect {
        fn push_pixel(&mut self, black: bool) {
            if black {
                self.bitmap.set_black(self.x, self.y);
            }
            self.x += 1;
        }
        fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
            for _ in 0..chunk_count * 8 {
                if self.x < self.bitmap.width {
                    self.push_pixel(black);
                }
            }
        }
        fn next_line(&mut self) {
            self.x = 0;
            self.y += 1;
        }
    }
    let image = hayro_jbig2::Image::new_embedded(page, globals).expect("parse JBIG2");
    let mut c = Collect {
        bitmap: djvu_rs::Bitmap::new(image.width(), image.height()),
        x: 0,
        y: 0,
    };
    image.decode(&mut c).expect("decode JBIG2");
    c.bitmap
}
