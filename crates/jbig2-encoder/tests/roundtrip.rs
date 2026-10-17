//! Round trips through an independent decoder (hayro-jbig2).

use jbig2_encoder::{
    Bitmap, EncodeOptions, Globals, Page, PageStats, Placement, Shape, encode_globals, encode_page,
};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn blob(rng: &mut Rng) -> Bitmap {
    let (w, h) = (1 + rng.below(24) as u32, 1 + rng.below(30) as u32);
    let seed = rng.next();
    Bitmap::from_fn(w, h, |x, y| {
        let v = (x as u64 * 31 + y as u64 * 17 + seed) % 7;
        v < 3 || (x + y) % 5 == 0
    })
}

/// A copy of `b` with a few pixels flipped and possibly a changed size.
fn variant(rng: &mut Rng, b: &Bitmap) -> Bitmap {
    let w = (b.width() as i32 + rng.below(3) as i32 - 1).max(1) as u32;
    let h = (b.height() as i32 + rng.below(3) as i32 - 1).max(1) as u32;
    let flips: Vec<(u32, u32)> = (0..3)
        .map(|_| (rng.below(w as u64) as u32, rng.below(h as u64) as u32))
        .collect();
    Bitmap::from_fn(w, h, |x, y| {
        (b.get(x as i32, y as i32) != 0) ^ flips.contains(&(x, y))
    })
}

fn decode(page: &[u8], globals: Option<&Globals>) -> Bitmap {
    struct Collect {
        width: u32,
        pixels: Vec<bool>,
    }
    impl hayro_jbig2::Decoder for Collect {
        fn push_pixel(&mut self, black: bool) {
            self.pixels.push(black);
        }
        fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
            for _ in 0..chunk_count * 8 {
                self.pixels.push(black);
            }
        }
        fn next_line(&mut self) {
            let w = self.width as usize;
            let len = self.pixels.len().div_ceil(w) * w;
            self.pixels.truncate(len);
        }
    }
    let image =
        hayro_jbig2::Image::new_embedded(page, globals.map(|g| g.data.as_slice())).expect("parse");
    let mut c = Collect {
        width: image.width(),
        pixels: Vec::new(),
    };
    image.decode(&mut c).expect("decode");
    let w = image.width();
    Bitmap::from_fn(w, image.height(), |x, y| c.pixels[(y * w + x) as usize])
}

fn render(page: &Page, shapes: &[&Bitmap], placements: &[Placement]) -> Bitmap {
    let (rx, ry, rw, rh) = page.region;
    let mut out = Bitmap::new(page.width, page.height);
    for p in placements {
        let b = shapes[p.shape];
        for y in 0..b.height() as i32 {
            for x in 0..b.width() as i32 {
                let (px, py) = (p.x + x, p.y + y);
                if b.get(x, y) != 0 && px >= 0 && py >= 0 && px < rw as i32 && py < rh as i32 {
                    out.set(rx + px as u32, ry + py as u32, true);
                }
            }
        }
    }
    out
}

fn check(
    page: &Page,
    global: &[Shape],
    local: &[Shape],
    placements: &[Placement],
    options: &EncodeOptions,
) -> PageStats {
    let globals = (!global.is_empty()).then(|| encode_globals(global, options));
    let encoded = encode_page(page, globals.as_ref(), local, placements, options);
    let shapes: Vec<&Bitmap> = global.iter().chain(local).map(|s| &s.bitmap).collect();
    let expected = render(page, &shapes, placements);
    let got = decode(&encoded.data, globals.as_ref());
    assert_eq!(got.width(), expected.width());
    assert_eq!(got.height(), expected.height());
    for y in 0..expected.height() {
        assert_eq!(
            got.row(y),
            expected.row(y),
            "row {y} differs ({:?})",
            encoded.stats
        );
    }
    encoded.stats
}

fn scatter(rng: &mut Rng, n_shapes: usize, count: usize, w: u32, h: u32) -> Vec<Placement> {
    (0..count)
        .map(|_| Placement {
            shape: rng.below(n_shapes as u64) as usize,
            x: rng.below(w as u64 + 20) as i32 - 10,
            y: rng.below(h as u64 + 20) as i32 - 10,
        })
        .collect()
}

#[test]
fn local_shapes_only() {
    let mut rng = Rng(0x1234_5678);
    for n in [1, 2, 3, 4, 5, 8, 16, 33] {
        let local: Vec<Shape> = (0..n)
            .map(|_| Shape {
                bitmap: blob(&mut rng),
                parent: None,
            })
            .collect();
        let page = Page::new(300, 200, 300);
        let placements = scatter(&mut rng, n, 3 * n, 300, 200);
        check(&page, &[], &local, &placements, &EncodeOptions::default());
    }
}

#[test]
fn empty_page() {
    check(
        &Page::new(50, 40, 300),
        &[],
        &[],
        &[],
        &EncodeOptions::default(),
    );
}

#[test]
fn globals_refinements_and_aliases() {
    let mut rng = Rng(0xdead_beef);
    let (mut inline, mut aliased, mut dict) = (0, 0, 0);
    for (n_global, n_local) in [(1, 1), (4, 4), (7, 9), (31, 1), (32, 0), (64, 20)] {
        let mut global: Vec<Shape> = Vec::new();
        for i in 0..n_global {
            let parent = (i > 0 && rng.below(3) == 0).then(|| rng.below(i as u64) as usize);
            let bitmap = match parent {
                Some(p) => variant(&mut rng, &global[p].bitmap),
                None => blob(&mut rng),
            };
            global.push(Shape { bitmap, parent });
        }
        let mut local: Vec<Shape> = Vec::new();
        for i in 0..n_local {
            let shape = match rng.below(4) {
                0 => Shape {
                    bitmap: global[rng.below(n_global as u64) as usize].bitmap.clone(),
                    parent: None,
                }, // alias
                1 => {
                    let p = rng.below((n_global + i) as u64) as usize;
                    let base = if p < n_global {
                        &global[p].bitmap
                    } else {
                        &local[p - n_global].bitmap
                    };
                    Shape {
                        bitmap: variant(&mut rng, base),
                        parent: Some(p),
                    }
                }
                _ => Shape {
                    bitmap: blob(&mut rng),
                    parent: None,
                },
            };
            local.push(shape);
        }
        let page = Page::new(400, 300, 600);
        let placements = scatter(
            &mut rng,
            n_global + n_local,
            4 * (n_global + n_local),
            400,
            300,
        );
        for options in [
            EncodeOptions::default(),
            EncodeOptions {
                refine: false,
                ..Default::default()
            },
            EncodeOptions {
                inline_refine: false,
                ..Default::default()
            },
            EncodeOptions {
                search: true,
                ..Default::default()
            },
        ] {
            let stats = check(&page, &global, &local, &placements, &options);
            inline += stats.inline_shapes;
            aliased += stats.aliased_shapes;
            dict += stats.dictionary_shapes;
        }
    }
    assert!(
        inline > 0 && aliased > 0 && dict > 0,
        "{inline} {aliased} {dict}"
    );
}

#[test]
fn refinement_chains_and_empty_shapes() {
    let mut rng = Rng(42);
    let mut local = vec![
        Shape {
            bitmap: blob(&mut rng),
            parent: None,
        },
        Shape {
            bitmap: Bitmap::default(),
            parent: None,
        },
    ];
    for i in 2..12 {
        let bitmap = variant(
            &mut rng,
            &local[i - 1].bitmap.clone().max_nonempty(&local[0].bitmap),
        );
        local.push(Shape {
            bitmap,
            parent: Some(i - 1),
        });
    }
    let page = Page::new(120, 90, 300);
    let placements = scatter(&mut rng, local.len(), 30, 120, 90);
    let stats = check(&page, &[], &local, &placements, &EncodeOptions::default());
    assert!(stats.dictionary_shapes >= 10, "{stats:?}");
}

#[test]
fn sub_region() {
    let mut rng = Rng(7);
    let local: Vec<Shape> = (0..6)
        .map(|_| Shape {
            bitmap: blob(&mut rng),
            parent: None,
        })
        .collect();
    let mut page = Page::new(200, 150, 300);
    page.region = (13, 21, 160, 100);
    let placements = scatter(&mut rng, 6, 25, 160, 100);
    check(&page, &[], &local, &placements, &EncodeOptions::default());
}

trait NonEmpty {
    fn max_nonempty(self, fallback: &Bitmap) -> Bitmap;
}

impl NonEmpty for Bitmap {
    fn max_nonempty(self, fallback: &Bitmap) -> Bitmap {
        if self.is_empty() {
            fallback.clone()
        } else {
            self
        }
    }
}

/// Refinements wider/taller than their reference (positive reference offsets), in a
/// dictionary and inline. hayro-jbig2 0.3.1 mis-decoded these (vendor/hayro-jbig2).
#[test]
fn refinement_with_reference_offset() {
    let parent = Bitmap::from_fn(21, 30, |x, y| (x * 7 + y * 3) % 5 < 2 || x == 20);
    let wider = Bitmap::from_fn(23, 31, |x, y| {
        parent.get(x as i32 - 1, y as i32) != 0 || y == 30
    });
    let narrower = Bitmap::from_fn(19, 29, |x, y| parent.get(x as i32 + 1, y as i32 + 1) != 0);
    let global = [
        Shape {
            bitmap: parent,
            parent: None,
        },
        Shape {
            bitmap: wider.clone(),
            parent: Some(0),
        },
    ];
    let local = [
        Shape {
            bitmap: Bitmap::from_fn(25, 32, |x, y| wider.get(x as i32 - 1, y as i32) != 0),
            parent: Some(1),
        },
        Shape {
            bitmap: narrower,
            parent: Some(0),
        },
    ];
    let page = Page::new(200, 100, 300);
    let placements: Vec<Placement> = (0..4)
        .map(|s| Placement {
            shape: s,
            x: 10 + 40 * s as i32,
            y: 20,
        })
        .collect();
    for options in [
        EncodeOptions::default(),
        EncodeOptions {
            inline_refine: false,
            ..Default::default()
        },
    ] {
        check(&page, &global, &local, &placements, &options);
    }
}
